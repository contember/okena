//! Generic list overlay component for searchable/navigable lists.
//!
//! Provides shared infrastructure for overlays like:
//! - Command palette (searchable)
//! - Project switcher (searchable with extra actions)
//! - Theme selector (no search)
//! - Shell selector (no search)
//! - File search (fuzzy search with scoring)

use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_ui::simple_input::SimpleInputState;

/// Configuration for a list overlay.
#[derive(Clone)]
pub struct ListOverlayConfig {
    /// Width of the modal in pixels.
    pub width: f32,
    /// Maximum height of the modal in pixels.
    pub max_height: f32,
    /// Title shown in the modal header.
    pub title: String,
    /// Optional subtitle shown below the title.
    pub subtitle: Option<String>,
    /// Placeholder text for the search input. If None, search is disabled and
    /// the state has no input.
    pub search_placeholder: Option<String>,
    /// Message shown when the list is empty after filtering.
    pub empty_message: String,
    /// Keyboard hints shown in the footer as (key, description) pairs.
    pub keyboard_hints: Vec<(String, String)>,
    /// Whether to center the modal vertically (true) or position at top (false).
    pub centered: bool,
    /// Key context for keyboard shortcuts (e.g., "CommandPalette").
    pub key_context: String,
}

impl ListOverlayConfig {
    /// Create a new config with default values.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            width: 500.0,
            max_height: 450.0,
            title: title.into(),
            subtitle: None,
            search_placeholder: None,
            empty_message: "No items found".to_string(),
            keyboard_hints: vec![
                ("Enter".to_string(), "select".to_string()),
                ("Esc".to_string(), "close".to_string()),
            ],
            centered: false,
            key_context: "ListOverlay".to_string(),
        }
    }

    /// Set the subtitle.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Enable search with the given placeholder.
    pub fn searchable(mut self, placeholder: impl Into<String>) -> Self {
        self.search_placeholder = Some(placeholder.into());
        self
    }

    /// Set the modal dimensions.
    pub fn size(mut self, width: f32, max_height: f32) -> Self {
        self.width = width;
        self.max_height = max_height;
        self
    }

    /// Center the modal vertically.
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    /// Set the key context for keyboard shortcuts.
    pub fn key_context(mut self, context: impl Into<String>) -> Self {
        self.key_context = context.into();
        self
    }

    /// Set the empty message.
    pub fn empty_message(mut self, message: impl Into<String>) -> Self {
        self.empty_message = message.into();
        self
    }

    /// Set keyboard hints as (key, description) pairs.
    pub fn keyboard_hints(mut self, hints: Vec<(impl Into<String>, impl Into<String>)>) -> Self {
        self.keyboard_hints = hints
            .into_iter()
            .map(|(k, d)| (k.into(), d.into()))
            .collect();
        self
    }
}

/// Result of filtering an item.
#[derive(Clone, Debug, Default)]
pub struct FilterResult {
    /// Original index in the items list.
    pub index: usize,
}

impl FilterResult {
    pub fn new(index: usize) -> Self {
        Self { index }
    }
}

/// Shared state for list overlays.
pub struct ListOverlayState<T: Clone> {
    /// Focus handle for keyboard events.
    pub focus_handle: FocusHandle,
    /// Scroll handle for list scrolling.
    pub scroll_handle: ScrollHandle,
    /// All items in the list.
    pub items: Vec<T>,
    /// Filtered items (indices).
    pub filtered: Vec<FilterResult>,
    /// Currently selected index (into filtered list).
    pub selected_index: usize,
    /// Search input, present iff the config is searchable. A real text input
    /// (cursor, selection, Cmd+A, word deletion); the owner subscribes to its
    /// `InputChangedEvent` to refilter, renders it with `search_input_row` and
    /// focuses it with `focus_search`.
    pub search_input: Option<Entity<SimpleInputState>>,
    /// Configuration.
    pub config: ListOverlayConfig,
}

impl<T: Clone> ListOverlayState<T> {
    /// Create a new state with the given items and config.
    pub fn new(items: Vec<T>, config: ListOverlayConfig, cx: &mut App) -> Self {
        let filtered: Vec<FilterResult> = (0..items.len()).map(FilterResult::new).collect();
        let search_input = config
            .search_placeholder
            .clone()
            .map(|placeholder| cx.new(|cx| SimpleInputState::new(cx).placeholder(placeholder)));

        Self {
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            items,
            filtered,
            selected_index: 0,
            search_input,
            config,
        }
    }

    /// Create a new state with a pre-selected index.
    pub fn with_selected(
        items: Vec<T>,
        config: ListOverlayConfig,
        selected_index: usize,
        cx: &mut App,
    ) -> Self {
        let mut state = Self::new(items, config, cx);
        state.selected_index = selected_index.min(state.filtered.len().saturating_sub(1));
        state
    }

