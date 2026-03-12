# Issue 02: Write pre-run and post-run metadata in loop_runner.rs

**Priority:** high
**Files:** `src/views/layout/kruh_pane/loop_runner.rs`, `src/views/layout/kruh_pane/status_parser.rs`

## Description

Write run metadata (`startedAt`, `agent`, `model`, `iteration`, `endedAt`, `exitCode`, `duration`) into the current issue file's YAML frontmatter before and after each agent iteration. This uses the `update_issue_frontmatter` and `iso_now` functions from issue 01.

## Pre-run metadata write

**Location:** After iteration increment (line ~204, after `l.iteration += 1`), before git snapshot (line ~208).

Add a block that:
1. Reads `first_pending_number` (already available from step 4 of the loop)
2. Finds the issue file path via `find_issue_file_path(&effective_config.docs_dir, &issue_number)`
3. Writes these keys to frontmatter:
   - `startedAt`: value from `iso_now()`
   - `agent`: value from `effective_config.agent`
   - `model`: value from `effective_config.model`
   - `iteration`: value from the current iteration number (get from loop state)
4. Wraps the I/O in `smol::unblock` (following existing pattern for filesystem operations in this file)
5. Logs a warning on failure but does not break the loop

```rust
// Write pre-run metadata to issue frontmatter
let issue_number_for_meta = first_pending_number.clone().unwrap_or_default();
if !issue_number_for_meta.is_empty() {
    let docs_dir_meta = effective_config.docs_dir.clone();
    let agent_name = effective_config.agent.clone();
    let model_name = effective_config.model.clone();
    let iteration_val = /* get current iteration from loop state */;
    let _ = smol::unblock(move || {
        if let Some(path) = find_issue_file_path(&docs_dir_meta, &issue_number_for_meta) {
            let _ = update_issue_frontmatter(&path, &[
                ("startedAt", &iso_now()),
                ("agent", &agent_name),
                ("model", &model_name),
                ("iteration", &iteration_val.to_string()),
            ]);
        }
    }).await;
}
```

## Post-run metadata write

**Location:** After exit code handling (line ~377, after `cx.notify()`), before the `is_done` check (line ~379).

Add a block that:
1. Uses the same `first_pending_number` from earlier in the iteration
2. Writes these keys to frontmatter:
   - `endedAt`: value from `iso_now()`
   - `exitCode`: value from `exit_code`
   - `duration`: computed from `iteration_start_time.elapsed()`, formatted as `"{secs}s"` (e.g. `"267s"`)
3. Same `smol::unblock` pattern

```rust
// Write post-run metadata to issue frontmatter
let issue_number_for_post = first_pending_number.clone().unwrap_or_default();
if !issue_number_for_post.is_empty() {
    let docs_dir_post = effective_config.docs_dir.clone();
    let exit_code_str = exit_code.to_string();
    let duration_str = /* compute from iteration_start_time */;
    let _ = smol::unblock(move || {
        if let Some(path) = find_issue_file_path(&docs_dir_post, &issue_number_for_post) {
            let _ = update_issue_frontmatter(&path, &[
                ("endedAt", &iso_now()),
                ("exitCode", &exit_code_str),
                ("duration", &duration_str),
            ]);
        }
    }).await;
}
```

## Getting iteration_start_time for duration

The `iteration_start_time` is set in step 7 (line ~200): `l.iteration_start_time = Some(std::time::Instant::now())`. To compute duration at the post-run point, read it from the loop state:

```rust
let duration_secs = this.update(cx, |pane, _| {
    pane.loop_ref(loop_id)
        .and_then(|l| l.iteration_start_time.map(|t| t.elapsed().as_secs()))
        .unwrap_or(0)
}).unwrap_or(0);
let duration_str = format!("{}s", duration_secs);
```

## Import additions

Add to the existing import from `super::status_parser`:
- `find_issue_file_path` (already imported? check — it's used in status_parser.rs but may not be imported in loop_runner.rs)
- `update_issue_frontmatter`
- `iso_now`

## Verification

- `cargo build` compiles
- `cargo test -p okena -- kruh` passes
- Manual: run a loop on a plan with issues, check that the issue file gets `startedAt`, `agent`, `model`, `iteration`, `endedAt`, `exitCode`, `duration` in its frontmatter after the iteration
