//! Menu components for context menus.

use crate::theme::ThemeColors;
use crate::tokens::*;
use gpui::prelude::FluentBuilder;
use gpui::*;

/// Menu item text size (13px) — slightly larger than TEXT_MD for readability.
const MENU_TEXT: Pixels = px(13.0);

/// Menu item icon size (15px).
const MENU_ICON: Pixels = px(15.0);

/// Context menu item with icon and label.
///
/// Returns a Stateful<Div> that can have `.on_click()` chained.
pub fn menu_item(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    menu_item_with_color(id, icon, label, t.text_primary, t.text_muted, t)
}

/// Context menu item with custom text and icon colors.
///
/// Use this for items with warning/error colors or disabled states.
pub fn menu_item_with_color(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    text_color: u32,
    icon_color: u32,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .mx(SPACE_SM)
        .px(SPACE_LG)
        .py(SPACE_SM)
        .flex()
        .items_center()
        .gap(SPACE_LG)
        .rounded(RADIUS_STD)
        .cursor_pointer()
        .text_size(MENU_TEXT)
        .text_color(rgb(text_color))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(svg().path(icon).size(MENU_ICON).text_color(rgb(icon_color)))
        .child(label.into())
}

/// Context menu item in disabled state (no hover, default cursor).
pub fn menu_item_disabled(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .mx(SPACE_SM)
        .px(SPACE_LG)
        .py(SPACE_SM)
        .flex()
        .items_center()
        .gap(SPACE_LG)
        .rounded(RADIUS_STD)
        .text_size(MENU_TEXT)
        .text_color(rgb(t.text_muted))
        .child(
            svg()
                .path(icon)
                .size(MENU_ICON)
                .text_color(rgb(t.text_muted)),
        )
        .child(label.into())
}

/// Context menu item with conditional enabled/disabled state.
///
/// When `enabled` is true: shows pointer cursor, hover effect, and primary colors.
/// When `enabled` is false: shows default cursor, no hover, and muted colors.
///
/// Returns a Stateful<Div> that can have `.on_click()` chained (caller should guard with `enabled`).
pub fn menu_item_conditional(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    enabled: bool,
    t: &ThemeColors,
) -> Stateful<Div> {
    let (text_color, icon_color) = if enabled {
        (t.text_primary, t.text_muted)
    } else {
        (t.text_muted, t.text_muted)
    };

    let bg_hover = t.bg_hover;

    let base = div()
        .id(id)
        .mx(SPACE_SM)
        .px(SPACE_LG)
        .py(SPACE_SM)
        .flex()
        .items_center()
        .gap(SPACE_LG)
        .rounded(RADIUS_STD)
        .text_size(MENU_TEXT)
        .text_color(rgb(text_color))
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::Arrow
        })
        .child(svg().path(icon).size(MENU_ICON).text_color(rgb(icon_color)))
        .child(label.into());

    if enabled {
        base.hover(move |s| s.bg(rgb(bg_hover)))
    } else {
        base
    }
}

/// Context menu panel with standard styling (bg, border, shadow, min_w, py).
///
/// Comes with stop-propagation handlers on left-click, right-click, and scroll.
/// Caller adds `.child(menu_item(...))` for content.
pub fn context_menu_panel(id: impl Into<ElementId>, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(id)
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(8.0))
        .shadow_xl()
        .min_w(px(240.0))
        .py(SPACE_SM)
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Right, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_scroll_wheel(|_, _, cx| {
            cx.stop_propagation();
        })
}

/// Menu separator - 1px horizontal line.
pub fn menu_separator(t: &ThemeColors) -> Div {
    div().h(px(1.0)).mx(SPACE_XL).my(SPACE_SM).bg(rgb(t.border))
}

/// Section label above a run of menu items.
///
/// Replaces a bare `menu_separator` where the group has a name worth showing: the
/// divider costs a line either way, so it may as well say what the group is.
/// Aligns with `menu_item`'s text, not the panel edge.
pub fn menu_section(label: impl Into<SharedString>, t: &ThemeColors) -> Div {
    div()
        .mx(SPACE_SM)
        .px(SPACE_LG)
        .pt(SPACE_MD)
        .pb(SPACE_XS)
        .text_size(TEXT_SM)
        .text_color(rgb(t.text_muted))
        .child(label.into().to_uppercase())
}

/// Bar button icon size — a touch smaller than `MENU_ICON`, which reads too heavy
/// next to a short centred label.
const BAR_ICON: Pixels = px(13.0);

