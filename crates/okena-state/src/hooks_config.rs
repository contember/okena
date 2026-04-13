//! Lifecycle hook configuration.
//!
//! Backward-compatible deserialization: accepts both the legacy flat key format
//! and the new grouped (`project`/`terminal`/`worktree`) format.

use serde::{Deserialize, Serialize};

/// Project lifecycle hooks
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectHooks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_open: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_close: Option<String>,
}

/// Terminal lifecycle hooks
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TerminalHooks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_create: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_close: Option<String>,
    /// Shell wrapper template. `{shell}` is replaced with the resolved shell command.
    /// Example: `devcontainer exec --workspace-folder $OKENA_PROJECT_PATH -- {shell}`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_wrapper: Option<String>,
}

/// Worktree lifecycle hooks
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorktreeHooks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_create: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_close: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_merge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_merge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_remove: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_remove: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_rebase_conflict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_dirty_close: Option<String>,
}

/// Grouped hook configuration (project, terminal, worktree).
/// Backward-compatible: deserializes both the old flat format and the new grouped format.
#[derive(Clone, Debug, Default, Serialize)]
pub struct HooksConfig {
    #[serde(default, skip_serializing_if = "is_default")]
    pub project: ProjectHooks,
    #[serde(default, skip_serializing_if = "is_default")]
    pub terminal: TerminalHooks,
    #[serde(default, skip_serializing_if = "is_default")]
    pub worktree: WorktreeHooks,
}

fn is_default<T: Default + PartialEq>(val: &T) -> bool {
    *val == T::default()
}

const FLAT_HOOK_KEYS: &[&str] = &[
    "on_project_open", "on_project_close",
    "on_worktree_create", "on_worktree_close",
    "pre_merge", "post_merge",
    "before_worktree_remove", "worktree_removed",
    "on_rebase_conflict", "on_dirty_worktree_close",
];

