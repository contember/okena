//! Submenu (flyout) primitive for context menus.
//!
//! A parent row inside a `context_menu_panel` that opens a second panel beside it.
//! Built on `menu.rs` so the terminal, tab and sidebar menus can share one flyout.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::menu::{context_menu_panel, menu_item};
use crate::theme::ThemeColors;
use crate::tokens::SPACE_SM;
use gpui::prelude::*;
use gpui::*;

/// Hover dwell before a flyout opens, so a cursor passing over the row does not flash it open.
const OPEN_DELAY: Duration = Duration::from_millis(180);

/// Grace after the pointer leaves both the row and the flyout. It buys the diagonal travel
/// across the sibling rows between the parent row and the flyout, so no exact path is needed.
const CLOSE_GRACE: Duration = Duration::from_millis(300);

/// Flyout width, pinned to `context_menu_panel`'s `min_w` so the edge-flip decision is exact.
pub const FLYOUT_WIDTH: Pixels = px(240.0);

/// `menu_item`'s horizontal margin (`SPACE_SM`) plus the panel's 1px border. The flyout is
/// pulled back over both so it touches the parent panel with no gap for the pointer to
/// fall through.
const PANEL_EDGE_OVERLAP: Pixels = px(7.0);

/// `menu_item`'s horizontal margin, used by the pure edge-flip helpers.
const PANEL_ITEM_MARGIN: Pixels = SPACE_SM;

/// `context_menu_panel`'s border plus its vertical padding — lines the first flyout item
/// up with the parent row.
const PANEL_TOP_INSET: Pixels = px(7.0);

/// Chevron size on the parent row.
const CHEVRON_SIZE: Pixels = px(12.0);

/// A group needs at least this many entries to be worth a flyout.
const MIN_FLYOUT_ITEMS: usize = 2;

// =============================================================================
// Pure layout decisions
// =============================================================================

/// How a group renders for a given number of entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmenuLayout {
    /// No entries — the group is dropped entirely, leaving no row behind.
    Empty,
    /// One entry — a plain `menu_item`; a one-item flyout is not worth the extra click.
    Collapsed,
    /// Two or more entries — a parent row with a flyout.
    Flyout,
}

/// Decide how a group with `item_count` entries renders.
pub fn submenu_layout(item_count: usize) -> SubmenuLayout {
    match item_count {
        0 => SubmenuLayout::Empty,
        n if n < MIN_FLYOUT_ITEMS => SubmenuLayout::Collapsed,
        _ => SubmenuLayout::Flyout,
    }
}

/// Which side of the parent row a flyout opens toward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlyoutSide {
    Right,
    Left,
}

/// Prefer the right side; flip left only when the right overflows the window and the left fits.
///
/// Mirrors what GPUI's own `AnchoredFitMode::SwitchAnchor` does, but flips around the parent
/// panel instead of around the anchor point, so a flipped flyout sits beside the panel.
pub fn flyout_side(
    row: Bounds<Pixels>,
    flyout_width: Pixels,
    viewport_width: Pixels,
) -> FlyoutSide {
    let right_overflows = row.right() + PANEL_ITEM_MARGIN + flyout_width > viewport_width;
    let left_fits = row.left() - PANEL_ITEM_MARGIN - flyout_width >= px(0.0);

    if right_overflows && left_fits {
        FlyoutSide::Left
    } else {
        FlyoutSide::Right
    }
}

// =============================================================================
// Items
// =============================================================================

/// Click handler shared by the rendered item and the flyout's Enter key.
type ItemHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// One entry inside a flyout.
pub struct SubmenuItem {
    id: SharedString,
    icon: SharedString,
    label: SharedString,
    on_click: ItemHandler,
}

impl SubmenuItem {
    /// `on_click` takes the shape `Context::listener` produces, so callers pass it straight in.
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
            on_click: Rc::new(on_click),
        }
    }
}

// =============================================================================
// State
// =============================================================================

