use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OkenaProjectConfig {
    #[serde(default)]
    pub services: Vec<ServiceDefinition>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceDefinition {
    pub name: String,
    pub command: String,
    #[serde(default = "default_cwd")]
    pub cwd: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub restart_on_crash: bool,
    #[serde(default = "default_restart_delay")]
    pub restart_delay_ms: u64,
}

fn default_cwd() -> String {
    ".".to_string()
}

fn default_restart_delay() -> u64 {
    1000
}

/// Load project config from `{project_path}/okena.yaml`.
///
/// Returns `Ok(None)` if the file doesn't exist, `Err` on parse failure.
pub fn load_project_config(project_path: &str) -> Result<Option<OkenaProjectConfig>, String> {
    let path = Path::new(project_path).join("okena.yaml");
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let config: OkenaProjectConfig =
        serde_yaml::from_str(&content).map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
    Ok(Some(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let yaml = r#"
services:
  - name: "Vite Dev"
    command: "npm run dev"
"#;
        let config: OkenaProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.services.len(), 1);
        assert_eq!(config.services[0].name, "Vite Dev");
        assert_eq!(config.services[0].command, "npm run dev");
        // Verify defaults
        assert_eq!(config.services[0].cwd, ".");
        assert!(!config.services[0].auto_start);
        assert!(!config.services[0].restart_on_crash);
        assert_eq!(config.services[0].restart_delay_ms, 1000);
        assert!(config.services[0].env.is_empty());
    }

    #[test]
    fn parse_full_config() {
        let yaml = r#"
services:
  - name: "Vite Dev"
    command: "npm run dev"
    cwd: "frontend"
    env:
      NODE_ENV: development
      PORT: "3000"
    auto_start: true
    restart_on_crash: true
    restart_delay_ms: 2000
"#;
        let config: OkenaProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.services.len(), 1);
        let svc = &config.services[0];
        assert_eq!(svc.name, "Vite Dev");
        assert_eq!(svc.command, "npm run dev");
        assert_eq!(svc.cwd, "frontend");
        assert_eq!(svc.env.get("NODE_ENV").unwrap(), "development");
        assert_eq!(svc.env.get("PORT").unwrap(), "3000");
        assert!(svc.auto_start);
        assert!(svc.restart_on_crash);
        assert_eq!(svc.restart_delay_ms, 2000);
    }

    #[test]
    fn parse_empty_services() {
        let yaml = "services: []\n";
        let config: OkenaProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.services.is_empty());
    }

    #[test]
    fn default_values() {
        let yaml = r#"
services:
  - name: "test"
    command: "echo hi"
"#;
        let config: OkenaProjectConfig = serde_yaml::from_str(yaml).unwrap();
        let svc = &config.services[0];
        assert_eq!(svc.cwd, ".");
        assert!(!svc.auto_start);
        assert!(!svc.restart_on_crash);
        assert_eq!(svc.restart_delay_ms, 1000);
    }
}
