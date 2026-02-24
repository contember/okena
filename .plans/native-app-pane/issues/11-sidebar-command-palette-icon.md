# Issue 11: Sidebar app items, command palette, and kruh icon

**Priority:** medium
**Files:** `src/views/panels/sidebar/mod.rs`, `src/views/panels/sidebar/project_list.rs`, `src/views/overlays/command_palette.rs`, `assets/icons/kruh.svg` (new)

## Description

Add UI entry points for creating and managing kruh app panes: sidebar items, command palette command, and the kruh icon asset.

## New file: `assets/icons/kruh.svg`

Create a simple loop/cycle SVG icon for the kruh app. Should match the style of existing icons in `assets/icons/` (monochrome, 16x16 or 24x24 viewBox, `currentColor` fill).

Suggested design: two arrows forming a circular loop (representing the iteration cycle).

```svg
<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <path d="M21 12a9 9 0 1 1-6.22-8.56" />
  <path d="M21 3v9h-9" />
</svg>
```

(This is a "refresh-cw" / loop style icon. Adjust as needed to match the existing icon set.)

## Changes to `src/views/panels/sidebar/mod.rs`

### Add `SidebarCursorItem::App` variant

Find the `SidebarCursorItem` enum and add:

```rust
App { project_id: String, app_id: String },
```

### Update cursor navigation

The sidebar's keyboard cursor navigation iterates through items. App items should be included alongside terminal items when a project is expanded. Update the item collection logic to include app entries after terminal entries for each project.

### Handle app item clicks

When an App cursor item is clicked/selected:
- Focus the app pane via the focus manager
- Dispatch appropriate focus action

## Changes to `src/views/panels/sidebar/project_list.rs`

### Add `render_app_item()` method

Mirrors `render_terminal_item()`:

```rust
fn render_app_item(
    &self,
    project_id: &str,
    app_id: &str,
    app_kind: &AppKind,
    is_focused: bool,
    cx: &mut Context<Sidebar>,
) -> impl IntoElement {
    let t = theme(cx);
    let icon = match app_kind {
        AppKind::Kruh => "icons/kruh.svg",
    };
    let name = match app_kind {
        AppKind::Kruh => "Kruh",
    };

    div()
        .flex()
        .items_center()
        .px_2()
        .py(px(2.0))
        .gap_1()
        .rounded_sm()
        .when(is_focused, |el| el.bg(t.selection))
        .cursor_pointer()
        .child(
            svg().path(icon).size(px(14.0)).text_color(t.muted_foreground)
        )
        .child(
            div().text_xs().child(name)
        )
        .on_mouse_down(MouseButton::Left, /* click handler to focus app */)
}
```

### Interleave app items with terminal items

In the project expansion rendering, after listing terminal items, also list app items. Use `layout.collect_app_ids()` to get app IDs and render each with `render_app_item()`.

To get the `AppKind` for each app, traverse the layout tree to find the App node matching each app_id.

## Changes to `src/views/overlays/command_palette.rs`

### Add "New Kruh App" command

In the command entries list, add a new command:

```rust
CommandEntry {
    label: "New Kruh App".to_string(),
    description: Some("Create a new Kruh automation loop pane".to_string()),
    // ... other fields per the CommandEntry struct
}
```

When this command is executed:
1. Get the currently focused project ID
2. Dispatch `ActionRequest::CreateApp { project_id, app_kind: "kruh".to_string(), app_config: serde_json::Value::Null }`
3. Close the command palette

Look at how existing commands like "New Terminal" are implemented and follow the same pattern.

### Register the command

Commands are likely registered in a list or returned from a function. Add the Kruh command alongside existing commands.

## Acceptance Criteria

- `kruh.svg` icon renders correctly in the UI
- Sidebar shows app items under expanded projects
- Clicking app items in sidebar focuses the app pane
- "New Kruh App" command appears in command palette
- Executing the command creates a new Kruh app pane in the focused project
- `cargo build` succeeds
