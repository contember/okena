use okena_core::process;
use crate::manager::ServiceStatus;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Timeout for Docker CLI commands.  When the Docker daemon is not running the
/// CLI can hang for many seconds waiting for a connection; this cap prevents it
/// from blocking background executor threads and starving the UI.
const DOCKER_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a *successful* `docker compose version` check stays cached.
const AVAILABLE_TTL: Duration = Duration::from_secs(60);

/// Last time `docker compose version` was confirmed available. Only successes
/// are cached (see [`is_docker_compose_available`]).
static AVAILABLE_OK_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// Check if `docker compose` CLI is available.
///
/// Caches **only success** for [`AVAILABLE_TTL`]: once Docker is up it stays up
/// for the session, so re-spawning `docker compose version` on every service
/// poll (per project) is pure waste. A *failure* is deliberately never cached —
/// Docker Desktop may not have started yet, and a sticky `false` would
/// permanently disable the integration; so we keep re-checking until it works.
pub fn is_docker_compose_available() -> bool {
    if let Ok(guard) = AVAILABLE_OK_AT.lock()
        && let Some(ts) = *guard
        && ts.elapsed() < AVAILABLE_TTL
    {
        return true;
    }

    let mut cmd = process::command("docker");
    cmd.args(["compose", "version"]);
    let ok = process::safe_output_with_timeout(&mut cmd, DOCKER_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false);

    if ok && let Ok(mut guard) = AVAILABLE_OK_AT.lock() {
        *guard = Some(Instant::now());
    }
    ok
}

/// Compose file names to probe, in priority order.
const COMPOSE_FILE_NAMES: &[&str] = &[
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];

/// Detect a compose file in `project_path`. Returns the filename if found.
pub fn detect_compose_file(project_path: &str) -> Option<String> {
    let base = std::path::Path::new(project_path);
    for name in COMPOSE_FILE_NAMES {
        if base.join(name).exists() {
            return Some(name.to_string());
        }
    }
    None
}

/// Cache of parsed service lists keyed by `(project_path, compose_file)`,
/// invalidated by the compose file's modification time. `docker compose config`
/// is a heavy spawn whose output only changes when the file does, yet the
/// service poller asks for it repeatedly — this serves the parsed list from
/// memory until the file actually changes.
#[allow(clippy::type_complexity)]
static CONFIG_CACHE: Mutex<Option<HashMap<(String, String), (SystemTime, Vec<String>)>>> =
    Mutex::new(None);

