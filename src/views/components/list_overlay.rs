//! Generic list overlay component for searchable/navigable lists.
//!
//! Provides shared infrastructure for overlays like:
//! - Command palette (searchable)
//! - Project switcher (searchable with extra actions)
//! - Theme selector (no search)
//! - Shell selector (no search)
//! - File search (fuzzy search with scoring)

use gpui::*;

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
    /// Placeholder text for the search input. If None, search is disabled.
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

    /// Check if search is enabled.
    pub fn has_search(&self) -> bool {
        self.search_placeholder.is_some()
    }
}

/// Result of filtering an item, with optional match metadata.
#[derive(Clone, Debug, Default)]
pub struct FilterResult<M: Clone + Default = ()> {
    /// Original index in the items list.
    pub index: usize,
    /// Optional match metadata (e.g., matched positions for highlighting).
    pub match_data: M,
}

impl<M: Clone + Default> FilterResult<M> {
    pub fn new(index: usize) -> Self {
        Self {
            index,
            match_data: M::default(),
        }
    }

    pub fn with_match_data(index: usize, match_data: M) -> Self {
        Self { index, match_data }
    }
}

/// Shared state for list overlays.
pub struct ListOverlayState<T: Clone, M: Clone + Default = ()> {
    /// Focus handle for keyboard events.
    pub focus_handle: FocusHandle,
    /// Scroll handle for list scrolling.
    pub scroll_handle: ScrollHandle,
    /// All items in the list.
    pub items: Vec<T>,
    /// Filtered items (indices + match data).
    pub filtered: Vec<FilterResult<M>>,
    /// Currently selected index (into filtered list).
    pub selected_index: usize,
    /// Current search query.
    pub search_query: String,
    /// Configuration.
    pub config: ListOverlayConfig,
}

impl<T: Clone, M: Clone + Default> ListOverlayState<T, M> {
    /// Create a new state with the given items and config.
    pub fn new(items: Vec<T>, config: ListOverlayConfig, cx: &mut App) -> Self {
        let filtered: Vec<FilterResult<M>> = (0..items.len())
            .map(FilterResult::new)
            .collect();

        Self {
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            items,
            filtered,
            selected_index: 0,
            search_query: String::new(),
            config,
        }
    }

    /// Create a new state with a pre-selected index.
    pub fn with_selected(items: Vec<T>, config: ListOverlayConfig, selected_index: usize, cx: &mut App) -> Self {
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

    /// Get the currently selected filter result with match data.
    pub fn selected_filter_result(&self) -> Option<&FilterResult<M>> {
        self.filtered.get(self.selected_index)
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

    /// Add a character to the search query.
    pub fn push_search_char(&mut self, ch: char) {
        self.search_query.push(ch);
    }

    /// Remove the last character from the search query.
    pub fn pop_search_char(&mut self) -> bool {
        if !self.search_query.is_empty() {
            self.search_query.pop();
            true
        } else {
            false
        }
    }

    /// Update the filtered list and reset selection to first item.
    pub fn set_filtered(&mut self, filtered: Vec<FilterResult<M>>) {
        self.filtered = filtered;
        self.selected_index = 0;
    }

    /// Check if the list is empty after filtering.
    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    /// Get the number of filtered items.
    pub fn len(&self) -> usize {
        self.filtered.len()
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
    /// Search query changed (character added or removed).
    QueryChanged,
    /// A custom action was triggered (e.g., "space" for toggle visibility).
    Custom(String),
    /// No action taken.
    None,
}

/// Characters allowed in search queries.
const SEARCH_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -_./";

/// Handle keyboard events for a list overlay.
///
/// # Arguments
///
/// * `state` - The list overlay state
/// * `event` - The key down event
/// * `extra_keys` - Additional key handlers as (key, action_name) pairs
///
/// # Returns
///
/// The action to take in response to the key event.
pub fn handle_list_overlay_key<T: Clone, M: Clone + Default>(
    state: &mut ListOverlayState<T, M>,
    event: &KeyDownEvent,
    extra_keys: &[(&str, &str)],
) -> ListOverlayAction {
    let key = event.keystroke.key.as_str();

    // Check extra keys first
    for &(k, action) in extra_keys {
        if key == k {
            return ListOverlayAction::Custom(action.to_string());
        }
    }

    match key {
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
        "backspace" => {
            if state.config.has_search() && state.pop_search_char() {
                ListOverlayAction::QueryChanged
            } else {
                ListOverlayAction::None
            }
        }
        key if key.len() == 1 && state.config.has_search() => {
            let ch = key.chars().next().unwrap();
            if SEARCH_CHARS.contains(ch) {
                state.push_search_char(ch);
                ListOverlayAction::QueryChanged
            } else {
                ListOverlayAction::None
            }
        }
        _ => ListOverlayAction::None,
    }
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