/// Hover, keyboard and geometry state for the submenus of one menu view.
///
/// The host view owns exactly one of these and reaches it through [`SubmenuHost`].
#[derive(Default)]
pub struct SubmenuState {
    /// Id of the group whose flyout is open.
    open: Option<SharedString>,
    /// Id of the group whose row is under the pointer.
    hovered_row: Option<SharedString>,
    /// Whether the pointer is inside the open flyout.
    over_flyout: bool,
    /// Keyboard highlight inside the open flyout; cleared once the mouse takes over.
    active_item: Option<usize>,
    /// Row bounds captured during paint, keyed by group id. Used for the edge flip.
    row_bounds: HashMap<SharedString, Bounds<Pixels>>,
    /// Focus target for the open flyout; keyboard navigation lives on it.
    /// Created on first render so the state itself needs no `App` to build.
    flyout_focus: Option<FocusHandle>,
    /// Per-row focus handles, created on first render of each row.
    row_focus: HashMap<SharedString, FocusHandle>,
    /// Pending open or close. Dropping it cancels it, which is how a re-hover aborts one.
    timer: Option<Task<()>>,
}

impl SubmenuState {
    /// Whether any flyout is open.
    pub fn is_any_open(&self) -> bool {
        self.open.is_some()
    }

    /// Whether the flyout of `id` is open.
    pub fn is_open(&self, id: &str) -> bool {
        self.open.as_deref() == Some(id)
    }

    /// Keyboard highlight inside the open flyout.
    pub fn active_item(&self) -> Option<usize> {
        self.active_item
    }

    /// Close any open flyout. Returns whether something was open.
    pub fn close(&mut self) -> bool {
        self.active_item = None;
        self.timer = None;
        self.open.take().is_some()
    }

    /// Open `id` right away, bypassing the hover-intent delay (click and keyboard use this).
    pub fn open_now(&mut self, id: SharedString) {
        self.open = Some(id);
        self.active_item = None;
        self.timer = None;
    }

    /// Move the keyboard highlight, wrapping at both ends.
    pub fn move_active(&mut self, forward: bool, len: usize) {
        if len == 0 {
            return;
        }
        self.active_item = Some(match self.active_item {
            Some(index) if forward => (index + 1) % len,
            Some(0) => len - 1,
            Some(index) => index - 1,
            None if forward => 0,
            None => len - 1,
        });
    }

    /// Record that the pointer entered or left the row of `id`.
    pub fn set_row_hovered(&mut self, id: &SharedString, hovered: bool) {
        if hovered {
            self.hovered_row = Some(id.clone());
        } else if self.hovered_row.as_deref() == Some(id.as_ref()) {
            self.hovered_row = None;
        }
    }

    /// Record that the pointer entered or left the open flyout.
    pub fn set_flyout_hovered(&mut self, hovered: bool) {
        self.over_flyout = hovered;
        if hovered {
            // The mouse takes the highlight over from the keyboard.
            self.active_item = None;
        }
    }

    /// Finish a pending hover-intent open, unless the pointer already moved off the row.
    pub fn commit_open(&mut self, id: &SharedString) -> bool {
        if self.hovered_row.as_deref() != Some(id.as_ref()) || self.is_open(id) {
            return false;
        }
        self.open = Some(id.clone());
        self.active_item = None;
        true
    }

    /// Whether the open flyout has lost the pointer from both its row and itself.
    pub fn should_close(&self) -> bool {
        match &self.open {
            Some(open) => !self.over_flyout && self.hovered_row.as_deref() != Some(open.as_ref()),
            None => false,
        }
    }

    /// Close once the grace period has run out and the pointer is still away.
    pub fn close_if_left(&mut self) -> bool {
        if self.should_close() {
            self.close()
        } else {
            false
        }
    }

    /// Whether the flyout or one of the parent rows currently holds focus.
    ///
    /// A host menu that refocuses itself while rendering must consult this, or it snatches
    /// focus back from a flyout that has not been rendered yet.
    pub fn holds_focus(&self, window: &Window) -> bool {
        self.flyout_focus
            .as_ref()
            .is_some_and(|handle| handle.is_focused(window))
            || self
                .row_focus
                .values()
                .any(|handle| handle.is_focused(window))
    }

    /// Bounds of the row of `id` as of the last paint.
    pub fn row_bounds(&self, id: &str) -> Option<Bounds<Pixels>> {
        self.row_bounds.get(id).copied()
    }

    /// Record row bounds during paint. Deliberately does not notify — a notify here
    /// would re-render every frame.
    pub fn set_row_bounds(&mut self, id: SharedString, bounds: Bounds<Pixels>) {
        self.row_bounds.insert(id, bounds);
    }

    fn arm(&mut self, task: Task<()>) {
        self.timer = Some(task);
    }

    fn flyout_focus(&mut self, cx: &mut App) -> FocusHandle {
        self.flyout_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone()
    }

