# Issue 03: KruhPane types, config, and agent definitions

**Priority:** high
**Files:** `src/views/layout/kruh_pane/types.rs` (new), `src/views/layout/kruh_pane/config.rs` (new), `Cargo.toml`

## Description

Create the foundational types and configuration for the KruhPane. These are standalone modules with no dependencies on other new code, so they can be built early.

## New file: `src/views/layout/kruh_pane/types.rs`

### `KruhState` enum

```rust
#[derive(Clone, Debug, Default, PartialEq)]
pub enum KruhState {
    #[default]
    Idle,
    Running,
    Paused,
    WaitingForStep,
    Completed,
}
```

### `OutputLine` struct

```rust
#[derive(Clone, Debug)]
pub struct OutputLine {
    pub text: String,
    pub timestamp: std::time::Instant,
    pub is_error: bool,
}
```

### `KruhPaneEvent` enum

```rust
pub enum KruhPaneEvent {
    Close,
}
```

### `StatusProgress` struct

```rust
#[derive(Clone, Debug, Default)]
pub struct StatusProgress {
    pub pending: usize,
    pub done: usize,
    pub total: usize,
    pub pending_issues: Vec<String>,
    pub done_issues: Vec<String>,
    pub pending_refs: Vec<IssueRef>,
    pub done_refs: Vec<IssueRef>,
}
```

### `IssueRef` struct

```rust
#[derive(Clone, Debug)]
pub struct IssueRef {
    pub number: String,
    pub name: String,
}
```

## New file: `src/views/layout/kruh_pane/config.rs`

### `KruhConfig` struct

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KruhConfig {
    pub agent: String,
    pub model: String,
    pub max_iterations: usize,
    pub sleep_secs: u64,
    pub docs_dir: String,
    pub dangerous: bool,
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
        }
    }
}
```

### `AgentDef` struct and agent definitions

```rust
pub struct AgentDef {
    pub name: &'static str,
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
                let mut args = vec!["aider".into(), "--yes".into(), "--message".into(), prompt.into()];
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
```

### `detect_agents()` function

```rust
pub fn detect_agents() -> Vec<&'static str> {
    AGENTS.iter()
        .filter(|a| which::which(a.binary).is_ok())
        .map(|a| a.name)
        .collect()
}

pub fn find_agent(name: &str) -> Option<&'static AgentDef> {
    AGENTS.iter().find(|a| a.name == name)
}
```

## Changes to `Cargo.toml`

Add `which` dependency if not already present:

```toml
which = "7"
```

(`serde_json` and `uuid` are already in the dependencies.)

## Tests

Add tests in `config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = KruhConfig::default();
        assert_eq!(config.agent, "claude");
        assert_eq!(config.max_iterations, 100);
    }

    #[test]
    fn test_config_serialization() {
        let config = KruhConfig::default();
        let json = serde_json::to_value(&config).unwrap();
        let deserialized: KruhConfig = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.agent, config.agent);
    }

    #[test]
    fn test_claude_command_building() {
        let config = KruhConfig { agent: "claude".into(), model: "opus".into(), dangerous: true, ..Default::default() };
        let agent = find_agent("claude").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["claude", "--model", "opus", "--dangerously-skip-permissions", "-p", "do stuff"]);
    }

    #[test]
    fn test_codex_command_building() {
        let config = KruhConfig::default();
        let agent = find_agent("codex").unwrap();
        let cmd = agent.build_command(&config, "do stuff");
        assert_eq!(cmd, vec!["codex", "-q", "do stuff"]);
    }

    // Add tests for each agent's command building
}
```

## Acceptance Criteria

- `KruhConfig` serializes/deserializes to JSON correctly
- All 8 agents have correct command building
- `detect_agents()` returns available agents on the system
- `which` crate added to Cargo.toml
- `cargo build` and `cargo test` succeed
