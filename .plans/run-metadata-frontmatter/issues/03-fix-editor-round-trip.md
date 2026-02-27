# Issue 03: Fix editor round-trip to preserve run metadata

**Priority:** high
**Files:** `src/views/layout/kruh_pane/mod.rs`, `src/views/layout/kruh_pane/status_parser.rs`

## Description

The issue editor (`open_editor_at` / `save_editor`) currently reconstructs frontmatter from `KruhPlanOverrides::to_frontmatter()`, which only knows 5 config keys: `agent`, `model`, `max_iterations`, `sleep_secs`, `dangerous`. After issue 02 writes run metadata keys (`startedAt`, `endedAt`, `exitCode`, `duration`, `iteration`) to the frontmatter, opening and saving the issue in the editor would strip those keys.

Fix this by storing the extra (non-config) frontmatter keys when opening the editor and merging them back when saving.

## Changes to `mod.rs`

### 1. Add field to `KruhPane` struct

Add a new field after the existing `editor_fm_dangerous` field:

```rust
pub editor_fm_extra: Vec<(String, String)>,
```

Initialize it to `Vec::new()` in `KruhPane::new()`.

### 2. Populate in `open_editor_at`

In the `if target == EditTarget::Issue` branch (line ~281), after parsing overrides, extract extra keys using the new `extract_extra_frontmatter_keys` function:

```rust
self.editor_fm_extra = status_parser::extract_extra_frontmatter_keys(&content);
```

### 3. Merge in `save_editor`

In `save_editor`, when `editor_target == Some(EditTarget::Issue)` (line ~352), instead of just calling `overrides.to_frontmatter()`, build the full content using `update_frontmatter_content`:

```rust
let content = if self.editor_target == Some(EditTarget::Issue) {
    let overrides = self.read_frontmatter_inputs(cx);
    // Start with body content
    let mut full_content = body;
    // Build frontmatter from overrides + extra keys
    let mut updates: Vec<(&str, &str)> = Vec::new();
    // Add override keys
    if let Some(ref a) = overrides.agent { updates.push(("agent", a)); }
    if let Some(ref m) = overrides.model { updates.push(("model", m)); }
    if let Some(n) = overrides.max_iterations { /* format and push */ }
    if let Some(s) = overrides.sleep_secs { /* format and push */ }
    if let Some(d) = overrides.dangerous { /* format and push */ }
    // Add extra keys preserved from original
    for (k, v) in &self.editor_fm_extra {
        updates.push((k, v));
    }
    // Use update_frontmatter_content to build the final content
    status_parser::update_frontmatter_content(&full_content, &updates)
} else {
    body
};
```

Alternatively, a simpler approach: build the frontmatter string manually by combining `overrides.to_frontmatter()` lines with the extra keys. The key insight is that `to_frontmatter()` already produces `---\nkey: val\n...\n---\n`, so we need to either:
- (a) Use `update_frontmatter_content` on the body to add all keys at once, OR
- (b) Extend `to_frontmatter()` to accept extra keys

Option (a) is cleaner since `update_frontmatter_content` already handles the formatting.

### 4. Clear in `close_editor`

In `close_editor` (line ~393), add:
```rust
self.editor_fm_extra = Vec::new();
```

## Test to add in `status_parser.rs`

Add a roundtrip test:

**`test_editor_roundtrip_preserves_metadata`** — Simulate the editor flow:
1. Start with content that has both config keys and run metadata keys
2. Call `extract_extra_frontmatter_keys` to get extras
3. Call `parse_plan_overrides_content` to get overrides
4. Reconstruct using `update_frontmatter_content` with overrides + extras
5. Verify all keys (both config and metadata) survive

## Verification

- `cargo build` compiles
- `cargo test -p okena -- kruh` passes
- Manual: start a loop, let it write metadata to an issue, open the issue in the editor, save without changes, verify metadata keys are still present in the file
