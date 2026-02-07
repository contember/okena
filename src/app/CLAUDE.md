# app/ — Main Application Entity

The `Okena` entity is the central coordinator that owns the top-level GPUI entities (RootView, Workspace, RequestBroker, PtyManager) and routes events between them.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | `Okena` struct — owns RootView, Workspace, RequestBroker, PtyManager. Runs the PTY event loop (batched processing via `async_channel`). Sets up workspace auto-save observer. |
| `detached_terminals.rs` | Opens separate OS windows for detached terminals. |
| `remote_commands.rs` | Bridge from remote server to GPUI thread — handles `RemoteCommand` variants (GetState, SendText, RunCommand, ResizeTerminal, etc.) by dispatching into Workspace/PtyManager. |
| `update_checker.rs` | Background update check loop: 30s initial delay, 24h cycle, `CancelToken` for clean shutdown. |

## Key Patterns

- **Batched PTY processing**: The PTY event loop reads all available events from the channel before notifying, to avoid per-byte UI updates.
- **`data_version` skip-save**: Workspace observer compares `data_version` to avoid saving when only transient state changed.
- **Remote bridge**: Remote commands arrive via `async_channel`, execute on the GPUI thread, and reply via `oneshot` channel.
