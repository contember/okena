use crate::process;
use crate::services::manager::ServiceStatus;
use serde::Deserialize;
use std::sync::OnceLock;

/// Check if `docker compose` CLI is available (cached via OnceLock).
pub fn is_docker_compose_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let mut cmd = process::command("docker");
        cmd.args(["compose", "version"]);
        process::safe_output(&mut cmd)
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
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

/// List service names defined in a compose file.
pub fn list_services(project_path: &str, compose_file: &str) -> Result<Vec<String>, String> {
    let mut cmd = process::command("docker");
    cmd.args(["compose", "-f", compose_file, "config", "--services"])
        .current_dir(project_path);

    let output = process::safe_output(&mut cmd)
        .map_err(|e| format!("Failed to run docker compose config: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("docker compose config failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().filter(|l| !l.is_empty()).map(String::from).collect())
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
pub fn poll_status(project_path: &str, compose_file: &str) -> Result<Vec<DockerServiceStatus>, String> {
    let mut cmd = process::command("docker");
    cmd.args(["compose", "-f", compose_file, "ps", "--format", "json", "-a"])
        .current_dir(project_path);

    let output = process::safe_output(&mut cmd)
        .map_err(|e| format!("Failed to run docker compose ps: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("docker compose ps failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_docker_ps_output(&stdout)
}

/// Parse the output of `docker compose ps --format json`.
/// Docker outputs either NDJSON (one JSON object per line) or a JSON array.
pub fn parse_docker_ps_output(output: &str) -> Result<Vec<DockerServiceStatus>, String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let entries: Vec<DockerPsEntry> = if trimmed.starts_with('[') {
        // JSON array format
        serde_json::from_str(trimmed)
            .map_err(|e| format!("Failed to parse docker ps JSON array: {}", e))?
    } else {
        // NDJSON format (one JSON object per line)
        trimmed
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<DockerPsEntry>(line)
                    .map_err(|e| format!("Failed to parse docker ps line: {}", e))
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
}