    fn row_focus(&mut self, id: &SharedString, cx: &mut App) -> FocusHandle {
        self.row_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }
}

/// Implemented by the menu view that owns the flyout state, so the primitive's own
/// listeners can reach it.
pub trait SubmenuHost: 'static {
    fn submenu_state(&mut self) -> &mut SubmenuState;
}

// =============================================================================
// Rendering
// =============================================================================

/// A menu group that renders as a flyout beside its parent row.
pub struct Submenu {
    id: SharedString,
    icon: SharedString,
    label: SharedString,
    items: Vec<SubmenuItem>,
}

impl Submenu {
    pub fn new(
        id: impl Into<SharedString>,
        icon: impl Into<SharedString>,
        label: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            icon: icon.into(),
            label: label.into(),
            items: Vec::new(),
        }
    }

    pub fn item(mut self, item: SubmenuItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = SubmenuItem>) -> Self {
        self.items.extend(items);
        self
    }

    /// Build the element for this group.
    ///
    /// Collapses to a plain `menu_item` for a single entry and renders nothing for none, so a
    /// caller can drop entries for the current mode without leaving a dead-end row behind.
    pub fn render<V: SubmenuHost>(
        mut self,
        state: &mut SubmenuState,
        t: &ThemeColors,
        window: &Window,
        cx: &mut Context<V>,
    ) -> AnyElement {
        match submenu_layout(self.items.len()) {
            SubmenuLayout::Empty => div().into_any_element(),
            SubmenuLayout::Collapsed => {
                let item = self.items.remove(0);
                let handler = item.on_click;
                menu_item(ElementId::Name(item.id), item.icon, item.label, t)
                    .on_click(move |event, window, cx| handler(event, window, cx))
                    .into_any_element()
            }
            SubmenuLayout::Flyout => self.render_flyout(state, t, window, cx),
        }
    }

    fn render_flyout<V: SubmenuHost>(
        self,
        state: &mut SubmenuState,
        t: &ThemeColors,
        window: &Window,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let Submenu {
            id,
            icon,
            label,
            items,
        } = self;
        let entity = cx.entity().downgrade();
        let is_open = state.is_open(&id);
        let active_item = state.active_item();
        let row_focus = state.row_focus(&id, cx);
        let flyout_focus = state.flyout_focus(cx);
        let handlers: Vec<ItemHandler> = items.iter().map(|item| item.on_click.clone()).collect();
        let item_count = items.len();

        // Only the side comes from the measured row; the offset is relative to the row
        // itself, so the flyout meets the panel edge whatever box the canvas reports.
        let side = state
            .row_bounds(&id)
            .map(|bounds| flyout_side(bounds, FLYOUT_WIDTH, window.viewport_size().width))
            .unwrap_or(FlyoutSide::Right);

        let bounds_setter = {
            let entity = entity.clone();
            let id = id.clone();
            move |bounds: Bounds<Pixels>, _: &mut Window, cx: &mut App| {
                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |view, _| {
                        view.submenu_state().set_row_bounds(id.clone(), bounds);
                    });
                }
            }
        };

        let row = menu_item(ElementId::Name(id.clone()), icon, label, t)
            .relative()
            .track_focus(&row_focus)
            .when(is_open, |row| row.bg(rgb(t.bg_hover)))
            .child(div().flex_1())
            .child(
                svg()
                    .path("icons/chevron-right.svg")
                    .size(CHEVRON_SIZE)
                    .text_color(rgb(t.text_muted)),
            )
            .child(
                canvas(bounds_setter, |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .on_hover({
                let id = id.clone();
                cx.listener(move |view, hovered: &bool, _window, cx| {
                    view.submenu_state().set_row_hovered(&id, *hovered);
                    if *hovered {
                        arm_open(view, id.clone(), cx);
                    } else {
                        arm_close(view, cx);
                    }
                })
            })
            .on_click({
                let id = id.clone();
                let flyout_focus = flyout_focus.clone();
                cx.listener(move |view, _, window, cx| {
                    let opened = !view.submenu_state().is_open(&id);
                    if opened {
                        view.submenu_state().open_now(id.clone());
                        window.focus(&flyout_focus, cx);
                    } else {
                        view.submenu_state().close();
                    }
                    cx.notify();
                })
            })
            .on_key_down({
                let id = id.clone();
                let row_focus = row_focus.clone();
                let flyout_focus = flyout_focus.clone();
                cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                    // The flyout is a child of this row, so its keys bubble through here too.
                    if !row_focus.is_focused(window) {
                        return;
                    }
                    if !matches!(event.keystroke.key.as_str(), "right" | "enter") {
                        return;
                    }
                    view.submenu_state().open_now(id.clone());
                    view.submenu_state().move_active(true, item_count);
                    window.focus(&flyout_focus, cx);
                    cx.stop_propagation();
                    cx.notify();
                })
            });

        if !is_open {
            return row.into_any_element();
        }

        let item_elements: Vec<AnyElement> = items
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let handler = item.on_click;
                let entity = entity.clone();
                menu_item(ElementId::Name(item.id), item.icon, item.label, t)
                    .when(active_item == Some(index), |el| el.bg(rgb(t.bg_hover)))
                    .on_click(move |event, window, cx| {
                        close_flyout(&entity, cx);
                        handler(event, window, cx);
                    })
                    .into_any_element()
            })
            .collect();

        let flyout = context_menu_panel(ElementId::Name(format!("{id}-flyout").into()), t)
            .w(FLYOUT_WIDTH)
            .track_focus(&flyout_focus)
            .on_hover(cx.listener(move |view, hovered: &bool, _window, cx| {
                view.submenu_state().set_flyout_hovered(*hovered);
                if !*hovered {
                    arm_close(view, cx);
                }
                cx.notify();
            }))
            .on_key_down({
                let entity = entity.clone();
                let row_focus = row_focus.clone();
                // A plain closure, not `cx.listener`: activating an item calls back into a
                // handler the host built with `cx.listener`, which cannot run while the
                // host entity is already leased.
                move |event: &KeyDownEvent, window, cx: &mut App| {
                    let Some(view) = entity.upgrade() else {
                        return;
                    };
                    match event.keystroke.key.as_str() {
                        key @ ("down" | "up") => {
                            view.update(cx, |view, cx| {
                                view.submenu_state().move_active(key == "down", item_count);
                                cx.notify();
                            });
                            cx.stop_propagation();
                        }
                        "left" => {
                            view.update(cx, |view, cx| {
                                view.submenu_state().close();
                                cx.notify();
                            });
                            window.focus(&row_focus, cx);
                            cx.stop_propagation();
                        }
                        "enter" => {
                            let active = view.update(cx, |view, cx| {
                                let active = view.submenu_state().active_item();
                                view.submenu_state().close();
                                cx.notify();
                                active
                            });
                            if let Some(handler) = active.and_then(|index| handlers.get(index)) {
                                handler(&ClickEvent::default(), window, cx);
                            }
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }
            })
            .children(item_elements);

        // Anchored to the row's own edge rather than to window coordinates: the row is
        // inset from the panel by its margin and padding, and deriving the offset from the
        // measured bounds meant guessing which box `canvas` reports. `left_full` puts the
        // flyout exactly on the row's outer edge, pulled back over the panel's border and
        // item margin so the two panels touch and the pointer cannot fall between them.
        let positioned = match side {
            FlyoutSide::Right => flyout.left_full().ml(-PANEL_EDGE_OVERLAP),
            FlyoutSide::Left => flyout.right_full().mr(-PANEL_EDGE_OVERLAP),
        };

        // Deferred so it paints over the menu items that follow it in the panel.
        row.child(deferred(positioned.absolute().top(-PANEL_TOP_INSET)).with_priority(1))
            .into_any_element()
    }
}

