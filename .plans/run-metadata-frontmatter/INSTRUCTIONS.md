# Run Metadata Frontmatter

Write run metadata (`startedAt`, `endedAt`, `exitCode`, `duration`, `iteration`, `agent`, `model`) into issue file YAML frontmatter before and after each agent iteration in the Kruh loop runner.

## Goals

1. Persist run info so it can be displayed to users and serves as a basic log
2. Generic frontmatter update functions that can be reused for other metadata
3. Preserve run metadata through the editor round-trip (open issue → edit → save)

## Architecture

- `status_parser.rs` — Pure functions for frontmatter manipulation (no GPUI dependencies)
- `loop_runner.rs` — Async I/O calls to write metadata at iteration boundaries
- `mod.rs` — Editor state management to preserve extra frontmatter keys

## Key design decisions

- Frontmatter keys are stored as ordered `Vec<(String, String)>` to preserve insertion order
- Known config keys (`agent`, `model`, `max_iterations`, `sleep_secs`, `dangerous`) are separated from "extra" keys (run metadata) so the editor can handle them independently
- Writing `agent` and `model` into the frontmatter doubles as a config override for future runs — this is intentional
- All filesystem I/O in `loop_runner.rs` uses `smol::unblock` to avoid blocking the GPUI event loop
- Failures to write metadata are logged as warnings but never break the loop

## Resulting frontmatter example

```yaml
---
model: claude-sonnet-4-6
max_iterations: 50
startedAt: 2026-02-25T14:30:45
agent: claude
iteration: 3
endedAt: 2026-02-25T14:35:12
exitCode: 0
duration: 267s
---
```

## Verification

1. `cargo build` — compiles without errors
2. `cargo test -p okena -- kruh` — all existing + new tests pass
