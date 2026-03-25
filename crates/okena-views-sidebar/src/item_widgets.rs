//! Shared widget helpers for sidebar project/folder/terminal items.
//!
//! Each helper returns a partially-built element that the caller can chain
//! additional handlers onto (e.g. `.on_click()`).

use okena_core::theme::ThemeColors;
use okena_ui::rename_state::{rename_input, RenameState};
use okena_ui::simple_input::SimpleInput;
use okena_ui::tokens::{ui_text_xs, ui_text_sm, ui_text_md};
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;
use okena_ui::icon_button::icon_button;

/// Expand/collapse arrow (chevron-down/right, 16x16).
///
/// Caller chains `.on_click()` to toggle.
pub fn sidebar_expand_arrow(
    id: impl Into<ElementId>,
    is_expanded: bool,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .w(px(12.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(if is_expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                })
                .size(px(12.0))
                .text_color(rgb(t.text_secondary)),
        )
}

/// Color indicator container (16x16, cursor_pointer, hover opacity).
///
/// `child` is the inner element -- either a colored dot or a folder SVG.
/// Caller chains `.on_click()` to show color picker.
pub fn sidebar_color_indicator(
    id: impl Into<ElementId>,
    child: impl IntoElement,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .w(px(14.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.opacity(0.7))
        .child(child)
}

/// Rename input container with SimpleInput.
///
/// Returns `Some(element)` if renaming is active, `None` otherwise.
/// Caller chains `.on_action(Cancel)` / `.on_key_down(Enter)`.
pub fn sidebar_rename_input<T: 'static + Clone>(
    id: impl Into<ElementId>,
    rename_state: &Option<RenameState<T>>,
    t: &ThemeColors,
    cx: &App,
) -> Option<Stateful<Div>> {
    let input = rename_input(rename_state)?;
    Some(
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .bg(rgb(t.bg_hover))
            .rounded(px(2.0))
            .child(SimpleInput::new(input).text_size(ui_text_md(cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(|_, _window, cx| {
                cx.stop_propagation();
            }),
    )
}

/// Name label with ellipsis and standard text styling.
///
/// Caller chains `.on_click()` for select / double-click rename.
pub fn sidebar_name_label(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    t: &ThemeColors,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(ui_text_md(cx))
        .text_color(rgb(t.text_primary))
        .text_ellipsis()
        .child(name.into())
}

/// Collapsible group header (e.g. "Terminals (3)" or "Services (2)").
///
/// Returns a `Stateful<Div>` so the caller can chain `.on_click()` to toggle collapse.
pub fn sidebar_group_header(
    id: impl Into<ElementId>,
    label: &str,
    count: usize,
    is_collapsed: bool,
    is_cursor: bool,
    left_padding: f32,
    t: &ThemeColors,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(20.0))
        .pl(px(left_padding))
        .pr(px(8.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .when(is_cursor, |d: Stateful<Div>| d.border_l_2().border_color(rgb(t.border_active)))
        .child(
            // Expand/collapse chevron (smaller than project arrow)
            svg()
                .path(if is_collapsed {
                    "icons/chevron-right.svg"
                } else {
                    "icons/chevron-down.svg"
                })
                .size(px(10.0))
                .text_color(rgb(t.text_muted))
                .flex_shrink_0(),
        )
        .child(
            // Group label
            div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(label.to_string()),
        )
        .child(
            // Item count badge
            div()
                .flex_shrink_0()
                .px(px(3.0))
                .py(px(0.0))
                .rounded(px(3.0))
                .bg(rgb(t.bg_secondary))
                .text_size(ui_text_xs(cx))
                .text_color(rgb(t.text_muted))
                .child(format!("{}", count)),
        )
}

/// Idle dot badge (6x6 circle in border_idle color).
///
/// Used to indicate terminals waiting for input when a project/folder is collapsed.
pub fn sidebar_idle_dot(t: &ThemeColors) -> Div {
    div()
        .flex_shrink_0()
        .w(px(6.0))
        .h(px(6.0))
        .rounded(px(3.0))
        .bg(rgb(t.border_idle))
}

/// Worktree count badge (git-branch icon + number).
/// Shown on parent projects that have active worktrees.
pub fn sidebar_worktree_badge(count: usize, t: &ThemeColors, cx: &App) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(
            svg()
                .path("icons/git-branch.svg")
                .size(px(10.0))
                .text_color(rgb(t.text_muted)),
        )
        .child(
            div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(format!("{}", count)),
        )
}

/// Terminal count badge for hidden/inactive projects.
/// Shown inline after the project name as a small highlighted badge.
pub fn sidebar_terminal_count_badge(count: usize, t: &ThemeColors, cx: &App) -> Div {
    div()
        .flex_shrink_0()
        .ml(px(4.0))
        .px(px(4.0))
        .rounded(px(3.0))
        .bg(rgb(t.bg_header))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_primary))
        .child(format!("{}", count))
}

/// Visibility toggle button with hover-reveal behavior.
///
/// Shows the eye icon on hover only.
///
/// Caller chains `.on_click()` to handle the toggle action.
pub fn sidebar_visibility_button(
    id: impl Into<ElementId>,
    show_in_overview: bool,
    group_name: &'static str,
    tooltip_text: &'static str,
    t: &ThemeColors,
) -> Stateful<Div> {
    icon_button(id, "icons/eye.svg", t)
        .opacity(0.0)
        .when(show_in_overview, |d| d.opacity(1.0))
        .group_hover(group_name, |s| s.opacity(1.0))
        .tooltip(move |_window, cx| Tooltip::new(tooltip_text).build(_window, cx))
}

/// Project/worktree name with optional terminal count badge.
///
/// When `hide_badge` is false and the project has terminals, renders the name
/// alongside a count badge. Otherwise returns the name_label as-is.
pub fn sidebar_name_or_badge(
    name_label: Stateful<Div>,
    name: &str,
    hide_badge: bool,
    terminal_count: usize,
    t: &ThemeColors,
    cx: &App,
) -> AnyElement {
    if !hide_badge && terminal_count > 0 {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_primary))
                    .child(name.to_string()),
            )
            .child(sidebar_terminal_count_badge(terminal_count, t, cx))
            .into_any_element()
    } else {
        name_label.into_any_element()
    }
}

/// Empty spacer matching expand arrow dimensions (12x16).
pub fn sidebar_expand_spacer() -> Div {
    div().flex_shrink_0().w(px(12.0)).h(px(16.0))
}
