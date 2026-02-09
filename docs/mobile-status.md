# Mobile Client — Architecture & Status

## Overview

Flutter + Rust FFI mobile app (Android/iOS) for controlling a remote Okena desktop instance. Uses `alacritty_terminal` in Rust for ANSI processing — identical terminal emulation as the desktop app. Communicates with the desktop's remote server via REST + WebSocket.

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  Flutter Mobile App                                          │
│                                                              │
│  ┌──────────────────┐    ┌────────────────────────────────┐  │
│  │ Dart UI           │    │ Rust (via flutter_rust_bridge)  │  │
│  │                   │    │                                 │  │
│  │ Screens            │    │ ConnectionManager (OnceLock)    │  │
│  │ Providers          │    │  ├─ RemoteClient<Handler>       │  │
│  │ Widgets            │    │  ├─ MobileConnectionHandler     │  │
│  │                   │    │  └─ TerminalHolder per terminal │  │
│  │                   │    │     └─ alacritty_terminal::Term │  │
│  └──────────────────┘    └────────────────────────────────┘  │
│          │                          │                         │
│     flutter_rust_bridge (FFI)       │                         │
└──────────┼──────────────────────────┼─────────────────────────┘
           │           HTTP + WebSocket
           ▼                          ▼
┌──────────────────────────────────────────────────────────────┐
│  Okena Desktop (server)                                      │
│                                                              │
│  Remote Server (src/remote/)                                 │
│  ├── POST /v1/pair       → code → bearer token               │
│  ├── GET  /v1/state      → workspace snapshot (JSON)         │
│  ├── POST /v1/actions    → send_text, split, close, resize   │
│  └── WS   /v1/stream     → binary PTY frames + state events │
└──────────────────────────────────────────────────────────────┘
```

## Repository Structure

```
Cargo.toml                     ← workspace: members = [".", "mobile/native", "crates/okena-core"]
src/                           ← desktop app
crates/okena-core/             ← shared crate (API types, client state machine, theme colors)
mobile/
  android/, ios/               ← platform shells
  lib/
    main.dart                  ← App entry, MultiProvider setup, AppRouter
    src/
      models/
        saved_server.dart      ← SavedServer data class, JSON persistence
        layout_node.dart       ← Sealed classes: TerminalNode, SplitNode, TabsNode
      providers/
        connection_provider.dart  ← Saved servers CRUD, connection lifecycle, status polling
        workspace_provider.dart   ← Project list polling, focused project tracking
      screens/
        server_list_screen.dart   ← Server list + add bottom sheet
        pairing_screen.dart       ← Connect → pair flow, code input
        workspace_screen.dart     ← AppBar + drawer + layout + key toolbar
      theme/
        app_theme.dart         ← JetBrainsMono font, Catppuccin dark colors
      widgets/
        key_toolbar.dart       ← ESC, TAB, CTRL/ALT (sticky), arrows
        layout_renderer.dart   ← Recursive layout tree → TerminalView/Flex/Tabs
        project_drawer.dart    ← Project list drawer with disconnect button
        status_indicator.dart  ← Colored dot + label
        terminal_painter.dart  ← CustomPainter: bg rects → text → cursor
        terminal_view.dart     ← Terminal widget: resize, input, 30fps polling
      rust/api/                ← generated Dart bindings (do not edit)
  fonts/                       ← JetBrainsMono (Regular, Bold, Italic, BoldItalic)
  native/                      ← Rust FFI crate
    Cargo.toml
    src/
      lib.rs                   ← pub mod api; pub mod client;
      api/
        connection.rs          ← connect, pair, disconnect, connection_status
        terminal.rs            ← get_visible_cells, get_cursor, send_text, resize_terminal
        state.rs               ← get_projects, is_dirty, send_special_key, get_project_layout_json
      client/
        manager.rs             ← ConnectionManager singleton (OnceLock + tokio runtime)
        handler.rs             ← MobileConnectionHandler (impl ConnectionHandler)
        terminal_holder.rs     ← TerminalHolder (alacritty_terminal::Term wrapper)
```

## Data Flow

### PTY output (server → mobile screen)

```
Remote PTY process
  → PtyBroadcaster (server)
  → WebSocket binary frame [proto=1][type=1][stream_id:u32][data...]
  → RemoteClient WS reader task (okena-core)
  → MobileConnectionHandler.on_terminal_output()
  → TerminalHolder.process_output(data)     ← alacritty ANSI processing
  → dirty flag set
  → Flutter polls is_dirty() every 33ms → get_visible_cells() via FFI
  → TerminalPainter (CustomPainter) renders cell grid