/// Start the hover-intent delay before the flyout of `id` opens.
fn arm_open<V: SubmenuHost>(view: &mut V, id: SharedString, cx: &mut Context<V>) {
    if view.submenu_state().is_open(&id) {
        view.submenu_state().timer = None;
        return;
    }
    let task = cx.spawn({
        let id = id.clone();
        async move |this: WeakEntity<V>, cx| {
            smol::Timer::after(OPEN_DELAY).await;
            this.update(cx, |view, cx| {
                if view.submenu_state().commit_open(&id) {
                    cx.notify();
                }
            })
            .ok();
        }
    });
    view.submenu_state().arm(task);
}

/// Start the grace period after the pointer left the row or the flyout.
///
/// The task re-reads the hover state when it fires, so it does not matter in which order
/// the leave and enter events of a single pointer move arrive.
fn arm_close<V: SubmenuHost>(view: &mut V, cx: &mut Context<V>) {
    let task = cx.spawn(async move |this: WeakEntity<V>, cx| {
        smol::Timer::after(CLOSE_GRACE).await;
        this.update(cx, |view, cx| {
            if view.submenu_state().close_if_left() {
                cx.notify();
            }
        })
        .ok();
    });
    view.submenu_state().arm(task);
}

fn close_flyout<V: SubmenuHost>(entity: &WeakEntity<V>, cx: &mut App) {
    if let Some(entity) = entity.upgrade() {
        entity.update(cx, |view, cx| {
            view.submenu_state().close();
            cx.notify();
        });
    }
}

