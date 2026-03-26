# app/ — Main Application Entity

The `Okena` entity is the central coordinator that owns the top-level GPUI entities (RootView, Workspace, RequestBroker, PtyManager) and routes events between them.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | `Okena` struct — owns all top-level entities. Runs the PTY event loop (batched `async_channel` processing). Sets up workspace auto-save observer. |
| `detached_terminals.rs` | Opens separate OS windows for detached terminals. |
| `headless.rs` | Headless mode (no GUI). |
| `remote_commands.rs` | Bridge from remote server to GPUI thread — handles `RemoteCommand` variants by dispatching into Workspace/PtyManager. |

## Key Patterns

- **Batched PTY processing**: The PTY event loop reads all available events from the channel before notifying, to avoid per-byte UI updates.
- **`data_version` skip-save**: Workspace observer compares `data_version` to avoid saving when only transient state changed.
- **Remote bridge**: Remote commands arrive via `async_channel`, execute on the GPUI thread, and reply via `oneshot` channel.
