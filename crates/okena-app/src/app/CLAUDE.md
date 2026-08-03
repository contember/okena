# app/ — Main Application Entity

The `Okena` entity coordinates desktop-only GPUI entities and synchronizes them with the daemon-backed workspace.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | `Okena` struct — owns top-level desktop entities and daemon connection state. |
| `detached_terminals.rs` | Opens separate OS windows for detached terminals. |
| `detached_overlays.rs` | Opens detached overlay windows. |
| `extras.rs` | Auxiliary `Okena` methods for desktop actions. |
| `notifications.rs` | Desktop notification integration. |

## Key Patterns

- **Daemon authority**: Persistent workspace, settings, PTYs, services, and git state are daemon-owned. Do not add local desktop write paths for them.
- **Client presentation**: Window placement and other presentation-only state stay in the desktop process and `window-layout.json`.
