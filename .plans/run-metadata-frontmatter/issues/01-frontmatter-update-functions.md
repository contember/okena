# Issue 01: Add generic frontmatter update functions to status_parser.rs

**Priority:** high
**Files:** `src/views/layout/kruh_pane/status_parser.rs`

## Description

Add four new public functions to `status_parser.rs` that enable reading, writing, and filtering YAML frontmatter keys in issue markdown files. These are foundational functions consumed by the loop runner (issue 02) and editor round-trip fix (issue 03).

## Functions to implement

### `update_frontmatter_content(content: &str, updates: &[(&str, &str)]) -> String`

Pure function. Parses ALL existing frontmatter key-value pairs into an ordered `Vec<(String, String)>`, applies upserts from `updates` (overwrite if key exists, append if new), preserves body content after the closing `---`. If no frontmatter exists, creates a new `---` section. Preserves the order of existing keys; new keys are appended at the end.

Example:
```rust
let content = "---\nmodel: gpt-4\n---\n\n# Issue";
let result = update_frontmatter_content(content, &[("startedAt", "2026-02-25T14:30:45"), ("model", "opus")]);
// Result: "---\nmodel: opus\nstartedAt: 2026-02-25T14:30:45\n---\n\n# Issue"
```

### `update_issue_frontmatter(file_path: &str, updates: &[(&str, &str)]) -> std::io::Result<()>`

I/O wrapper: reads file via `std::fs::read_to_string`, calls `update_frontmatter_content`, writes back via `std::fs::write`.

### `extract_extra_frontmatter_keys(content: &str) -> Vec<(String, String)>`

Returns all frontmatter key-value pairs that are NOT known config override keys. The known config keys are: `agent`, `model`, `max_iterations`, `sleep_secs`, `dangerous`. Everything else (e.g. `startedAt`, `endedAt`, `exitCode`, `duration`, `iteration`) is returned. Uses the same frontmatter parsing logic as `parse_plan_overrides_content`.

### `iso_now() -> String`

Formats current local time as `YYYY-MM-DDTHH:MM:SS`. Use the `time` crate which is already available (same pattern as `status_bar.rs` — check how it gets local time there). Example output: `"2026-02-25T14:30:45"`.

## Tests to add

Add these tests in the existing `#[cfg(test)] mod tests` block:

1. **`test_update_frontmatter_add_to_existing`** — Content has frontmatter with `model: gpt-4`, add `startedAt` key, verify both present in output.
2. **`test_update_frontmatter_overwrite_existing`** — Content has `model: gpt-4`, update `model` to `opus`, verify changed.
3. **`test_update_frontmatter_create_new`** — Content has no frontmatter (just `# Issue\n...`), add a key, verify frontmatter section created.
4. **`test_update_frontmatter_preserve_body`** — Verify body content after `---` is preserved exactly (including blank lines).
5. **`test_update_frontmatter_multiple_updates`** — Apply 3+ updates at once, verify all present.
6. **`test_update_frontmatter_empty_content`** — Empty string input, add a key, verify valid frontmatter output.
7. **`test_extract_extra_keys_with_extras`** — Content has `model`, `startedAt`, `exitCode` — only `startedAt` and `exitCode` returned.
8. **`test_extract_extra_keys_without_extras`** — Content only has known keys (`agent`, `model`) — returns empty vec.
9. **`test_extract_extra_keys_no_frontmatter`** — No frontmatter — returns empty vec.
10. **`test_iso_now_format`** — Call `iso_now()`, verify it matches regex `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}$`.

## Implementation notes

- Parse frontmatter the same way `parse_plan_overrides_content` does: find opening `---`, find closing `\n---`, split lines on first `:`.
- The known config keys constant should be a `&[&str]` array: `["agent", "model", "max_iterations", "sleep_secs", "dangerous"]`.
- For `iso_now`, look at `src/views/panels/status_bar.rs` for the `time` crate usage pattern in this project.
