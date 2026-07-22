use crate::manager::ServiceStatus;
use okena_core::process;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Timeout for Docker CLI commands.  When the Docker daemon is not running the
/// CLI can hang for many seconds waiting for a connection; this cap prevents it
/// from blocking background executor threads and starving the UI.
const DOCKER_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a *successful* `docker compose version` check stays cached. Once
/// Docker is up it stays up for the session.
const AVAILABLE_TTL_OK: Duration = Duration::from_secs(60);
/// How long a *failed* check stays cached — short, so Docker Desktop starting
/// later is picked up quickly and a `false` never becomes sticky.
const AVAILABLE_TTL_FAIL: Duration = Duration::from_secs(5);

/// Cached `docker compose version` result with the time it was taken.
static AVAILABLE_CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
/// Single-flight gate so the per-project pollers waking together at startup
/// don't all spawn `docker compose version` at once.
static AVAILABLE_REFRESH_LOCK: Mutex<()> = Mutex::new(());

/// Check if `docker compose` CLI is available.
///
/// Cached with an asymmetric TTL — 60s for success, 5s for failure — so the
/// check isn't re-spawned on every service poll, yet a not-yet-running Docker
/// is re-probed every few seconds rather than disabled for the session.
/// Refreshes are single-flighted to avoid a startup thundering herd.
pub fn is_docker_compose_available() -> bool {
    let cached = || {
        AVAILABLE_CACHE.lock().ok().and_then(|g| {
            g.and_then(|(ts, ok)| {
                let ttl = if ok {
                    AVAILABLE_TTL_OK
                } else {
                    AVAILABLE_TTL_FAIL
                };
                (ts.elapsed() < ttl).then_some(ok)
            })
        })
    };

    if let Some(ok) = cached() {
        return ok;
    }
    let _flight = AVAILABLE_REFRESH_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(ok) = cached() {
        return ok;
    }

    let mut cmd = process::command("docker");
    cmd.args(["compose", "version"]);
    let ok = process::safe_output_with_timeout(&mut cmd, DOCKER_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false);

    if let Ok(mut guard) = AVAILABLE_CACHE.lock() {
        *guard = Some((Instant::now(), ok));
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

    let config: ComposeConfig =
        serde_json::from_str(json).map_err(|e| ServiceError::ParseError {
            context: "docker compose config JSON".to_string(),
            detail: e.to_string(),
        })?;

    Ok(config
        .services
        .into_iter()
        .filter(|(_, svc)| !matches!(svc.deploy, Some(DeployConfig { replicas: Some(0) })))
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

/// How long a `docker ps -a` snapshot is reused before refreshing.
const PS_SNAPSHOT_TTL: Duration = Duration::from_secs(4);

/// Shared snapshot of *all* compose containers on the host, from a single
/// `docker ps -a`. Per-project pollers used to each spawn `docker compose ps`
/// every 5s; with N compose projects that was N spawns per cycle. They now
/// share this one snapshot, so the host is queried at most once per TTL
/// regardless of how many projects are polling.
static PS_SNAPSHOT: Mutex<Option<(Instant, Vec<ContainerSnapshot>)>> = Mutex::new(None);

/// Single-flight gate for refreshing [`PS_SNAPSHOT`]. Without it, all per-project
/// pollers waking on the same ~5s tick find the cache stale at once and each
/// spawn their own `docker ps` before any stores a result (thundering herd).
static PS_REFRESH_LOCK: Mutex<()> = Mutex::new(());

/// One compose container distilled from `docker ps -a --format json`.
#[derive(Clone)]
struct ContainerSnapshot {
    /// `com.docker.compose.project.working_dir` — the project directory, used
    /// to match a container back to an okena project. (We avoid matching on
    /// `config_files`: its value can contain commas, which collide with the
    /// label-list separator in `docker ps`'s flattened `Labels` string.)
    working_dir: Option<std::path::PathBuf>,
    service: String,
    state: String,
    exit_code: Option<u32>,
    ports: Vec<u16>,
}

/// Raw JSON shape from `docker ps -a --format json` (NDJSON).
#[derive(Deserialize)]
struct DockerPsAEntry {
    #[serde(rename = "Labels")]
    labels: Option<String>,
    #[serde(rename = "State")]
    state: Option<String>,
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(rename = "Ports")]
    ports: Option<String>,
}

/// Value of `key` in docker's flattened `k=v,k=v` label string. Robust to
/// commas inside *other* values (those fragments simply lack `=` for `key`).
fn label_value<'a>(labels: &'a str, key: &str) -> Option<&'a str> {
    labels.split(',').find_map(|kv| {
        kv.split_once('=')
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v)
    })
}

/// Parse the exit code out of a `docker ps` Status like "Exited (137) 2h ago".
fn parse_exit_code(status: &str) -> Option<u32> {
    let start = status.find("Exited (")? + "Exited (".len();
    let rest = &status[start..];
    rest[..rest.find(')')?].parse().ok()
}