    /// Get the currently selected item, if any.
    pub fn selected_item(&self) -> Option<&T> {
        self.filtered
            .get(self.selected_index)
            .map(|f| &self.items[f.index])
    }

    /// Move selection up.
    pub fn select_prev(&mut self) -> bool {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.scroll_to_selected();
            true
        } else {
            false
        }
    }

    /// Move selection down.
    pub fn select_next(&mut self) -> bool {
        if self.selected_index < self.filtered.len().saturating_sub(1) {
            self.selected_index += 1;
            self.scroll_to_selected();
            true
        } else {
            false
        }
    }

    /// Scroll to keep the selected item visible.
    pub fn scroll_to_selected(&self) {
        if !self.filtered.is_empty() {
            self.scroll_handle.scroll_to_item(self.selected_index);
        }
    }

    /// Current search query (empty when the overlay is not searchable).
    pub fn search_query(&self, cx: &App) -> String {
        self.search_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default()
    }

    /// Refilter whenever the search input changes. Every searchable overlay
    /// needs exactly this, so it lives here rather than in each owner.
    pub fn on_search_changed<V: 'static>(
        &self,
        cx: &mut Context<V>,
        refilter: impl Fn(&mut V, &mut Context<V>) + 'static,
    ) {
        if let Some(input) = &self.search_input {
            cx.subscribe(
                input,
                move |this, _, _: &okena_ui::simple_input::InputChangedEvent, cx| {
                    refilter(this, cx);
                    cx.notify();
                },
            )
            .detach();
        }
    }

    /// The search row, when the overlay is searchable.
    pub fn search_row(&self, t: &okena_core::theme::ThemeColors, cx: &App) -> Option<Div> {
        self.search_input
            .as_ref()
            .map(|input| search_input_row(input, t, cx))
    }

    /// Drop the search input's selection, so the next keystroke extends the
    /// query instead of replacing it.
    pub fn deselect_search(&self, cx: &mut App) {
        if let Some(input) = &self.search_input {
            input.update(cx, |input, cx| {
                input.clear_selection(cx);
            });
        }
    }

    /// Keep keyboard focus where typing belongs: the search input when there
    /// is one, otherwise the overlay itself. Call on every render.
    pub fn focus_search(&self, window: &mut Window, cx: &mut App) {
        match &self.search_input {
            Some(input) => focus_input(input, window, cx),
            None => {
                if !self.focus_handle.is_focused(window) {
                    window.focus(&self.focus_handle, cx);
                }
            }
        }
    }

    /// Update the filtered list and reset selection to first item.
    pub fn set_filtered(&mut self, filtered: Vec<FilterResult>) {
        self.filtered = filtered;
        self.selected_index = 0;
    }

    /// Check if the list is empty after filtering.
    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }
}

/// Actions that can result from key handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListOverlayAction {
    /// Close the overlay.
    Close,
    /// Move selection up.
    SelectPrev,
    /// Move selection down.
    SelectNext,
    /// Confirm the current selection.
    Confirm,
    /// No action taken.
    None,
}

/// Handle keyboard events for a list overlay.
///
/// Navigation and confirm/close only. Text editing belongs to the state's
/// search input, which consumes the keys it handles before they bubble here.
/// An overlay that needs a key of its own the input would otherwise type
/// (the project switcher's Space) takes it in a `capture_key_down` handler.
///
/// Escape normally never gets here: the default keymap binds it to `Cancel`,
/// and gpui dispatches bindings before key listeners, so it closes the overlay
/// even while the input has a selection. The arm below covers a keymap where
/// Escape was rebound.
pub fn handle_list_overlay_key<T: Clone>(
    state: &mut ListOverlayState<T>,
    event: &KeyDownEvent,
) -> ListOverlayAction {
    match event.keystroke.key.as_str() {
        "escape" => ListOverlayAction::Close,
        "up" => {
            if state.select_prev() {
                ListOverlayAction::SelectPrev
            } else {
                ListOverlayAction::None
            }
        }
        "down" => {
            if state.select_next() {
                ListOverlayAction::SelectNext
            } else {
                ListOverlayAction::None
            }
        }
        "enter" => ListOverlayAction::Confirm,
        _ => ListOverlayAction::None,
    }
}

/// Navigation helper for uniform list overlays.
///
/// Manages selected_index within a total count, scrolling to keep selection visible.
/// Returns true if the selection changed.
pub fn select_prev(selected_index: &mut usize, scroll_handle: &UniformListScrollHandle) -> bool {
    if *selected_index > 0 {
        *selected_index -= 1;
        scroll_handle.scroll_to_item(*selected_index, ScrollStrategy::Top);
        true
    } else {
        false
    }
}

/// Move selection down. Returns true if the selection changed.
pub fn select_next(
    selected_index: &mut usize,
    total: usize,
    scroll_handle: &UniformListScrollHandle,
) -> bool {
    if *selected_index < total.saturating_sub(1) {
        *selected_index += 1;
        scroll_handle.scroll_to_item(*selected_index, ScrollStrategy::Top);
        true
    } else {
        false
    }
}

