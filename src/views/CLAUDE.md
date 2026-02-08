# views/ — UI Views & Components

All GPUI views: the main window, layout system, sidebar, overlays, and reusable components.

## View Hierarchy

```
RootView (root/)
├── TitleBar (chrome/title_bar.rs)
├── Sidebar (panels/sidebar/) — project/folder list, drag-and-drop
├── ProjectColumn (panels/project_column.rs)
│   └── LayoutContainer (layout/layout_container.rs)
│       ├── TerminalPane (layout/terminal_pane/) — 11 files
│       ├── SplitPane (layout/split_pane.rs)
│       └── Tabs (layout/tabs/) — 3 files
├── StatusBar (panels/status_bar.rs)
└── Overlays (overlays/) — managed by OverlayManager
    ├── CommandPalette, ProjectSwitcher, FileSearch
    ├── SettingsPanel (11 files), SessionManager (3 files)
    ├── DiffViewer (7 files), FileViewer (4 files)
    ├── MarkdownRenderer (4 files)
    ├── AddProjectDialog, WorktreeDialog, ThemeSelector
    ├── KeybindingsHelp, ShellSelectorOverlay
    ├── ContextMenu, FolderContextMenu
    └── DetachedTerminal
```

## Files

| File/Dir | Purpose |
|----------|---------|
| `mod.rs` | Module re-exports. |
| `overlay_manager.rs` | `OverlayManager` entity — centralized modal overlay lifecycle. `OverlaySlot<T>` generic wrapper. `CloseEvent` trait for cleanup. |
| `sidebar_controller.rs` | `SidebarController` — sidebar animation state, auto-hide behavior. |

### root/ — Main Window View

| File | Purpose |
|------|---------|
| `mod.rs` | `RootView` entity — owns SidebarController, OverlayManager, TitleBar. Subscribes to RequestBroker. |
| `render.rs` | Top-level `Render` impl — assembles the full window layout. |
| `handlers.rs` | Action handlers (keybinding dispatch, overlay toggles). |
| `sidebar.rs` | Sidebar rendering and interaction within RootView. |
| `terminal_actions.rs` | Terminal-scoped actions (copy, paste, search, zoom, split). |

### chrome/ — Window Chrome

| File | Purpose |
|------|---------|
| `mod.rs` | Re-exports. |
| `title_bar.rs` | Custom title bar (Windows CSD, macOS traffic lights). |
| `header_buttons.rs` | Window control buttons. |

### panels/ — Side Panels

| File | Purpose |
|------|---------|
| `mod.rs` | Re-exports. |
| `project_column.rs` | `ProjectColumn` — wraps LayoutContainer for active project. |
| `status_bar.rs` | Bottom status bar (branch, terminal info). |
| `sidebar/mod.rs` | `Sidebar` entity — project list with drag-and-drop, folder expansion. |
| `sidebar/project_list.rs` | Project item rendering and interaction. |
| `sidebar/folder_list.rs` | Folder item rendering, expand/collapse. |
| `sidebar/color_picker.rs` | Folder color picker (backdrop overlay + absolute positioned panel). |
| `sidebar/item_widgets.rs` | Shared sidebar item widget builders. |
| `sidebar/drag.rs` | Drag-and-drop implementation for sidebar items. |

### layout/ — Terminal Layout System

| File | Purpose |
|------|---------|
| `mod.rs` | Re-exports. |
| `layout_container.rs` | `LayoutContainer` — renders recursive `LayoutNode` tree. |
| `split_pane.rs` | `SplitPane` — horizontal/vertical split with draggable divider. |
| `navigation.rs` | Directional focus navigation between panes. |
| `tabs/mod.rs` | Tabbed container view. |
| `tabs/context_menu.rs` | Tab right-click context menu. |
| `tabs/shell_selector.rs` | Shell selector in tab bar. |
| `terminal_pane/mod.rs` | `TerminalPane` — single terminal view (11 files total). |
| `terminal_pane/render.rs` | Terminal pane rendering. |
| `terminal_pane/header.rs` | Pane header bar (title, buttons). |
| `terminal_pane/content.rs` | Terminal content area. |
| `terminal_pane/actions.rs` | Pane-level actions (split, close, move). |
| `terminal_pane/navigation.rs` | In-pane navigation. |
| `terminal_pane/scrollbar.rs` | Scrollbar overlay. |
| `terminal_pane/search_bar.rs` | In-terminal search UI. |
| `terminal_pane/shell_selector.rs` | Shell selector for pane. |
| `terminal_pane/url_detector.rs` | Clickable URL detection. |
| `terminal_pane/zoom.rs` | Pane zoom/maximize. |

### overlays/ — Modal Overlays

See file listing above in view hierarchy. Key overlays:

| Overlay | Files | Purpose |
|---------|-------|---------|
| `settings_panel/` | 11 | Full settings UI with sidebar categories. |
| `diff_viewer/` | 7 | Side-by-side diff with syntax highlighting. |
| `file_viewer/` | 4 | File preview with syntax highlighting. |
| `markdown_renderer/` | 4 | Markdown rendering. |
| `session_manager/` | 3 | Workspace session management. |
| `command_palette.rs` | 1 | Fuzzy command search. |
| `project_switcher.rs` | 1 | Quick project switching. |
| `file_search.rs` | 1 | File search across project. |

### components/ — Reusable UI Components

| File | Purpose |
|------|---------|
| `mod.rs` | Re-exports. |
| `code_view.rs` | Syntax-highlighted code display. |
| `dropdown.rs` | Generic dropdown component. |
| `list_overlay.rs` | Scrollable list overlay. |
| `modal_backdrop.rs` | Modal backdrop with click-to-close. |
| `path_autocomplete.rs` | Path autocomplete input. |
| `rename_state.rs` | `RenameState<T>` — generic inline rename with blur handling. |
| `simple_input.rs` | Simple text input component. |
| `syntax.rs` | Syntax highlighting via syntect. |
| `ui_helpers.rs` | Shared UI helper functions. |

## Key Patterns

- **OverlayManager**: Centralized overlay lifecycle. Each overlay type gets an `OverlaySlot<T>`. `CloseEvent` trait for cleanup on dismiss.
- **RequestBroker → RootView**: RootView observes RequestBroker queues and opens appropriate overlays/sidebar actions.
- **Drag-and-drop**: `on_drag()` + `drag_over::<T>()` + `on_drop()` pattern in sidebar.
- **RenameState<T>**: `start_rename_with_blur` / `finish_rename` / `cancel_rename` for inline editing.