#[cfg(test)]
mod tests {
    // Imported one by one on purpose: a glob of `super::*` would pull in gpui's own
    // `test` attribute macro and make `#[test]` expand into itself.
    use super::{
        FLYOUT_WIDTH, FlyoutSide, SubmenuLayout, SubmenuState, flyout_side, submenu_layout,
    };
    use gpui::{Bounds, Pixels, SharedString, point, px, size};

    fn row(x: f32, width: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(100.0)),
            size: size(px(width), px(26.0)),
        }
    }

    #[test]
    fn empty_group_renders_nothing() {
        assert_eq!(submenu_layout(0), SubmenuLayout::Empty);
    }

    #[test]
    fn single_entry_collapses_to_a_plain_item() {
        assert_eq!(submenu_layout(1), SubmenuLayout::Collapsed);
    }

    #[test]
    fn two_or_more_entries_get_a_flyout() {
        assert_eq!(submenu_layout(2), SubmenuLayout::Flyout);
        assert_eq!(submenu_layout(9), SubmenuLayout::Flyout);
    }

    #[test]
    fn hover_intent_is_dropped_when_the_pointer_moves_on() {
        let mut state = SubmenuState::default();
        let id: SharedString = "buffer".into();

        state.set_row_hovered(&id, true);
        state.set_row_hovered(&id, false);

        assert!(!state.commit_open(&id));
        assert!(!state.is_any_open());
    }

    #[test]
    fn hover_intent_opens_while_the_pointer_stays() {
        let mut state = SubmenuState::default();
        let id: SharedString = "buffer".into();

        state.set_row_hovered(&id, true);

        assert!(state.commit_open(&id));
        assert!(state.is_open("buffer"));
    }

    #[test]
    fn leaving_the_row_for_the_flyout_does_not_close() {
        let mut state = SubmenuState::default();
        let id: SharedString = "buffer".into();
        state.open_now(id.clone());

        // The diagonal: the flyout is entered before or after the row is left, in either order.
        state.set_flyout_hovered(true);
        state.set_row_hovered(&id, false);

        assert!(!state.should_close());
        assert!(!state.close_if_left());
    }

    #[test]
    fn leaving_both_the_row_and_the_flyout_closes() {
        let mut state = SubmenuState::default();
        let id: SharedString = "buffer".into();
        state.open_now(id.clone());
        state.set_row_hovered(&id, true);

        state.set_row_hovered(&id, false);
        state.set_flyout_hovered(false);

        assert!(state.close_if_left());
        assert!(!state.is_any_open());
    }

    #[test]
    fn keyboard_highlight_wraps_at_both_ends() {
        let mut state = SubmenuState::default();

        state.move_active(true, 3);
        assert_eq!(state.active_item(), Some(0));
        state.move_active(false, 3);
        assert_eq!(state.active_item(), Some(2));
        state.move_active(true, 3);
        assert_eq!(state.active_item(), Some(0));
    }

    #[test]
    fn opens_right_when_there_is_room() {
        let side = flyout_side(row(100.0, 228.0), FLYOUT_WIDTH, px(1200.0));
        assert_eq!(side, FlyoutSide::Right);
    }

    #[test]
    fn flips_left_at_the_window_edge() {
        // Row ends at 928; 928 + 6 + 240 = 1174 > 1000, and 100 - 6 - 240 fits.
        let side = flyout_side(row(700.0, 228.0), FLYOUT_WIDTH, px(1000.0));
        assert_eq!(side, FlyoutSide::Left);
    }

    #[test]
    fn stays_right_when_neither_side_fits() {
        // A window narrower than the flyout: flipping would only make it worse,
        // so it stays right and lets `snap_to_window` pull it back in.
        let side = flyout_side(row(10.0, 180.0), FLYOUT_WIDTH, px(200.0));
        assert_eq!(side, FlyoutSide::Right);
    }
}