/// Focus `input` unless it already has focus. Overlays call this on every
/// render so typing always lands in their search field.
pub fn focus_input(input: &Entity<SimpleInputState>, window: &mut Window, cx: &mut App) {
    if !input.read(cx).focus_handle(cx).is_focused(window) {
        input.update(cx, |input, cx| input.focus(window, cx));
    }
}

/// Render a search input row with ">" prompt prefix and a SimpleInput.
///
/// The one search field every list overlay uses, sized as the prominent line
/// at the top of a quick-pick dialog.
pub fn search_input_row(
    input: &Entity<okena_ui::simple_input::SimpleInputState>,
    t: &okena_core::theme::ThemeColors,
    cx: &App,
) -> Div {
    use okena_ui::simple_input::SimpleInput;
    use okena_ui::tokens::{SPACE_LG, SPACE_MD, ui_text_xl};

    div()
        .px(SPACE_LG)
        .py(px(10.0))
        .border_b_1()
        .border_color(rgb(t.border))
        .flex()
        .items_center()
        .gap(SPACE_MD)
        .child(
            div()
                .text_size(ui_text_xl(cx))
                .text_color(rgb(t.text_muted))
                .child(">"),
        )
        .child(
            div()
                .flex_1()
                .child(SimpleInput::new(input).text_size(ui_text_xl(cx))),
        )
}

/// Substring filter for list items.
///
/// Returns filtered indices where any of the search fields contain the query.
pub fn substring_filter<T, F>(items: &[T], query: &str, get_fields: F) -> Vec<FilterResult>
where
    F: Fn(&T) -> Vec<String>,
{
    if query.is_empty() {
        return (0..items.len()).map(FilterResult::new).collect();
    }

    let query_lower = query.to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            get_fields(item)
                .iter()
                .any(|field| field.to_lowercase().contains(&query_lower))
        })
        .map(|(i, _)| FilterResult::new(i))
        .collect()
}

/// Render a "filters" toggle button that opens a popover.
///
/// `on_click` is called when the button is clicked (to toggle popover open state).
/// `on_bounds` is called during paint with the button's window-absolute bounds.
pub fn file_filter_button(
    id: impl Into<SharedString>,
    active_count: u8,
    t: &okena_core::theme::ThemeColors,
    cx: &App,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_bounds: impl Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    use okena_ui::tokens::ui_text_sm;

    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(4.0))
        .text_size(ui_text_sm(cx))
        .font_weight(FontWeight::MEDIUM)
        .when(active_count > 0, |d: Stateful<Div>| {
            d.bg(rgb(t.border_active)).text_color(rgb(t.text_primary))
        })
        .when(active_count == 0, |d: Stateful<Div>| {
            d.bg(rgb(t.bg_secondary)).text_color(rgb(t.text_muted))
        })
        .hover(|s: StyleRefinement| s.bg(rgb(t.bg_hover)))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(if active_count > 0 {
            format!("filters ({})", active_count)
        } else {
            "filters".to_string()
        })
        .child(canvas(on_bounds, |_, _, _, _| {}).absolute().size_full())
}

/// Render the file filter popover panel anchored below the given button bounds.
///
/// Returns a `Deferred` element. The caller is responsible for rendering a backdrop
/// and conditionally showing this popover — see settings panel dropdowns for the pattern.
/// `on_toggle` is called with `"ignored"` when the option is clicked.
pub fn file_filter_popover(
    bounds: Bounds<Pixels>,
    show_ignored: bool,
    t: &okena_core::theme::ThemeColors,
    cx: &App,
    on_toggle: impl Fn(&str, &mut Window, &mut App) + 'static,
) -> Deferred {
    use okena_ui::popover::popover_panel;

    deferred(
        anchored()
            .position(point(
                bounds.origin.x,
                bounds.origin.y + bounds.size.height + px(2.0),
            ))
            .snap_to_window()
            .child(
                popover_panel("filter-popover", t)
                    .min_w(px(180.0))
                    .py(px(4.0))
                    .child(file_filter_option(
                        "ignored",
                        "Include gitignored",
                        show_ignored,
                        t,
                        cx,
                        move |_, window, cx| on_toggle("ignored", window, cx),
                    )),
            ),
    )
}

fn file_filter_option(
    id: &str,
    label: &str,
    active: bool,
    t: &okena_core::theme::ThemeColors,
    cx: &App,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    use okena_ui::tokens::ui_text_sm;

    div()
        .id(ElementId::Name(format!("filter-{}", id).into()))
        .px(px(10.0))
        .py(px(6.0))
        .cursor_pointer()
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_primary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .flex()
        .items_center()
        .justify_between()
        .child(label.to_string())
        .when(active, |d: Stateful<Div>| {
            d.child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.border_active))
                    .child("✓"),
            )
        })
        .on_mouse_down(MouseButton::Left, on_click)
}
