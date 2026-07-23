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
