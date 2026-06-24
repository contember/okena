# okena-terminal — Terminal Emulation & PTY Management

Wraps `alacritty_terminal` for ANSI processing and `portable-pty` for cross-platform PTY handling.

## Files

| File | Purpose |
|------|---------|
| `terminal.rs` | `Terminal` struct wrapping `alacritty_terminal::Term`. `Arc<Mutex>` for thread safety. Selection, search, scrollback, resize, URL detection. |
| `pty_manager.rs` | `PtyManager` — PTY lifecycle. `PtyHandle` per terminal. Spawns OS reader/writer threads. `PtyOutputSink` trait for broadcasting. |
| `shell_config.rs` | `ShellType` enum, `CommandBuilder` construction. Cross-platform shell detection (bash/zsh/fish/sh on Unix; cmd/PowerShell/WSL on Windows). |
| `session_backend.rs` | `SessionBackend` enum — tmux/screen/dtach on Unix; psmux on Windows; per-distro tmux/dtach/screen inside WSL. |
| `input.rs` | Key-to-bytes conversion. DECCKM cursor mode handling. Platform-specific modifier mappings. |
| `backend.rs` | Terminal backend abstraction. |
| `process.rs` | Process spawning utilities. |

## OSC sequences (sidecar)

`terminal/osc_sidecar.rs` is a side-channel VTE parser run on the same byte
stream as the main processor, for sequences alacritty ignores or answers
differently than Okena wants: `OSC 7` / `OSC 1337 CurrentDir` (cwd), `OSC 9` /
`OSC 777` / `OSC 99` (notifications), `OSC 9;4` (progress), `OSC 133` (shell
marks, via a separate prompt sidecar), and `XTVERSION`.

### `OSC 9001` — agent status (Okena private)

A **stable contract** other tools depend on (see `docs/agent-status.md`). An AI
agent reports its own lifecycle by writing to its terminal:

```
ESC ] 9001 ; st=<working|blocked|done|idle|clear> [ ; msg=<b64> ] [ ; lbl=<b64-json> ] ST
```

- `9001` is a private OSC number (not a standard sequence); keep it stable.
- `msg`/`lbl` values are base64(UTF-8) so they stay `;`/`ST`-safe (the VTE parser
  splits OSC params on `;`).
- Unknown/missing `st` leaves the current status untouched; `clear` removes it.
- Parsed into the canonical `okena_core::agent_status::AgentStatus`, stored on
  `Terminal.agent_status` (runtime-only), read via `Terminal::agent_status()`.
  `msg`/`lbl` are clamped on parse (`AgentStatus::new_clamped`) so a hostile pane
  can't pin unbounded memory (custom ≤ a few KB, labels bounded) — mirrors the
  OSC 99 caps.
- `lbl=` reserves three keys — `agent`, `session_id`, `transcript_path`. With an
  `agent` id + a UUID-shaped `session_id`, they're captured into a **sticky**
  `okena_core::agent_session::AgentSession` on `Terminal.agent_session` (read via
  `Terminal::agent_session()`). Unlike `agent_status` it survives `st=clear`
  (it's the pane's session identity for resume + transcript stats, persisted by
  the app layer), and a change sets the `agent_session_dirty` edge (drained via
  `take_agent_session_dirty`). Per-harness resume/transcript logic is dispatched
  by `agent` id through the gpui-free `okena_core::agent_harness` registry
  (impls live in the `okena-ext-*` crates). A non-UUID `session_id` is dropped.
- A change stores into the shared one-shot `remote_dirty` edge (drained via
  `take_remote_dirty`), which the PTY event loop consumes
  (`Okena::process_remote_dirty`) to bump the remote `state_version`. This edge
  is **generic**, not agent-specific: any runtime-only signal that remote
  clients should see reuses it rather than adding its own changed-edge +
  per-feature drain. A transition into `blocked`/`done` also queues a
  `TerminalNotification` (reusing the OSC 9 notification path + focus
  suppression).
- `pty_manager.rs` exports `OKENA_TTY` (the pane's slave pty path, from
  `ptsname` on the master fd) into the pane env so a process **without a
  controlling terminal** — e.g. a Claude Code hook — can emit this OSC by
  writing to `$OKENA_TTY` (writing to the slave reaches Okena's master reader
  even through a nested dtach/tmux pty). `/dev/tty` only works for interactive
  processes, not hooks. Captured at first spawn via `cmd.env`, so it goes
  **stale on reattach** to a persistent dtach/tmux session (not yet refreshed
  per-attach — known follow-up).

## Threading Model

Three execution contexts access `Terminal`:

1. **GPUI thread** — the main UI thread. Runs `process_output` (via the batched PTY event loop in `Okena`), rendering (`with_content`), user input, resize, selection, scroll, and idle-detection reads. This is where the vast majority of field access happens.
2. **Tokio reader task** (remote connections only) — calls `enqueue_output` to buffer data without holding `term.lock()`. Touches only `pending_output`, `dirty`, and `last_output_time`.
3. **Resize debounce timer** — a short-lived `std::thread::spawn` that flushes a trailing-edge resize. Touches only `resize_state` and `transport`.

The PTY reader OS thread does **not** touch `Terminal` directly — it sends `PtyEvent::Data` through an `async_channel` to the GPUI thread, which calls `process_output`.

### Synchronization primitives

- **`Arc<Mutex<T>>`** — the `Arc` is needed when the value is shared with a sub-struct (`ZedEventListener`, `OscSidecar`) or handed to a background thread (`resize_state`). A few fields (`term`, `last_output_time`, `last_viewed_time`) have a historical `Arc` that is never cloned.
- **`Mutex<T>`** — interior mutability for `&self` methods. All `Mutex`-only fields are currently GPUI-thread-only; the mutex is for interior mutability, not cross-thread safety.
- **`AtomicBool` / `AtomicU64`** — lock-free signaling: `dirty` (cross-thread with tokio reader), `content_generation` / `waiting_for_input` / `had_user_input` (avoid mutex overhead in the render hot path).

See the doc comments on `pub struct Terminal` in `terminal.rs` for per-field thread-ownership documentation.

## Key Patterns

- **`TerminalsRegistry`**: `Arc<Mutex<HashMap<String, Arc<Terminal>>>>` — shared registry for PTY event routing.
- **Batched PTY processing**: The PTY reader thread sends `PtyEvent::Data` via `async_channel`. The GPUI thread drains all pending events before notifying, avoiding per-byte UI updates.
- **Remote output decoupling**: Remote tokio reader calls `enqueue_output` (just appends to `pending_output` + sets `dirty`). The GPUI thread drains via `drain_pending_output` inside `with_content`, so `term.lock()` is never held on the tokio thread.
- **Shell detection**: Auto-detects available shells on the system. On Windows, detects WSL distros and converts paths (`C:\` → `/mnt/c/`).