```

### Keyboard input (mobile → server)

```
Soft keyboard / key toolbar tap
  → Dart calls FFI send_text() or send_special_key()
  → ConnectionManager.send_ws_message()
  → WsClientMessage::SendText via WebSocket
  → Server bridge → PtyManager.send_input()
  → Remote PTY stdin
```

### State sync (project list, layouts)

```
WS "state_changed" event or initial connect
  → RemoteClient fetches GET /v1/state
  → Parses StateResponse, diffs against cached state
  → Creates/removes TerminalHolders for added/removed terminals
  → Auto-subscribes to new terminal streams
  → state_cache updated in MobileConnection
  → Flutter reads via FFI get_projects()
```

## Shared Core: okena-core

The `crates/okena-core/` crate contains all code shared between desktop and mobile:

| Module | Contents |
|--------|----------|
| `api` | `StateResponse`, `ApiProject`, `ApiLayoutNode`, `ActionRequest` — serde types |
| `client` | `RemoteClient<H>` state machine, `ConnectionHandler` trait, `ConnectionEvent`, `ConnectionStatus`, `WsClientMessage`, `RemoteConnectionConfig` |
| `client::id` | `make_prefixed_id()`, `strip_prefix()`, `is_remote_terminal()` |
| `client::state` | `diff_states()`, `collect_state_terminal_ids()` |
| `theme::colors` | `ThemeColors`, `DARK_THEME`, `ansi_to_argb()` |
| `keys` | `SpecialKey` enum with `to_bytes()` |
| `ws` | Binary PTY frame format helpers |
| `types` | `SplitDirection` |

The `client` module is behind a `client` feature flag (adds tokio, reqwest, tokio-tungstenite, async-channel, futures).

Desktop uses the same `RemoteClient<H>` with `DesktopConnectionHandler` (creates `Terminal` objects in the GPUI `TerminalsRegistry`). Mobile uses `MobileConnectionHandler` (creates `TerminalHolder` objects in a shared `HashMap`).

## Key Decisions

### Flutter + Rust FFI (not React Native + xterm.js)

- Same terminal parser as desktop (alacritty_terminal) — no rendering divergence
- Shared Rust code via okena-core — real code reuse, not just type duplication
- CustomPainter for grid rendering — full control, no WebView overhead
- Higher build complexity (NDK cross-compilation) — acceptable tradeoff

### NoopEventListener on mobile

The server's `Term` already handles PtyWrite responses (cursor reports, DA sequences). If the mobile `Term` also forwarded these back via WebSocket, they'd be written to the PTY twice. So mobile uses a no-op listener.

### Terminal ID namespacing

Remote terminal IDs use the prefix `remote:{connection_id}:{terminal_id}` to avoid collisions. The ConnectionHandler receives both the raw `terminal_id` (for WS messages to the server) and the `prefixed_id` (for local storage keys).

### ConnectionManager as OnceLock singleton

Mobile doesn't have GPUI's entity system. A `static OnceLock<ConnectionManager>` with a 2-thread tokio runtime provides the async backbone. All FFI functions access it via `ConnectionManager::get()`.

### FFI ConnectionStatus simplification

The FFI `ConnectionStatus` enum collapses `Reconnecting { attempt }` into `Connecting` — mobile UI doesn't need the attempt counter.

### DARK_THEME as default palette

Cell colors use `ThemeColors::DARK_THEME` for ANSI → ARGB conversion. Theme switching can be added later by passing a `ThemeColors` reference from the Flutter side.

## Current State

### Done

| Layer | What | Status |
|-------|------|--------|
| **Shared core** | okena-core with API types, RemoteClient state machine, ThemeColors | Complete |
| **Desktop client** | `src/remote_client/` — DesktopConnectionHandler, RemoteBackend, sidebar integration | Complete |
| **Desktop server** | All endpoints: health, pair, state, actions (including resize, create_terminal), WS stream | Complete |
| **Web client** | React SPA at `/v1/web/` — connect, pair, browse projects, render terminals (xterm.js) | Complete |
| **Mobile Rust core** | ConnectionManager, MobileConnectionHandler, TerminalHolder, all FFI functions wired to real networking | Complete |
| **Mobile Flutter UI** | Full UI: ServerListScreen, PairingScreen, WorkspaceScreen, project drawer, terminal rendering, key toolbar, layout rendering | Complete |
| **Mobile state management** | ConnectionProvider (saved servers, polling), WorkspaceProvider (project list, focus tracking) | Complete |
| **Terminal rendering** | CustomPainter (3-pass: bg, text, cursor), 30fps dirty polling, auto-resize with debounce | Complete |
| **Key toolbar** | ESC, TAB, CTRL/ALT sticky toggles, arrow keys | Complete |
| **Layout rendering** | Recursive split/tab layout from JSON, portrait-mode auto-vertical, tab switching | Complete |
| **Saved servers** | SharedPreferences persistence with JSON serialization | Complete |
| **Rust tests** | 8 mobile native + 23 okena-core unit tests | Passing |
| **Dart tests** | 22 unit tests (7 SavedServer, 7 LayoutNode, 8 terminal flags/colors) | Passing |

### Not yet done (polish)

| What | Description |
|------|-------------|
| **Gestures** | Text selection, pinch-to-zoom font size, scrollback (two-finger scroll) |
| **Auto-reconnect UI** | Visual feedback banner for reconnection attempts |
| **Long-press arrows** | Key repeat on long-press for arrow keys |
| **Theme sync** | Receive theme colors from server instead of hardcoded DARK_THEME |
| **F-keys** | F1–F12 in toolbar (swipe-up row) |
| **App icon & splash** | Custom launcher icon, branded splash screen |
| **On-device testing** | End-to-end test on physical Android device with real Okena server |

## FFI Surface

### connection.rs

| Function | Sync | Description |
|----------|------|-------------|
| `init_app()` | init | Setup FRB + ConnectionManager |
| `connect(host, port) → String` | sync | Create connection, start health check, return conn_id |
| `pair(conn_id, code)` | async | Pair with code, start WS |
| `disconnect(conn_id)` | sync | Close WS, cleanup terminals |
| `connection_status(conn_id) → ConnectionStatus` | sync | Current status |

### terminal.rs

| Function | Sync | Description |
|----------|------|-------------|
| `get_visible_cells(conn_id, terminal_id) → Vec<CellData>` | sync | Grid cells with ARGB colors + flags |
| `get_cursor(conn_id, terminal_id) → CursorState` | sync | Cursor position, shape, visibility |
| `send_text(conn_id, terminal_id, text)` | async | Send text input via WS |
| `resize_terminal(conn_id, terminal_id, cols, rows)` | async | Resize local grid + send WS resize |

### state.rs

| Function | Sync | Description |
|----------|------|-------------|
| `get_projects(conn_id) → Vec<ProjectInfo>` | sync | Project list from cached state |
| `get_focused_project_id(conn_id) → Option<String>` | sync | Server's focused project |
| `is_dirty(conn_id, terminal_id) → bool` | sync | Terminal has new output |
| `send_special_key(conn_id, terminal_id, key)` | async | Send named key (Enter, CtrlC, ArrowUp, ...) |
| `get_project_layout_json(conn_id, project_id) → Option<String>` | sync | Layout tree as JSON |
| `get_all_terminal_ids(conn_id) → Vec<String>` | sync | Flat list of all terminal IDs |

## Next Steps (Polish)

### 1. Gestures & interaction

- Pinch-to-zoom font size (adjust `_fontSize` → recompute grid → `resize_terminal()`)
- Long-press arrow keys for key repeat
- Text selection + copy (long-press → drag to select → clipboard)
- Two-finger scroll for scrollback

### 2. Visual polish

- Auto-reconnect banner (visual feedback when connection drops and reconnects)
- App icon and splash screen
- Theme sync from server (receive `ThemeColors` via state, pass to `TerminalPainter`)
- F1–F12 keys in toolbar (swipe-up secondary row)

### 3. On-device testing

- End-to-end test: connect to real Okena server, pair, browse projects, type in terminal
- Verify performance (30fps rendering, resize latency)
- Test with large terminal output (build logs, `htop`)

## Networking

The remote server binds to a configurable IP (default localhost). Mobile clients reach it via:

| Method | Notes |
|--------|-------|
| **Tailscale** (recommended) | Zero-config mesh VPN, free tier, works on mobile |
| **WireGuard** | Manual peer config, low latency |
| **SSH tunnel** | `ssh -L 19100:localhost:19100 server` |
| **LAN** | `--listen 192.168.x.x` to bind to LAN IP |

## Build

```bash
# Rust only
cargo build -p okena_mobile_native
cargo test -p okena_mobile_native

# Regenerate Dart bindings
cd mobile && flutter_rust_bridge_codegen generate

# Build APK
export ANDROID_HOME=~/android-sdk
export PATH="$HOME/flutter/bin:$HOME/.cargo/bin:$PATH"
cd mobile && flutter build apk --debug
```

**Critical notes:**
- `run_build_tool.sh` needs `$HOME/.cargo/bin` in PATH (Gradle daemon doesn't inherit it)
- Use `rustls-tls` (not `native-tls`) for all deps to avoid cross-compiling OpenSSL