/// Extract published host ports from a `docker ps` Ports string, e.g.
/// "1025/tcp, 0.0.0.0:3006->8025/tcp, [::]:3006->8025/tcp" -> [3006].
fn parse_published_ports(ports: &str) -> Vec<u16> {
    let mut out: Vec<u16> = ports
        .split(',')
        .filter_map(|part| {
            let host = &part[..part.find("->")?];
            host[host.rfind(':')? + 1..].trim().parse::<u16>().ok()
        })
        .filter(|&p| p > 0)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Refresh the shared snapshot via a single `docker ps -a`.
fn refresh_ps_snapshot() -> crate::ServiceResult<Vec<ContainerSnapshot>> {
    use crate::error::ServiceError;

    let mut cmd = process::command("docker");
    cmd.args(["ps", "-a", "--format", "json", "--no-trunc"]);
    let output = process::safe_output_with_timeout(&mut cmd, DOCKER_TIMEOUT)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ServiceError::CommandExitError {
            context: "docker ps".to_string(),
            stderr,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut containers = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(entry) = serde_json::from_str::<DockerPsAEntry>(line) else {
            continue;
        };
        let labels = entry.labels.unwrap_or_default();
        // Only compose-managed containers carry a service label.
        let Some(service) = label_value(&labels, "com.docker.compose.service") else {
            continue;
        };
        let state = entry.state.unwrap_or_else(|| "unknown".to_string());
        let exit_code = entry
            .status
            .as_deref()
            .and_then(parse_exit_code)
            .or(if state == "exited" { Some(0) } else { None });
        containers.push(ContainerSnapshot {
            working_dir: label_value(&labels, "com.docker.compose.project.working_dir")
                .map(std::path::PathBuf::from),
            service: service.to_string(),
            state,
            exit_code,
            ports: entry
                .ports
                .as_deref()
                .map(parse_published_ports)
                .unwrap_or_default(),
        });
    }
    Ok(containers)
}

/// Get the shared container snapshot, refreshing it if older than the TTL.
/// Refreshes are single-flighted: concurrent callers that find the cache stale
/// block on `PS_REFRESH_LOCK`, then re-check and reuse the snapshot the winner
/// just stored, so only one `docker ps` runs per refresh window.
fn ps_snapshot() -> crate::ServiceResult<Vec<ContainerSnapshot>> {
    let fresh_cached = || {
        PS_SNAPSHOT.lock().ok().and_then(|g| {
            g.as_ref()
                .filter(|(ts, _)| ts.elapsed() < PS_SNAPSHOT_TTL)
                .map(|(_, snap)| snap.clone())
        })
    };

    if let Some(snap) = fresh_cached() {
        return Ok(snap);
    }

    // Stale: serialize refreshes. Whoever gets the lock first refreshes; the
    // rest re-check below and reuse the freshly stored snapshot.
    let _flight = PS_REFRESH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(snap) = fresh_cached() {
        return Ok(snap);
    }

    let fresh = refresh_ps_snapshot()?;
    if let Ok(mut guard) = PS_SNAPSHOT.lock() {
        *guard = Some((Instant::now(), fresh.clone()));
    }
    Ok(fresh)
}

/// Poll status of all services in the compose project.
///
/// Reads from the shared `docker ps -a` snapshot and filters to the containers
/// whose compose project working-dir matches `project_path` — so no per-project
/// `docker compose ps` spawn.
pub fn poll_status(
    project_path: &str,
    _compose_file: &str,
) -> crate::ServiceResult<Vec<DockerServiceStatus>> {
    let project_canon = std::path::Path::new(project_path)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(project_path));

    let snapshot = ps_snapshot()?;
    Ok(snapshot
        .into_iter()
        .filter(|c| {
            c.working_dir.as_ref().is_some_and(|w| {
                let wc = w.canonicalize().unwrap_or_else(|_| w.clone());
                wc == project_canon
            })
        })
        .map(|c| DockerServiceStatus {
            name: c.service,
            state: c.state,
            exit_code: c.exit_code,
            ports: c.ports,
        })
        .collect())
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
        serde_json::from_str(trimmed).map_err(|e| ServiceError::ParseError {
            context: "docker ps JSON array".to_string(),
            detail: e.to_string(),
        })?
    } else {
        // NDJSON format (one JSON object per line)
        trimmed
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<DockerPsEntry>(line).map_err(|e| ServiceError::ParseError {
                    context: "docker ps line".to_string(),
                    detail: e.to_string(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    Ok(entries
        .into_iter()
        .map(|e| {
            let ports = extract_ports(&e.publishers);
            let name = e.service_name.or(e.container_name).unwrap_or_default();
            DockerServiceStatus {
                name,
                state: e.state.unwrap_or_else(|| "unknown".to_string()),
                exit_code: e.exit_code,
                ports,
            }
        })
        .collect())
}

/// Extract published host ports from the Publishers array.
fn extract_ports(publishers: &Option<Vec<Publisher>>) -> Vec<u16> {
    let Some(pubs) = publishers else {
        return Vec::new();
    };
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
        assert_eq!(
            map_docker_state("restarting", None),
            ServiceStatus::Restarting
        );
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
            ServiceStatus::Crashed {
                exit_code: Some(137)
            }
        );
        assert_eq!(
            map_docker_state("unknown_state", None),
            ServiceStatus::Stopped
        );
    }

    #[test]
    fn test_parse_publishers_ports() {
        let pubs = vec![
            Publisher {
                published_port: Some(8080),
            },
            Publisher {
                published_port: Some(0),
            },
            Publisher {
                published_port: None,
            },
            Publisher {
                published_port: Some(3000),
            },
            Publisher {
                published_port: Some(8080),
            }, // duplicate
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
