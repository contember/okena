# Issue 06: AppPaneEntity and LayoutContainer app rendering

**Priority:** high
**Files:** `src/views/layout/app_pane.rs` (new), `src/views/layout/layout_container.rs`, `src/views/layout/mod.rs`

## Description

Create the `AppPaneEntity` enum that wraps concrete app entities, and update `LayoutContainer` to create and render app panes. This is the core rendering integration that makes apps visible in the layout.

## New file: `src/views/layout/app_pane.rs`

```rust
use gpui::*;
use crate::views::layout::kruh_pane::KruhPane;

/// Wraps concrete app entities. We use an enum rather than trait objects
/// because GPUI entities are concrete types with Entity<T>.
pub enum AppPaneEntity {
    Kruh(Entity<KruhPane>),
}

impl AppPaneEntity {
    pub fn into_any_element(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        match self {
            AppPaneEntity::Kruh(entity) => entity.update(cx, |view, cx| {
                view.render(window, cx).into_any_element()
            }),
        }
    }

    pub fn display_name(&self, cx: &App) -> String {
        match self {
            AppPaneEntity::Kruh(_) => "Kruh".to_string(),
        }
    }

    pub fn icon_path(&self) -> &str {
        match self {
            AppPaneEntity::Kruh(_) => "icons/kruh.svg",
        }
    }

    pub fn app_id(&self, cx: &App) -> Option<String> {
        match self {
            AppPaneEntity::Kruh(entity) => entity.read(cx).app_id.clone(),
        }
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            AppPaneEntity::Kruh(entity) => entity.read(cx).focus_handle.clone(),
        }
    }
}
```

Note: The exact `Render` API depends on how GPUI's render works in this codebase. Check how `TerminalPane` renders and follow the same pattern. The `into_any_element` method may need adjustment — look at how `LayoutContainer::render_terminal()` gets the element from `TerminalPane`.

## Changes to `src/views/layout/layout_container.rs`

### Add field

```rust
pub struct LayoutContainer {
    // ... existing fields ...
    app_pane: Option<AppPaneEntity>,
}
```

Initialize `app_pane: None` in the constructor.

### Add `ensure_app_pane()` method

Mirrors `ensure_terminal_pane()`:

```rust
fn ensure_app_pane(
    &mut self,
    app_id: &Option<String>,
    app_kind: &AppKind,
    app_config: &serde_json::Value,
    window: &mut Window,
    cx: &mut Context<Self>,
) {
    // Check if we already have the right app pane
    if let Some(ref existing) = self.app_pane {
        if existing.app_id(&*cx) == *app_id {
            return;
        }
    }

    // Create new app pane based on kind
    let entity = match app_kind {
        AppKind::Kruh => {
            let config: KruhConfig = app_config
                .as_object()
                .map(|_| serde_json::from_value(app_config.clone()).unwrap_or_default())
                .unwrap_or_default();
            let pane = cx.new(|cx| KruhPane::new(
                self.workspace.clone(),
                self.project_id.clone(),
                self.project_path.clone(),
                self.layout_path.clone(),
                app_id.clone(),
                config,
                window,
                cx,
            ));
            AppPaneEntity::Kruh(pane)
        }
    };

    self.app_pane = Some(entity);
}
```

### Add `render_app()` method

Mirrors `render_terminal()`. Renders the app entity inside the layout container with standalone tab bar and drop zones:

```rust
fn render_app(
    &mut self,
    app_id: &Option<String>,
    app_kind: &AppKind,
    app_config: &serde_json::Value,
    window: &mut Window,
    cx: &mut Context<Self>,
) -> impl IntoElement {
    self.ensure_app_pane(app_id, app_kind, app_config, window, cx);

    // Clear terminal pane if we had one (switching from terminal to app)
    self.terminal_pane = None;

    let app_element = self.app_pane.as_ref()
        .map(|pane| pane.into_any_element(window, &*cx));

    // Wrap in container with standalone tab bar (similar to render_terminal)
    // Include drop zones for drag-and-drop support
    div()
        .size_full()
        .flex()
        .flex_col()
        .children(app_element)
}
```

Note: Look at exactly how `render_terminal()` structures its output (tab bar, drop zones, etc.) and replicate the same pattern. The app pane should feel like a terminal pane from the layout's perspective.

### Update `Render::render()` match

Add the `App` arm:

```rust
fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let node = /* get layout node */;
    match node {
        LayoutNode::Terminal { terminal_id, minimized, detached, .. } => {
            self.render_terminal(/* ... */)
        }
        LayoutNode::Split { direction, sizes, children } => {
            self.render_split(/* ... */)
        }
        LayoutNode::Tabs { children, active_tab } => {
            self.render_tabs(/* ... */)
        }
        LayoutNode::App { app_id, app_kind, app_config } => {
            self.render_app(&app_id, &app_kind, &app_config, window, cx)
        }
    }
}
```

### Stale entity cleanup

When switching between node types, clear the stale entity:
- When rendering `App`, set `self.terminal_pane = None`
- When rendering `Terminal`, set `self.app_pane = None`

## Changes to `src/views/layout/mod.rs`

Add module declarations:

```rust
pub mod app_pane;
pub mod kruh_pane;
```

Note: `kruh_pane` module won't compile yet until the KruhPane entity is created in issue 10. You may need to create a minimal stub `kruh_pane/mod.rs` that just defines the struct signature, or defer the mod declaration to issue 10 and use a forward reference. Alternatively, implement this issue after issues 03-05, 08-10.

## Acceptance Criteria

- `AppPaneEntity` enum compiles and provides `into_any_element`, `display_name`, `icon_path`, `app_id`, `focus_handle`
- `LayoutContainer` creates appropriate app pane entity based on `AppKind`
- `render_app()` renders the app inside the layout container
- Render match handles `LayoutNode::App` variant
- Stale entities are cleaned up when switching between terminal and app
- `cargo build` succeeds (may need KruhPane stub)
