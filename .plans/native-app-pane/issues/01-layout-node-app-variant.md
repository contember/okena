# Issue 01: Add LayoutNode::App variant and AppKind enum

**Priority:** high
**Files:** `src/workspace/state.rs`, `crates/okena-core/src/api.rs`

## Description

Add the `App` variant to `LayoutNode` in `src/workspace/state.rs` and the corresponding `ApiLayoutNode::App` variant in `crates/okena-core/src/api.rs`. This is the foundational change that all other issues depend on.

## Changes to `src/workspace/state.rs`

### Add `AppKind` enum

Add before the `LayoutNode` enum:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    #[default]
    Kruh,
}
```

### Add `App` variant to `LayoutNode`

```rust
App {
    app_id: Option<String>,
    #[serde(default)]
    app_kind: AppKind,
    #[serde(default)]
    app_config: serde_json::Value,
},
```

### Add helper methods to `LayoutNode`

- `pub fn new_app(kind: AppKind, config: serde_json::Value) -> Self` — creates `App { app_id: None, app_kind: kind, app_config: config }`
- `pub fn collect_app_ids(&self) -> Vec<String>` — recursively collects all `app_id` values (skipping `None`), similar to `collect_terminal_ids()`
- `pub fn find_app_path(&self, target_id: &str) -> Option<Vec<usize>>` — similar to `find_terminal_path()`, returns path to the App node with matching app_id

### Update all match arms on `LayoutNode`

Search through the entire file for `match` statements on `LayoutNode` variants. The `App` variant is a **leaf** like `Terminal`:

- `collect_terminal_ids()` / `collect_terminal_ids_into()` — `App { .. } => {}` (no-op, apps don't contribute terminal IDs)
- `find_terminal_path()` — `App { .. } => None` (apps are not terminals)
- `normalize()` — `App { .. } => {}` (no-op, already a leaf)
- `get_at_path()` / `get_at_path_mut()` — `App { .. } => None` when path is non-empty (leaf has no children)
- Any other match arms — treat as leaf, same as `Terminal`
- `count_leaves()` if it exists — `App { .. } => 1`
- `all_terminals_hidden()` / terminal visibility checks — `App` is never "hidden", so return `false`

### Serde backward compatibility

The `#[serde(tag = "type")]` on `LayoutNode` means old JSON without `"type": "app"` will still deserialize fine. New App nodes serialize with `"type": "app"`.

## Changes to `crates/okena-core/src/api.rs`

### Add `App` variant to `ApiLayoutNode`

```rust
App {
    app_id: Option<String>,
    app_kind: String,  // Use String here since okena-core doesn't need the full AppKind enum
},
```

### Update `collect_terminal_ids_into()`

Add `ApiLayoutNode::App { .. } => {}` (no-op).

### Update conversion from internal LayoutNode to ApiLayoutNode

If there's a `From<LayoutNode> for ApiLayoutNode` impl or similar conversion, add the `App` arm.

## Tests (`src/workspace/state.rs` `#[cfg(test)]`)

Add a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_serialization_roundtrip() {
        let node = LayoutNode::new_app(AppKind::Kruh, serde_json::json!({"docs_dir": "/tmp"}));
        let json = serde_json::to_string(&node).unwrap();
        let deserialized: LayoutNode = serde_json::from_str(&json).unwrap();
        // Verify fields match
    }

    #[test]
    fn test_collect_terminal_ids_excludes_apps() {
        let tree = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![0.5, 0.5],
            children: vec![
                LayoutNode::Terminal { terminal_id: Some("t1".into()), minimized: false, detached: false, shell_type: ShellType::default(), zoom_level: 1.0 },
                LayoutNode::new_app(AppKind::Kruh, serde_json::Value::Null),
            ],
        };
        let ids = tree.collect_terminal_ids();
        assert_eq!(ids, vec!["t1"]);
    }

    #[test]
    fn test_collect_app_ids() {
        // Build a tree with mixed Terminal + App leaves, verify only app_ids returned
    }

    #[test]
    fn test_find_app_path() {
        // Nested split/tab tree with an App leaf, verify path found
    }

    #[test]
    fn test_backward_compat_no_app_nodes() {
        let json = r#"{"type":"terminal","terminal_id":"t1"}"#;
        let node: LayoutNode = serde_json::from_str(json).unwrap();
        assert!(matches!(node, LayoutNode::Terminal { .. }));
    }
}
```

## Acceptance Criteria

- `LayoutNode::App` compiles and serializes/deserializes correctly
- All existing match arms updated — no compiler warnings about non-exhaustive patterns
- `collect_terminal_ids()` excludes app nodes
- `collect_app_ids()` and `find_app_path()` work correctly
- Existing tests pass (no regressions)
- `cargo build` succeeds with no warnings related to the new variant