/// Click handler shape produced by `Context::listener`, so callers pass one straight in.
type BarHandler = std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// One button inside an [`action_bar`].
pub struct ActionBarButton {
    id: SharedString,
    icon: SharedString,
    label: SharedString,
    enabled: bool,
    on_click: BarHandler,
}

impl ActionBarButton {
    pub fn new(
        id: impl Into<SharedString>,
        icon: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            icon: icon.into(),
            label: label.into(),
            enabled: true,
            on_click: std::rc::Rc::new(on_click),
        }
    }

    /// A disabled button keeps its slot so the bar's widths stay put.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// A row of buttons that together fill the menu's width — two split it in half,
/// three in thirds.
///
/// Rendered as one segmented control rather than separate boxes: a single outline
/// with hairline dividers, so the buttons read as one decision instead of several
/// commands that happen to sit on a line.
///
/// Use it where the buttons really are one decision — the clearest case is one verb
/// with a parameter, such as splitting a pane vertically or horizontally. Commands
/// that merely sit near each other belong in plain rows.
pub fn action_bar(buttons: Vec<ActionBarButton>, t: &ThemeColors) -> Div {
    let last = buttons.len().saturating_sub(1);

    let mut bar = div()
        .mx(SPACE_SM)
        .mb(SPACE_XS)
        .flex()
        .items_stretch()
        .rounded(RADIUS_STD)
        .bg(rgb(t.bg_secondary))
        .border_1()
        .border_color(rgb(t.border));

    for (index, button) in buttons.into_iter().enumerate() {
        let (text_color, icon_color) = if button.enabled {
            (t.text_primary, t.text_muted)
        } else {
            (t.text_muted, t.text_muted)
        };
        let bg_hover = t.bg_hover;
        let handler = button.on_click;

        let slot = div()
            .id(ElementId::from(button.id))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .justify_center()
            .gap(SPACE_SM)
            .px(SPACE_MD)
            .py(SPACE_SM)
            // Corners are set per end rather than clipped by the container, so a
            // hover fill stays inside the outline instead of squaring it off.
            .when(index == 0, |slot| slot.rounded_l(RADIUS_STD))
            .when(index == last, |slot| slot.rounded_r(RADIUS_STD))
            .when(index > 0, |slot| {
                slot.border_l_1().border_color(rgb(t.border))
            })
            .text_size(TEXT_MD)
            .text_color(rgb(text_color))
            .cursor(if button.enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .child(
                svg()
                    .path(button.icon)
                    .size(BAR_ICON)
                    .flex_shrink_0()
                    .text_color(rgb(icon_color)),
            )
            .child(div().min_w_0().truncate().child(button.label));

        bar = bar.child(if button.enabled {
            slot.hover(move |s| s.bg(rgb(bg_hover)))
                .on_click(move |event, window, cx| handler(event, window, cx))
        } else {
            slot
        });
    }

    bar
}

/// Header row of a menu: the target's icon and name, plus trailing controls.
///
/// The caller fills it with [`menu_header_name`] and whatever belongs on the right
/// edge, such as a shell chip. Lets a menu act on the thing it names without
/// spending a command row on each of its properties.
pub fn menu_header(icon: impl Into<SharedString>, t: &ThemeColors) -> Div {
    div()
        .mx(SPACE_SM)
        .px(SPACE_LG)
        .py(SPACE_SM)
        .flex()
        .items_center()
        .gap(SPACE_MD)
        .child(
            svg()
                .path(icon)
                .size(MENU_ICON)
                .text_color(rgb(t.border_active)),
        )
}

/// The clickable name inside a [`menu_header`], with a pencil marking it editable.
///
/// Takes the row's spare width so trailing controls sit against the right edge, and
/// caps its own width — an OSC-set terminal title can be arbitrarily long.
pub fn menu_header_name(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(SPACE_SM)
        .px(SPACE_XS)
        .py(px(2.0))
        .rounded(RADIUS_STD)
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(MENU_TEXT)
        .text_color(rgb(t.text_primary))
        .child(
            // `truncate` is overflow-hidden + nowrap + ellipsis: without the nowrap a long
            // OSC-set title wraps to a second line and grows the header instead of clipping.
            div().max_w(px(160.0)).truncate().child(label.into()),
        )
        .child(
            svg()
                .path("icons/edit.svg")
                .size(px(11.0))
                .text_color(rgb(t.text_muted)),
        )
}
