use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KruhConfig {
    pub agent: String,
    pub model: String,
    pub max_iterations: usize,
    pub sleep_secs: u64,
    pub docs_dir: String,
    pub dangerous: bool,
    #[serde(default)]
    pub plans_dir: String,
}

/// Per-plan overrides parsed from YAML frontmatter in INSTRUCTIONS.md.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KruhPlanOverrides {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub max_iterations: Option<usize>,
    pub sleep_secs: Option<u64>,
    pub dangerous: Option<bool>,
}

impl KruhPlanOverrides {
    /// Return a list of human-readable override labels (e.g., "agent: codex", "model: gpt-4").
    pub fn labels(&self) -> Vec<String> {
        let mut labels = Vec::new();
        if let Some(agent) = &self.agent {
            labels.push(format!("agent: {agent}"));
        }
        if let Some(model) = &self.model {
            labels.push(format!("model: {model}"));
        }
        if let Some(n) = self.max_iterations {
            labels.push(format!("iters: {n}"));
        }
        if let Some(s) = self.sleep_secs {
            labels.push(format!("sleep: {s}s"));
        }
        if let Some(d) = self.dangerous {
            labels.push(format!("dangerous: {d}"));
        }
        labels
    }

    pub fn has_any(&self) -> bool {
        self.agent.is_some()
            || self.model.is_some()
            || self.max_iterations.is_some()
            || self.sleep_secs.is_some()
            || self.dangerous.is_some()
    }

    /// Serialize overrides to YAML frontmatter string. Returns empty string if no overrides.
    pub fn to_frontmatter(&self) -> String {
        if !self.has_any() {
            return String::new();
        }
        let mut lines = vec!["---".to_string()];
        if let Some(agent) = &self.agent {
            lines.push(format!("agent: {agent}"));
        }
        if let Some(model) = &self.model {
            lines.push(format!("model: {model}"));
        }
        if let Some(n) = self.max_iterations {
            lines.push(format!("max_iterations: {n}"));
        }
        if let Some(s) = self.sleep_secs {
            lines.push(format!("sleep_secs: {s}"));
        }
        if let Some(d) = self.dangerous {
            lines.push(format!("dangerous: {d}"));
        }
        lines.push("---".to_string());
        lines.join("\n") + "\n"
    }
}

impl KruhConfig {
    /// Return a new config with plan-level overrides applied.
    pub fn with_overrides(&self, overrides: &KruhPlanOverrides) -> Self {
        Self {
            agent: overrides.agent.clone().unwrap_or_else(|| self.agent.clone()),
            model: overrides.model.clone().unwrap_or_else(|| self.model.clone()),
            max_iterations: overrides.max_iterations.unwrap_or(self.max_iterations),
            sleep_secs: overrides.sleep_secs.unwrap_or(self.sleep_secs),
            dangerous: overrides.dangerous.unwrap_or(self.dangerous),
            docs_dir: self.docs_dir.clone(),
            plans_dir: self.plans_dir.clone(),
        }
    }
}

impl Default for KruhConfig {
    fn default() -> Self {
        Self {
            agent: "claude".into(),
            model: "claude-sonnet-4-6".into(),
            max_iterations: 100,
            sleep_secs: 2,
            docs_dir: String::new(),
            dangerous: true,
            plans_dir: String::new(),
        }
    }
}

pub struct AgentDef {
    pub name: &'static str,
    #[allow(dead_code)]
    pub binary: &'static str,
}

impl AgentDef {
    pub fn build_command(&self, config: &KruhConfig, prompt: &str) -> Vec<String> {
        match self.name {
            "claude" => {
                let mut args = vec!["claude".into()];
                if !config.model.is_empty() {
                    args.extend(["--model".into(), config.model.clone()]);
                }
                if config.dangerous {
                    args.push("--dangerously-skip-permissions".into());
                }
                args.extend(["-p".into(), prompt.into()]);
                args
            }
            "codex" => vec!["codex".into(), "-q".into(), prompt.into()],
            "opencode" => {
                let mut args = vec!["opencode".into(), "run".into()];
                if !config.model.is_empty() {
                    args.extend(["--model".into(), config.model.clone()]);
                }
                args.push(prompt.into());
                args
            }
            "aider" => {
                let mut args =
                    vec!["aider".into(), "--yes".into(), "--message".into(), prompt.into()];
                if !config.model.is_empty() {
                    args.extend(["--model".into(), config.model.clone()]);
                }
                args
            }
            "goose" => vec!["goose".into(), "run".into(), "-t".into(), prompt.into()],
            "amp" => vec!["amp".into(), "-x".into(), prompt.into()],
            "cursor" => vec!["cursor".into(), "--cli".into(), prompt.into()],
            "copilot" => vec!["copilot".into(), prompt.into()],
            _ => vec![],
        }
    }
}