impl<'de> Deserialize<'de> for HooksConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let obj = match value.as_object() {
            Some(o) => o,
            None => return Ok(HooksConfig::default()),
        };

        let is_new_format = obj.contains_key("project") || obj.contains_key("terminal") || obj.contains_key("worktree");
        let has_flat_keys = !is_new_format && FLAT_HOOK_KEYS.iter().any(|k| obj.contains_key(*k));

        if has_flat_keys {
            let s = |key: &str| -> Option<String> {
                obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
            };
            Ok(HooksConfig {
                project: ProjectHooks {
                    on_open: s("on_project_open"),
                    on_close: s("on_project_close"),
                },
                terminal: TerminalHooks::default(),
                worktree: WorktreeHooks {
                    on_create: s("on_worktree_create"),
                    on_close: s("on_worktree_close"),
                    pre_merge: s("pre_merge"),
                    post_merge: s("post_merge"),
                    before_remove: s("before_worktree_remove"),
                    after_remove: s("worktree_removed"),
                    on_rebase_conflict: s("on_rebase_conflict"),
                    on_dirty_close: s("on_dirty_worktree_close"),
                },
            })
        } else {
            let deser = |key: &str| -> serde_json::Value {
                obj.get(key).cloned().unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
            };
            let project: ProjectHooks = serde_json::from_value(deser("project"))
                .unwrap_or_default();
            let terminal: TerminalHooks = serde_json::from_value(deser("terminal"))
                .unwrap_or_default();
            let worktree: WorktreeHooks = serde_json::from_value(deser("worktree"))
                .unwrap_or_default();
            Ok(HooksConfig { project, terminal, worktree })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_flat_format_full() {
        // workspace.json from a pre-grouped install — every legacy key set.
        let json = r#"{
            "on_project_open": "echo open",
            "on_project_close": "echo close",
            "on_worktree_create": "echo wt-create",
            "on_worktree_close": "echo wt-close",
            "pre_merge": "echo pre",
            "post_merge": "echo post",
            "before_worktree_remove": "echo before-rm",
            "worktree_removed": "echo after-rm",
            "on_rebase_conflict": "echo rebase",
            "on_dirty_worktree_close": "echo dirty"
        }"#;

        let config: HooksConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.project.on_open.as_deref(), Some("echo open"));
        assert_eq!(config.project.on_close.as_deref(), Some("echo close"));
        assert_eq!(config.worktree.on_create.as_deref(), Some("echo wt-create"));
        assert_eq!(config.worktree.on_close.as_deref(), Some("echo wt-close"));
        assert_eq!(config.worktree.pre_merge.as_deref(), Some("echo pre"));
        assert_eq!(config.worktree.post_merge.as_deref(), Some("echo post"));
        assert_eq!(config.worktree.before_remove.as_deref(), Some("echo before-rm"));
        assert_eq!(config.worktree.after_remove.as_deref(), Some("echo after-rm"));
        assert_eq!(config.worktree.on_rebase_conflict.as_deref(), Some("echo rebase"));
        assert_eq!(config.worktree.on_dirty_close.as_deref(), Some("echo dirty"));
        // Terminal section never existed in the legacy format.
        assert_eq!(config.terminal, TerminalHooks::default());
    }

    #[test]
    fn migrates_legacy_flat_format_partial() {
        let json = r#"{ "on_project_open": "setup.sh" }"#;
        let config: HooksConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.project.on_open.as_deref(), Some("setup.sh"));
        assert!(config.project.on_close.is_none());
        assert_eq!(config.worktree, WorktreeHooks::default());
        assert_eq!(config.terminal, TerminalHooks::default());
    }

    #[test]
    fn parses_new_grouped_format() {
        let json = r#"{
            "project": { "on_open": "echo open", "on_close": "echo close" },
            "terminal": { "on_create": "echo tc", "shell_wrapper": "wrap {shell}" },
            "worktree": { "on_create": "echo wtc", "pre_merge": "echo pm" }
        }"#;

        let config: HooksConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.project.on_open.as_deref(), Some("echo open"));
        assert_eq!(config.project.on_close.as_deref(), Some("echo close"));
        assert_eq!(config.terminal.on_create.as_deref(), Some("echo tc"));
        assert_eq!(config.terminal.shell_wrapper.as_deref(), Some("wrap {shell}"));
        assert_eq!(config.worktree.on_create.as_deref(), Some("echo wtc"));
        assert_eq!(config.worktree.pre_merge.as_deref(), Some("echo pm"));
    }

    #[test]
    fn parses_empty_object_as_default() {
        let config: HooksConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.project, ProjectHooks::default());
        assert_eq!(config.terminal, TerminalHooks::default());
        assert_eq!(config.worktree, WorktreeHooks::default());
    }

    #[test]
    fn parses_partial_new_format_fills_missing_groups_with_default() {
        // Only `project` group present — terminal and worktree should default.
        let json = r#"{ "project": { "on_open": "init.sh" } }"#;
        let config: HooksConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.project.on_open.as_deref(), Some("init.sh"));
        assert_eq!(config.terminal, TerminalHooks::default());
        assert_eq!(config.worktree, WorktreeHooks::default());
    }

    #[test]
    fn serializes_to_grouped_format_with_empty_groups_omitted() {
        let config = HooksConfig {
            project: ProjectHooks {
                on_open: Some("setup".into()),
                on_close: None,
            },
            terminal: TerminalHooks::default(),
            worktree: WorktreeHooks::default(),
        };

        let json = serde_json::to_value(&config).unwrap();
        // is_default subgroups are skipped on serialization.
        assert!(json.get("terminal").is_none(), "default terminal should be omitted");
        assert!(json.get("worktree").is_none(), "default worktree should be omitted");
        let project = json.get("project").expect("project group present");
        assert_eq!(project.get("on_open").and_then(|v| v.as_str()), Some("setup"));
        assert!(project.get("on_close").is_none(), "None on_close should be omitted");
    }

    #[test]
    fn legacy_to_new_roundtrip_via_serialize() {
        // Loading a legacy file and saving it should produce the new grouped format
        // with semantically equivalent content.
        let legacy = r#"{
            "on_project_open": "open",
            "pre_merge": "pm"
        }"#;

        let config: HooksConfig = serde_json::from_str(legacy).unwrap();
        let serialized = serde_json::to_string(&config).unwrap();

        // The serialized output must be in the new grouped format.
        assert!(serialized.contains("\"project\""), "expected grouped 'project' key");
        assert!(serialized.contains("\"worktree\""), "expected grouped 'worktree' key");
        assert!(!serialized.contains("\"on_project_open\""), "legacy keys must not survive");

        // And re-parsing must yield the same content.
        let reparsed: HooksConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reparsed.project.on_open.as_deref(), Some("open"));
        assert_eq!(reparsed.worktree.pre_merge.as_deref(), Some("pm"));
        assert!(reparsed.project.on_close.is_none());
    }

    #[test]
    fn new_format_takes_precedence_when_both_present() {
        // Edge case: a hand-edited workspace.json with both layouts. The grouped
        // format wins per current detection logic, and legacy keys are ignored.
        let json = r#"{
            "project": { "on_open": "new" },
            "on_project_open": "legacy"
        }"#;

        let config: HooksConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.project.on_open.as_deref(), Some("new"));
    }

    #[test]
    fn null_or_non_object_falls_back_to_default() {
        // Defensive — a stray `"hooks": null` in workspace.json must not crash.
        let config: HooksConfig = serde_json::from_str("null").unwrap();
        assert_eq!(config.project, ProjectHooks::default());
        assert_eq!(config.terminal, TerminalHooks::default());
        assert_eq!(config.worktree, WorktreeHooks::default());
    }
}