/// List service names defined in a compose file.
/// Excludes services with `deploy.replicas = 0`.
///
/// Result is cached per compose file and reused until the file's mtime changes,
/// so repeated polls don't re-spawn `docker compose config`.
pub fn list_services(project_path: &str, compose_file: &str) -> crate::ServiceResult<Vec<String>> {
    use crate::error::ServiceError;

    let key = (project_path.to_string(), compose_file.to_string());
    let mtime = std::path::Path::new(project_path)
        .join(compose_file)
        .metadata()
        .and_then(|m| m.modified())
        .ok();

    // Serve from cache when the file hasn't changed since we last parsed it.
    if let Some(mt) = mtime
        && let Ok(guard) = CONFIG_CACHE.lock()
        && let Some(cache) = guard.as_ref()
        && let Some((cached_mt, services)) = cache.get(&key)
        && *cached_mt == mt
    {
        return Ok(services.clone());
    }

    let mut cmd = process::command("docker");
    cmd.args(["compose", "-f", compose_file, "config", "--format", "json"])
        .current_dir(project_path);

    let output = process::safe_output_with_timeout(&mut cmd, DOCKER_TIMEOUT)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ServiceError::CommandExitError {
            context: "docker compose config".to_string(),
            stderr,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let services = parse_compose_config_services(&stdout)?;

    // Cache the parsed list against the file mtime so subsequent polls skip the
    // spawn until the compose file changes.
    if let Some(mt) = mtime
        && let Ok(mut guard) = CONFIG_CACHE.lock()
    {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(key, (mt, services.clone()));
    }

    Ok(services)
}

/// Parse `docker compose config --format json` output and return service names,
/// filtering out services that have `deploy.replicas` set to 0.
fn parse_compose_config_services(json: &str) -> crate::ServiceResult<Vec<String>> {
    use crate::error::ServiceError;

    let config: ComposeConfig = serde_json::from_str(json)
        .map_err(|e| ServiceError::ParseError {
            context: "docker compose config JSON".to_string(),
            detail: e.to_string(),
        })?;

    Ok(config
        .services
        .into_iter()
        .filter(|(_, svc)| {
            !matches!(svc.deploy, Some(DeployConfig { replicas: Some(0) }))
        })
        .map(|(name, _)| name)
        .collect())
}

#[derive(Deserialize)]
struct ComposeConfig {
    #[serde(default)]
    services: std::collections::HashMap<String, ComposeService>,
}

#[derive(Deserialize)]
struct ComposeService {
    deploy: Option<DeployConfig>,
}

#[derive(Deserialize)]
struct DeployConfig {
    replicas: Option<u32>,
}

/// Parsed status of one Docker service.
#[derive(Clone, Debug)]
pub struct DockerServiceStatus {
    pub name: String,
    pub state: String,
    pub exit_code: Option<u32>,
    pub ports: Vec<u16>,
}

/// Raw JSON shape from `docker compose ps --format json`.
/// Each line is a separate JSON object (NDJSON).
/// Docker CLI versions may use PascalCase or lowercase keys.
#[derive(Deserialize)]
struct DockerPsEntry {
    #[serde(alias = "service", rename = "Service")]
    service_name: Option<String>,

    #[serde(alias = "name", rename = "Name")]
    container_name: Option<String>,

    #[serde(alias = "state", rename = "State")]
    state: Option<String>,

    #[serde(alias = "exit_code", rename = "ExitCode")]
    exit_code: Option<u32>,

    #[serde(alias = "publishers", rename = "Publishers")]
    publishers: Option<Vec<Publisher>>,
}

#[derive(Deserialize)]
struct Publisher {
    #[serde(alias = "published_port", rename = "PublishedPort")]
    published_port: Option<u16>,
}

/// Poll status of all services in the compose project.
pub fn poll_status(project_path: &str, compose_file: &str) -> crate::ServiceResult<Vec<DockerServiceStatus>> {
    use crate::error::ServiceError;

    let mut cmd = process::command("docker");
    cmd.args(["compose", "-f", compose_file, "ps", "--format", "json", "-a"])
        .current_dir(project_path);

    let output = process::safe_output_with_timeout(&mut cmd, DOCKER_TIMEOUT)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ServiceError::CommandExitError {
            context: "docker compose ps".to_string(),
            stderr,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_docker_ps_output(&stdout)
}

/// Parse the output of `docker compose ps --format json`.
/// Docker outputs either NDJSON (one JSON object per line) or a JSON array.
pub fn parse_docker_ps_output(output: &str) -> crate::ServiceResult<Vec<DockerServiceStatus>> {
    use crate::error::ServiceError;

    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let entries: Vec<DockerPsEntry> = if trimmed.starts_with('[') {
        // JSON array format
        serde_json::from_str(trimmed)
            .map_err(|e| ServiceError::ParseError {
                context: "docker ps JSON array".to_string(),
                detail: e.to_string(),
            })?
    } else {
        // NDJSON format (one JSON object per line)
        trimmed
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<DockerPsEntry>(line)
                    .map_err(|e| ServiceError::ParseError {
                        context: "docker ps line".to_string(),
                        detail: e.to_string(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    Ok(entries.into_iter().map(|e| {
        let ports = extract_ports(&e.publishers);
        let name = e.service_name
            .or(e.container_name)
            .unwrap_or_default();
        DockerServiceStatus {
            name,
            state: e.state.unwrap_or_else(|| "unknown".to_string()),
            exit_code: e.exit_code,
            ports,
        }
    }).collect())
}

/// Extract published host ports from the Publishers array.
fn extract_ports(publishers: &Option<Vec<Publisher>>) -> Vec<u16> {
    let Some(pubs) = publishers else { return Vec::new() };
    let mut ports: Vec<u16> = pubs
        .iter()
        .filter_map(|p| p.published_port)
        .filter(|&p| p > 0)
        .collect();
    ports.sort();
    ports.dedup();
    ports
}

/// Map Docker state string to ServiceStatus enum.
pub fn map_docker_state(state: &str, exit_code: Option<u32>) -> ServiceStatus {
    match state.to_lowercase().as_str() {
        "running" => ServiceStatus::Running,
        "restarting" => ServiceStatus::Restarting,
        "paused" => ServiceStatus::Running, // still technically alive
        "created" => ServiceStatus::Stopped,
        "exited" => {
            if exit_code == Some(0) {
                ServiceStatus::Stopped
            } else {
                ServiceStatus::Crashed { exit_code }
            }
        }
        "dead" => ServiceStatus::Crashed { exit_code },
        _ => ServiceStatus::Stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_docker_ps_json_ndjson() {
        let output = r#"{"Service":"web","Name":"myapp-web-1","State":"running","ExitCode":0,"Publishers":[{"PublishedPort":8080},{"PublishedPort":0}]}
{"Service":"db","Name":"myapp-db-1","State":"exited","ExitCode":1,"Publishers":[]}
{"Service":"redis","Name":"myapp-redis-1","State":"running","ExitCode":0,"Publishers":[{"PublishedPort":6379}]}"#;

        let result = parse_docker_ps_output(output).unwrap();
        assert_eq!(result.len(), 3);

        assert_eq!(result[0].name, "web");
        assert_eq!(result[0].state, "running");
        assert_eq!(result[0].ports, vec![8080]);

        assert_eq!(result[1].name, "db");
        assert_eq!(result[1].state, "exited");
        assert_eq!(result[1].exit_code, Some(1));
        assert!(result[1].ports.is_empty());

        assert_eq!(result[2].name, "redis");
        assert_eq!(result[2].ports, vec![6379]);
    }

    #[test]
    fn test_parse_docker_ps_json_array() {
        let output = r#"[{"Service":"web","Name":"myapp-web-1","State":"running","ExitCode":0,"Publishers":[{"PublishedPort":3000}]}]"#;

        let result = parse_docker_ps_output(output).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "web");
        assert_eq!(result[0].ports, vec![3000]);
    }

    #[test]
    fn test_parse_docker_ps_empty() {
        assert!(parse_docker_ps_output("").unwrap().is_empty());
        assert!(parse_docker_ps_output("  \n  ").unwrap().is_empty());
    }

    #[test]
    fn test_map_docker_state() {
        assert_eq!(map_docker_state("running", None), ServiceStatus::Running);
        assert_eq!(map_docker_state("Running", None), ServiceStatus::Running);
        assert_eq!(map_docker_state("restarting", None), ServiceStatus::Restarting);
        assert_eq!(map_docker_state("paused", None), ServiceStatus::Running);
        assert_eq!(map_docker_state("created", None), ServiceStatus::Stopped);
        assert_eq!(map_docker_state("exited", Some(0)), ServiceStatus::Stopped);
        assert_eq!(
            map_docker_state("exited", Some(1)),
            ServiceStatus::Crashed { exit_code: Some(1) }
        );
        assert_eq!(
            map_docker_state("exited", None),
            ServiceStatus::Crashed { exit_code: None }
        );
        assert_eq!(
            map_docker_state("dead", Some(137)),
            ServiceStatus::Crashed { exit_code: Some(137) }
        );
        assert_eq!(map_docker_state("unknown_state", None), ServiceStatus::Stopped);
    }

    #[test]
    fn test_parse_publishers_ports() {
        let pubs = vec![
            Publisher { published_port: Some(8080) },
            Publisher { published_port: Some(0) },
            Publisher { published_port: None },
            Publisher { published_port: Some(3000) },
            Publisher { published_port: Some(8080) }, // duplicate
        ];
        let ports = extract_ports(&Some(pubs));
        assert_eq!(ports, vec![3000, 8080]);

        assert!(extract_ports(&None).is_empty());
        assert!(extract_ports(&Some(vec![])).is_empty());
    }

    #[test]
    fn test_detect_compose_file_priority() {
        // Just verify the priority order constant
        assert_eq!(COMPOSE_FILE_NAMES[0], "docker-compose.yml");
        assert_eq!(COMPOSE_FILE_NAMES[1], "docker-compose.yaml");
        assert_eq!(COMPOSE_FILE_NAMES[2], "compose.yml");
        assert_eq!(COMPOSE_FILE_NAMES[3], "compose.yaml");
    }

    #[test]
    fn test_parse_compose_config_filters_zero_replicas() {
        let json = r#"{
            "services": {
                "web": {},
                "db": { "deploy": { "replicas": 1 } },
                "worker": { "deploy": { "replicas": 0 } },
                "debug-tools": { "deploy": { "replicas": 0 } },
                "redis": { "deploy": {} }
            }
        }"#;

        let mut result = parse_compose_config_services(json).unwrap();
        result.sort();
        assert_eq!(result, vec!["db", "redis", "web"]);
    }
}
