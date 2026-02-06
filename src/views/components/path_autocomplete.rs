use crate::theme::theme;
use crate::views::components::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use std::path::PathBuf;

/// A suggestion for path auto-completion
#[derive(Clone, Debug)]
pub struct PathSuggestion {
    /// Directory name only (for display)
    pub display_name: String,
    /// Complete path
    pub full_path: String,
    /// Whether this is a directory
    pub is_directory: bool,
}

/// Path auto-complete state wrapping SimpleInputState
pub struct PathAutoCompleteState {
    input: Entity<SimpleInputState>,
    suggestions: Vec<PathSuggestion>,
    selected_index: usize,
    show_suggestions: bool,
    focus_handle: FocusHandle,
    /// Scroll handle for suggestions dropdown
    suggestions_scroll: ScrollHandle,
}

impl PathAutoCompleteState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Enter path...")
        });

        let focus_handle = cx.focus_handle();

        // Subscribe to input changes to update suggestions
        let input_for_subscription = input.clone();
        cx.subscribe(&input_for_subscription, |this, _, _event: &InputChangedEvent, cx| {
            this.on_input_changed(cx);
        }).detach();

        Self {
            input,
            suggestions: Vec::new(),
            selected_index: 0,
            show_suggestions: false,
            focus_handle,
            suggestions_scroll: ScrollHandle::new(),
        }
    }

    pub fn value(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    pub fn set_value(&mut self, value: impl Into<String>, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value(value, cx);
        });
        self.update_suggestions(cx);
    }

    #[allow(dead_code)]
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.focus(window, cx);
        });
    }

    #[allow(dead_code)]
    pub fn input(&self) -> &Entity<SimpleInputState> {
        &self.input
    }

    /// Returns true if suggestions should be shown
    pub fn has_suggestions(&self) -> bool {
        self.show_suggestions && !self.suggestions.is_empty()
    }

    /// Get the suggestions for external rendering
    pub fn suggestions(&self) -> &[PathSuggestion] {
        &self.suggestions
    }

    /// Get the selected index
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Get the scroll handle for the suggestions dropdown
    pub fn suggestions_scroll(&self) -> &ScrollHandle {
        &self.suggestions_scroll
    }

    /// Select a suggestion by index and complete it
    pub fn select_and_complete(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_index = index;
        self.complete_selected(cx);
    }

    /// Expand ~ to home directory
    fn expand_path(path: &str) -> String {
        if path.starts_with('~') {
            if let Some(home) = dirs::home_dir() {
                let rest = path.strip_prefix('~').unwrap_or("");
                return format!("{}{}", home.display(), rest);
            }
        }
        path.to_string()
    }

    /// Get the directory to list and the prefix to filter by
    fn parse_path_for_completion(path: &str) -> (PathBuf, String) {
        let expanded = Self::expand_path(path);
        let path_buf = PathBuf::from(&expanded);

        if expanded.ends_with('/') || expanded.is_empty() {
            // List directory contents
            (path_buf, String::new())
        } else if path_buf.is_dir() {
            // If it's an existing directory without trailing slash, still list its contents
            (path_buf, String::new())
        } else {
            // Get parent directory and use filename as prefix filter
            let parent = path_buf.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
            let prefix = path_buf
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from)
                .unwrap_or_default();
            (parent, prefix)
        }
    }

    fn on_input_changed(&mut self, cx: &mut Context<Self>) {
        self.update_suggestions(cx);
    }

    fn update_suggestions(&mut self, cx: &mut Context<Self>) {
        let current_value = self.input.read(cx).value().to_string();

        // Don't show suggestions for empty input
        if current_value.is_empty() {
            self.suggestions.clear();
            self.show_suggestions = false;
            self.selected_index = 0;
            cx.notify();
            return;
        }

        let (dir_path, prefix) = Self::parse_path_for_completion(&current_value);

        let mut new_suggestions = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&dir_path) {
            for entry in entries.filter_map(|e| e.ok()) {
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy().to_string();

                // Skip hidden files unless user typed a dot
                if name.starts_with('.') && !prefix.starts_with('.') {
                    continue;
                }

                // Filter by prefix (case-insensitive)
                if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix.to_lowercase()) {
                    continue;
                }

                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let full_path = entry.path().to_string_lossy().to_string();

                // Convert back to use ~ if the original path used it
                let display_full_path = if current_value.starts_with('~') {
                    if let Some(home) = dirs::home_dir() {
                        let home_str = home.to_string_lossy().to_string();
                        if full_path.starts_with(&home_str) {
                            format!("~{}", &full_path[home_str.len()..])
                        } else {
                            full_path.clone()
                        }
                    } else {
                        full_path.clone()
                    }
                } else {
                    full_path.clone()
                };

                new_suggestions.push(PathSuggestion {
                    display_name: name,
                    full_path: display_full_path,
                    is_directory: is_dir,
                });
            }
        }

        // Sort: directories first, then alphabetically
        new_suggestions.sort_by(|a, b| {
            match (a.is_directory, b.is_directory) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()),
            }
        });

        // Limit suggestions
        new_suggestions.truncate(10);

        self.suggestions = new_suggestions;
        self.show_suggestions = !self.suggestions.is_empty();
        self.selected_index = 0;
        cx.notify();
    }

    fn complete_selected(&mut self, cx: &mut Context<Self>) {
        // Clone suggestion data before borrowing self mutably
        let suggestion_data = self.suggestions.get(self.selected_index)
            .map(|s| (s.full_path.clone(), s.is_directory));

        if let Some((full_path, is_directory)) = suggestion_data {
            let mut path = full_path;
            if is_directory && !path.ends_with('/') {
                path.push('/');
            }
            self.input.update(cx, |input, cx| {
                input.set_value(&path, cx);
            });
            self.suggestions.clear();
            self.show_suggestions = false;
            self.selected_index = 0;

            // If it's a directory, update suggestions for the new path
            if is_directory {
                self.update_suggestions(cx);
            }
        }
        cx.notify();
    }

    fn select_previous(&mut self, cx: &mut Context<Self>) {
        if !self.suggestions.is_empty() && self.selected_index > 0 {
            self.selected_index -= 1;
            self.scroll_to_selected();
            cx.notify();
        }
    }

    fn select_next(&mut self, cx: &mut Context<Self>) {
        if !self.suggestions.is_empty() && self.selected_index < self.suggestions.len() - 1 {
            self.selected_index += 1;
            self.scroll_to_selected();
            cx.notify();
        }
    }

    /// Scroll to make the selected item visible
    fn scroll_to_selected(&self) {
        let item_height = px(26.0); // py(6px) * 2 + content
        // Use smaller visible height to account for container borders/padding
        // and ensure item is fully visible before it would be cut off
        let visible_height = px(148.0); // ~5.5 items visible at a time
        let zero = px(0.0);

        let selected_top = px(self.selected_index as f32 * 26.0);
        let selected_bottom = selected_top + item_height;

        // In gpui, scroll offset is negative when scrolled down
        // offset.y = 0 means top of content visible
        // offset.y = -100 means scrolled down 100px
        let current_offset = self.suggestions_scroll.offset().y;
        let scroll_top = zero - current_offset; // Convert to positive "how far scrolled"
        let scroll_bottom = scroll_top + visible_height;

        // Scroll down if selected item is at or below visible area
        if selected_bottom > scroll_bottom {
            let new_scroll = selected_bottom - visible_height;
            self.suggestions_scroll.set_offset(point(zero, zero - new_scroll));
        }
        // Scroll up if selected item is above visible area
        else if selected_top < scroll_top {
            self.suggestions_scroll.set_offset(point(zero, zero - selected_top));
        }
    }

    fn hide_suggestions(&mut self, cx: &mut Context<Self>) {
        self.show_suggestions = false;
        self.suggestions.clear();
        self.selected_index = 0;
        cx.notify();
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let key = event.keystroke.key.as_str();

        match key {
            "tab" => {
                if self.show_suggestions && !self.suggestions.is_empty() {
                    self.complete_selected(cx);
                    return true;
                }
            }
            "up" => {
                if self.show_suggestions {
                    self.select_previous(cx);
                    return true;
                }
            }
            "down" => {
                if self.show_suggestions {
                    self.select_next(cx);
                    return true;
                }
            }
            "escape" => {
                if self.show_suggestions {
                    self.hide_suggestions(cx);
                    return true;
                }
            }
            "enter" => {
                if self.show_suggestions && !self.suggestions.is_empty() {
                    self.complete_selected(cx);
                    return true;
                }
            }
            _ => {}
        }

        false
    }
}

impl Render for PathAutoCompleteState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Only render the input field - suggestions dropdown is rendered separately
        // at the parent level (sidebar) to ensure proper z-ordering
        div()
            .w_full()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                // Stop all key events from bubbling to prevent terminal interference
                cx.stop_propagation();
                this.handle_key_down(event, cx);
            }))
            .child(
                div()
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(4.0))
                    .child(
                        SimpleInput::new(&self.input)
                            .text_size(px(12.0))
                    )
            )
    }
}

impl Focusable for PathAutoCompleteState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