pub const AGENTS: &[AgentDef] = &[
    AgentDef { name: "claude", binary: "claude" },
    AgentDef { name: "codex", binary: "codex" },
    AgentDef { name: "opencode", binary: "opencode" },
    AgentDef { name: "aider", binary: "aider" },
    AgentDef { name: "goose", binary: "goose" },
    AgentDef { name: "amp", binary: "amp" },
    AgentDef { name: "cursor", binary: "cursor" },
    AgentDef { name: "copilot", binary: "copilot" },
];

#[allow(dead_code)]
pub fn detect_agents() -> Vec<&'static str> {
    AGENTS.iter().filter(|a| which::which(a.binary).is_ok()).map(|a| a.name).collect()
}

pub fn find_agent(name: &str) -> Option<&'static AgentDef> {
    AGENTS.iter().find(|a| a.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = KruhConfig::default();
        assert_eq!(config.agent, "claude");
        assert_eq!(config.max_iterations, 100);
        assert_eq!(config.sleep_secs, 2);
        assert!(config.dangerous);
        assert!(config.docs_dir.is_empty());
    }

    #[test]
    fn test_config_serialization() {
        let config = KruhConfig::default();
        let json = serde_json::to_value(&config).unwrap();
        let deserialized: KruhConfig = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent, config.agent);
        assert_eq!(deserialized.model, config.model);
        assert_eq!(deserialized.max_iterations, config.max_iterations);
        assert_eq!(deserialized.sleep_secs, config.sleep_secs);
        assert_eq!(deserialized.dangerous, config.dangerous);
    }

    #[test]
    fn test_claude_command_building() {
        let config = KruhConfig {
            agent: "claude".into(),
            model: "opus".into(),
            dangerous: true,
            ..Default::default()
        };
        let agent = find_agent("claude").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(
            cmd,
            vec!["claude", "--model", "opus", "--dangerously-skip-permissions", "-p", "do stuff"]
        );
    }

    #[test]
    fn test_claude_command_no_model() {
        let config = KruhConfig {
            agent: "claude".into(),
            model: String::new(),
            dangerous: false,
            ..Default::default()
        };
        let agent = find_agent("claude").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["claude", "-p", "do stuff"]);
    }

    #[test]
    fn test_codex_command_building() {
        let config = KruhConfig::default();
        let agent = find_agent("codex").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["codex", "-q", "do stuff"]);
    }

    #[test]
    fn test_opencode_command_building() {
        let config =
            KruhConfig { model: "gpt-4".into(), ..Default::default() };
        let agent = find_agent("opencode").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["opencode", "run", "--model", "gpt-4", "do stuff"]);
    }

    #[test]
    fn test_opencode_command_no_model() {
        let config = KruhConfig { model: String::new(), ..Default::default() };
        let agent = find_agent("opencode").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["opencode", "run", "do stuff"]);
    }

    #[test]
    fn test_aider_command_building() {
        let config =
            KruhConfig { model: "gpt-4".into(), ..Default::default() };
        let agent = find_agent("aider").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["aider", "--yes", "--message", "do stuff", "--model", "gpt-4"]);
    }

    #[test]
    fn test_aider_command_no_model() {
        let config = KruhConfig { model: String::new(), ..Default::default() };
        let agent = find_agent("aider").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["aider", "--yes", "--message", "do stuff"]);
    }

    #[test]
    fn test_goose_command_building() {
        let config = KruhConfig::default();
        let agent = find_agent("goose").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["goose", "run", "-t", "do stuff"]);
    }

    #[test]
    fn test_amp_command_building() {
        let config = KruhConfig::default();
        let agent = find_agent("amp").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["amp", "-x", "do stuff"]);
    }

    #[test]
    fn test_cursor_command_building() {
        let config = KruhConfig::default();
        let agent = find_agent("cursor").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["cursor", "--cli", "do stuff"]);
    }

    #[test]
    fn test_copilot_command_building() {
        let config = KruhConfig::default();
        let agent = find_agent("copilot").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["copilot", "do stuff"]);
    }

    #[test]
    fn test_find_agent_exists() {
        assert!(find_agent("claude").is_some());
        assert!(find_agent("codex").is_some());
        assert!(find_agent("aider").is_some());
    }

    #[test]
    fn test_find_agent_not_exists() {
        assert!(find_agent("nonexistent").is_none());
    }

    #[test]
    fn test_agents_count() {
        assert_eq!(AGENTS.len(), 8);
    }

    #[test]
    fn test_with_overrides_full() {
        let config = KruhConfig::default();
        let overrides = KruhPlanOverrides {
            agent: Some("codex".into()),
            model: Some("gpt-4".into()),
            max_iterations: Some(50),
            sleep_secs: Some(5),
            dangerous: Some(false),
        };
        let effective = config.with_overrides(&overrides);
        assert_eq!(effective.agent, "codex");
        assert_eq!(effective.model, "gpt-4");
        assert_eq!(effective.max_iterations, 50);
        assert_eq!(effective.sleep_secs, 5);
        assert!(!effective.dangerous);
    }

    #[test]
    fn test_with_overrides_partial() {
        let config = KruhConfig::default();
        let overrides = KruhPlanOverrides {
            agent: Some("aider".into()),
            ..Default::default()
        };
        let effective = config.with_overrides(&overrides);
        assert_eq!(effective.agent, "aider");
        assert_eq!(effective.model, config.model);
        assert_eq!(effective.max_iterations, config.max_iterations);
        assert_eq!(effective.sleep_secs, config.sleep_secs);
        assert_eq!(effective.dangerous, config.dangerous);
    }

    #[test]
    fn test_with_overrides_empty() {
        let config = KruhConfig::default();
        let overrides = KruhPlanOverrides::default();
        let effective = config.with_overrides(&overrides);
        assert_eq!(effective.agent, config.agent);
        assert_eq!(effective.model, config.model);
        assert_eq!(effective.max_iterations, config.max_iterations);
        assert_eq!(effective.sleep_secs, config.sleep_secs);
        assert_eq!(effective.dangerous, config.dangerous);
    }

    #[test]
    fn test_to_frontmatter_empty() {
        let overrides = KruhPlanOverrides::default();
        assert_eq!(overrides.to_frontmatter(), "");
    }

    #[test]
    fn test_to_frontmatter_full() {
        let overrides = KruhPlanOverrides {
            agent: Some("codex".into()),
            model: Some("gpt-4".into()),
            max_iterations: Some(50),
            sleep_secs: Some(5),
            dangerous: Some(false),
        };
        let fm = overrides.to_frontmatter();
        assert!(fm.starts_with("---\n"));
        assert!(fm.ends_with("---\n"));
        assert!(fm.contains("agent: codex\n"));
        assert!(fm.contains("model: gpt-4\n"));
        assert!(fm.contains("max_iterations: 50\n"));
        assert!(fm.contains("sleep_secs: 5\n"));
        assert!(fm.contains("dangerous: false\n"));
    }

    #[test]
    fn test_to_frontmatter_partial() {
        let overrides = KruhPlanOverrides {
            agent: Some("aider".into()),
            ..Default::default()
        };
        let fm = overrides.to_frontmatter();
        assert_eq!(fm, "---\nagent: aider\n---\n");
    }

    #[test]
    fn test_frontmatter_roundtrip() {
        use crate::views::layout::kruh_pane::status_parser::parse_plan_overrides_content;

        let original = KruhPlanOverrides {
            agent: Some("codex".into()),
            model: Some("gpt-4".into()),
            max_iterations: Some(50),
            sleep_secs: Some(5),
            dangerous: Some(false),
        };
        let fm = original.to_frontmatter();
        let body = "# Instructions\n";
        let content = format!("{fm}\n{body}");
        let parsed = parse_plan_overrides_content(&content);
        assert_eq!(parsed.agent, original.agent);
        assert_eq!(parsed.model, original.model);
        assert_eq!(parsed.max_iterations, original.max_iterations);
        assert_eq!(parsed.sleep_secs, original.sleep_secs);
        assert_eq!(parsed.dangerous, original.dangerous);
    }
}
