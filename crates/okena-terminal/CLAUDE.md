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
ESC ] 9001 ; st=<working|blocked|done|idle|clear> [ ; tid=<terminal-id> ] [ ; msg=<b64> ] [ ; lbl=<b64-json> ] ST
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
  `Terminal::agent_session()`). Unlike `agent_status` it survives `st=clear` —
  and is captured on it, since a harness maps session start/end onto `clear`.
  It's the pane's session identity for resume + transcript stats, persisted by
  the app layer, and a change sets the `agent_session_dirty` edge (drained via
  `take_agent_session_dirty`). Per-harness resume/transcript logic is dispatched
  by `agent` id through the gpui-free `okena_core::agent_harness` registry
  (impls live in `okena-agent-harnesses` — deliberately NOT the `okena-ext-*`
  crates, which pull gpui and so cannot be linked by the headless daemon). A
  non-UUID `session_id` is dropped, and `agent` / `transcript_path` are bounded
  (see `agent_session.rs`) because they are the only agent-status fields that
  reach disk. The three reserved keys are **stripped** from the display labels
  after capture, so session identity never rides the wire to remote clients.
- A change stores into the shared one-shot `remote_dirty` edge (drained via
  `take_remote_dirty`), which the PTY event loop consumes
  (`okena_daemon_core::pty_loop::drain_remote_dirty` — the daemon owns this
  drain; the GUI mirror does not) to bump the remote `state_version`. This edge
  is **generic**, not agent-specific: any runtime-only signal that remote
  clients should see reuses it rather than adding its own changed-edge +
  per-feature drain. A transition into `blocked`/`done` also queues a
  `TerminalNotification` (reusing the OSC 9 notification path + focus
  suppression).
- `pty_manager.rs` exports two env vars into the pane at spawn. Both go through
  `launch_environment`, **not** `cmd.env()` after the fact: under a session
  backend the spawned command is `sh -c "tmux new-session …"`, so a late
  `cmd.env` lands on tmux rather than the pane's shell — and an already-running
  tmux server's environment predates Okena entirely. `launch_environment` is
  what gets rendered as tmux `-e KEY=VAL`.
  - `OKENA_TTY` — the pane's slave pty path (portable-pty's `tty_name`, i.e.
    reentrant `ttyname_r`; **not** `libc::ptsname`, whose static buffer races
    concurrent terminal creation). Senders should **prefer** it over
    `/dev/tty`: writing to the slave reaches Okena's own master reader, whereas
    a hook's controlling terminal under tmux/screen is the *nested* pty, and
    those forward only a fixed allowlist of OSC numbers that 9001 is not on.
    It also covers processes with no controlling terminal at all. It is captured
    once at spawn, so it can go stale across a reattach — which is what `tid=`
    below contains.

    **Security note:** unlike an inherited fd, a *path* can be reopened, and
    Linux recycles `/dev/pts/N` numbers. Any process that outlives its pane (a
    daemonized dev server, a forking postinstall) keeps a way to write into
    whatever pane later inherits that number, and writes to a slave surface as
    terminal *output* — i.e. they are parsed as escape sequences. Same-user, so
    not a privilege boundary, but treat the var as a capability: `tid=` guards
    OSC 9001 specifically, nothing else.
  - `OKENA_TERMINAL_ID` — the pane's terminal id, echoed back as the OSC's
    `tid=`. The sidecar drops a status whose `tid` isn't its own, which is what
    contains the stale-`OKENA_TTY` failure mode above. Note the namespaces
    differ: this is the daemon's **raw** id, while a client-side `Terminal` is
    keyed `remote:{connection}:{raw}` — the sidecar matches the unprefixed
    suffix. Omitted `tid` is accepted.

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
- **Remote output decoupling**: Remote tokio readers call `enqueue_output` (append to `pending_output` + set `dirty`) and ring the manager's capacity-1 activity doorbell. The GPUI-thread activity pump drains/parses output, then emits targeted pane/sidebar notifications; `with_content` remains the fallback drain. Never restore per-pane polling or hold `term.lock()` on the tokio thread.
- **Persistent dtach teardown**: `SIGTERM` to the dtach master does not propagate to its PTY child tree. Teardown must keep the socket discoverable, revalidate PID birth markers, quiesce/reap descendants before the master, and unlink only after socket death is verified.
- **Shell detection**: Auto-detects available shells on the system. On Windows, detects WSL distros and converts paths (`C:\` → `/mnt/c/`).
