---
name: duplicate-detection
description: Detect duplicated code and choose safe deduplication targets in Rust or general codebases. Use when the user asks to find code clones, dedupe a PR or branch, run duplicate-code tools, reduce repeated logic, identify dangerous duplication that may drift, or continue incremental refactors driven by duplicate-detection reports.
---

# Duplicate Detection

Use this skill to turn clone reports into small, safe refactors. Prefer removing duplication that can cause behavior drift over reducing a metric for its own sake.

## Workflow

1. Establish scope.
   - Inspect the current branch, worktree state, language mix, and test commands.
   - For PR work, determine the correct merge base before judging whether duplication is new.
   - Check whether CI already runs linting such as `cargo clippy`; clippy is useful but is not a duplicate-code detector.

2. Generate duplicate reports.
   - Use at least one structural or token-based detector.
   - Exclude generated output, vendored dependencies, build artifacts, lockfiles, snapshots, and large fixtures unless the user explicitly wants them included.
   - Save reports under `.mine/` at the repo root — it is git-ignored, so report output can never be committed, and it survives a reboot alongside the repo it describes.

3. Triage the reports before editing.
   - Prioritize logic that can drift: policy checks, wire protocol construction, persistence, filesystem/process/network behavior, lifecycle transitions, cache invalidation, notifications, and cross-entrypoint behavior.
   - Treat tests as lower priority unless duplicated setup hides product behavior, makes assertions diverge, or blocks future maintenance.
   - Ignore harmless symmetry when abstraction would obscure intent: simple DTO conversions, repeated enum arms, tiny wrappers, trivial UI layout, or generated-looking glue.

4. Refactor in small slices.
   - Extract the smallest shared helper that matches an existing module boundary.
   - Preserve public APIs, FFI boundaries, serialized formats, and observable error text unless the user asked to change them.
   - Avoid macro layers, broad architecture moves, or generic frameworks unless the payoff is obvious; ask first when unsure.
   - Commit each coherent refactor separately when the user asks for incremental commits.

5. Verify and report.
   - Run targeted tests or checks for touched code; run broader checks when shared behavior moved.
   - Re-run the duplicate detector after meaningful refactors and report before/after counts.
   - Explain which duplicate classes remain and why they are lower priority.

## Rust Commands

Use `cargo-dupes` for Rust-focused clone reports when available:

```bash
cargo dupes --path . \
  --exclude target \
  --exclude node_modules \
  --exclude-tests \
  --min-lines 8 \
  --min-nodes 20 \
  --threshold 0.85 \
  --format json report > .mine/duplicate-detection-cargo-dupes.json
```

If it is missing and installing tools is acceptable in the environment:

```bash
cargo install cargo-dupes --locked
```

`cargo-dupes` may emit multiple JSON values. Read the first report object with `jq -s`:

```bash
jq -s '.[0] | {
  exact_groups: .exact_duplicate_groups,
  exact_lines: .exact_duplicate_lines,
  near_groups: .near_duplicate_groups,
  near_lines: .near_duplicate_lines
}' .mine/duplicate-detection-cargo-dupes.json
```

Run normal Rust quality gates for changed behavior:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Adjust those commands to match the repo's documented CI.

## General Commands

Use `jscpd` for broad token/text clone detection across mixed-language repos:

```bash
npx --yes jscpd@5 . \
  --min-lines 8 \
  --min-tokens 80 \
  --ignore "**/target/**,**/.git/**,**/node_modules/**,**/dist/**,**/build/**,**/coverage/**,**/Cargo.lock,**/package-lock.json,**/pnpm-lock.yaml,**/yarn.lock" \
  --reporters json,silent \
  --output .mine/duplicate-detection-jscpd
```

When the repo is mostly Rust, add `--format rust` to reduce noise. For mixed repos, let `jscpd` auto-detect formats or pass a comma-separated format list.

Summarize the JSON report:

```bash
jq '.statistics.total | {
  clones: .clones,
  duplicated_lines: .duplicatedLines,
  duplicated_percentage: .percentage
}' .mine/duplicate-detection-jscpd/jscpd-report.json
```

## Review Heuristics

Ask these questions for each candidate:

- Would a future change likely need to update both places?
- Is one copy already subtly different in a way that looks accidental?
- Does the duplication cross a boundary where behavior should stay aligned, such as local versus remote, daemon versus client, or API versus UI?
- Can the shared helper be named after a real domain concept rather than after the duplicated syntax?
- Will the refactor reduce branching and tests, or just hide two readable blocks behind indirection?

Proceed when the answers point to drift risk and the extraction is local. Leave the duplication alone when a shared abstraction would be more fragile than the repeated code.
