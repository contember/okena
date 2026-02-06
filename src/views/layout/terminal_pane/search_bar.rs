//! Search bar component for terminal pane.
//!
//! An Entity with Render that handles search input, match navigation, and options.

use crate::elements::terminal_element::SearchMatch;
use crate::terminal::terminal::Terminal;
use crate::theme::theme;
use crate::views::simple_input::{SimpleInput, SimpleInputState};
use crate::workspace::state::Workspace;
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::Arc;

/// Events emitted by SearchBar.
#[derive(Clone)]
pub enum SearchBarEvent {
    /// Search bar was closed
    Closed,
    /// Matches changed (matches, current_index)
    MatchesChanged(Arc<Vec<SearchMatch>>, Option<usize>),
}

impl EventEmitter<SearchBarEvent> for SearchBar {}

/// Search bar view for terminal search functionality.
pub struct SearchBar {
    /// Reference to workspace for focus management
    workspace: Entity<Workspace>,
    /// Reference to terminal for searching
    terminal: Option<Arc<Terminal>>,
    /// Search input state
    input: Option<Entity<SimpleInputState>>,
    /// Search matches
    matches: Arc<Vec<SearchMatch>>,
    /// Current match index
    current_match_index: Option<usize>,
    /// Case sensitive search
    case_sensitive: bool,
    /// Regex search
    use_regex: bool,
    /// Whether search is active
    is_active: bool,
}

impl SearchBar {
    pub fn new(workspace: Entity<Workspace>, _cx: &mut Context<Self>) -> Self {
        Self {
            workspace,
            terminal: None,
            input: None,
            matches: Arc::new(Vec::new()),
            current_match_index: None,
            case_sensitive: false,
            use_regex: false,
            is_active: false,
        }
    }

    /// Set the terminal reference.
    pub fn set_terminal(&mut self, terminal: Option<Arc<Terminal>>) {
        self.terminal = terminal;
    }

    /// Check if search is active.
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// Open search bar.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.is_active = true;
        let input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Search...")
                .icon("icons/search.svg")
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
        });
        self.input = Some(input);
        self.matches = Arc::new(Vec::new());
        self.current_match_index = None;

        // Clear focused terminal to prevent stealing focus back
        self.workspace.update(cx, |ws, cx| {
            ws.clear_focused_terminal(cx);
        });
        cx.notify();
    }

    /// Close search bar.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.is_active = false;
        self.input = None;
        self.matches = Arc::new(Vec::new());
        self.current_match_index = None;

        // Restore focus after closing
        self.workspace.update(cx, |ws, cx| {
            ws.restore_focused_terminal(cx);
        });

        cx.emit(SearchBarEvent::Closed);
        cx.notify();
    }

    /// Perform search with current query.
    pub fn perform_search(&mut self, cx: &mut Context<Self>) {
        let query = self.input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();

        if let Some(ref terminal) = self.terminal {
            let matches = terminal.search_grid(&query, self.case_sensitive, self.use_regex);
            let search_matches: Vec<SearchMatch> = matches
                .into_iter()
                .map(|(line, col, len)| SearchMatch { line, col, len })
                .collect();

            self.current_match_index = if !search_matches.is_empty() { Some(0) } else { None };
            self.matches = Arc::new(search_matches);

            cx.emit(SearchBarEvent::MatchesChanged(
                self.matches.clone(),
                self.current_match_index,
            ));
        }
        cx.notify();
    }

    /// Toggle case sensitivity.
    fn toggle_case_sensitive(&mut self, cx: &mut Context<Self>) {
        self.case_sensitive = !self.case_sensitive;
        self.perform_search(cx);
    }

    /// Toggle regex mode.
    fn toggle_regex(&mut self, cx: &mut Context<Self>) {
        self.use_regex = !self.use_regex;
        self.perform_search(cx);
    }

    /// Navigate to next match.
    pub fn next_match(&mut self, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }

        let next_idx = match self.current_match_index {
            Some(idx) => (idx + 1) % self.matches.len(),
            None => 0,
        };
        self.current_match_index = Some(next_idx);
        self.scroll_to_current_match();

        cx.emit(SearchBarEvent::MatchesChanged(
            self.matches.clone(),
            self.current_match_index,
        ));
        cx.notify();
    }

    /// Navigate to previous match.
    pub fn prev_match(&mut self, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }

        let prev_idx = match self.current_match_index {
            Some(idx) => {
                if idx == 0 {
                    self.matches.len() - 1
                } else {
                    idx - 1
                }
            }
            None => self.matches.len() - 1,
        };
        self.current_match_index = Some(prev_idx);
        self.scroll_to_current_match();

        cx.emit(SearchBarEvent::MatchesChanged(
            self.matches.clone(),
            self.current_match_index,
        ));
        cx.notify();
    }

    /// Scroll terminal to show current match.
    fn scroll_to_current_match(&self) {
        if let (Some(idx), Some(ref terminal)) = (self.current_match_index, &self.terminal) {
            if let Some(search_match) = self.matches.get(idx) {
                let screen_lines = terminal.screen_lines() as i32;
                let match_line = search_match.line;

                if match_line < 0 || match_line >= screen_lines {
                    let target_visible_line = screen_lines / 2;
                    let scroll_delta = target_visible_line - match_line;

                    if scroll_delta > 0 {
                        terminal.scroll_up(scroll_delta);
                    } else if scroll_delta < 0 {
                        terminal.scroll_down(-scroll_delta);
                    }
                }
            }
        }
    }

    /// Handle key down in search input.
    fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => {
                if event.keystroke.modifiers.shift {
                    self.prev_match(cx);
                } else {
                    self.next_match(cx);
                }
            }
            "escape" => {
                self.close(cx);
            }
            _ => {
                // Update search on text change
                self.perform_search(cx);
            }
        }
    }
}

impl Render for SearchBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let match_count = self.matches.len();
        let current_idx = self.current_match_index.map(|i| i + 1).unwrap_or(0);
        let match_text = if match_count > 0 {
            format!("{}/{}", current_idx, match_count)
        } else {
            "0/0".to_string()
        };
        let case_sensitive = self.case_sensitive;
        let is_regex = self.use_regex;

        div()
            .id("search-bar")
            .h(px(36.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .bg(rgb(t.bg_header))
            .child(
                if let Some(ref input) = self.input {
                    div()
                        .id("search-input-wrapper")
                        .flex_1()
                        .min_w(px(100.0))
                        .max_w(px(300.0))
                        .bg(rgb(t.bg_secondary))
                        .border_1()
                        .border_color(rgb(t.border_active))
                        .rounded(px(4.0))
                        .child(SimpleInput::new(input).text_size(px(12.0)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                            // Stop propagation for all keys to prevent terminal interference
                            cx.stop_propagation();
                            this.handle_key_down(event, cx);
                        }))
                        .into_any_element()
                } else {
                    div().flex_1().into_any_element()
                },
            )
            // Case-sensitive toggle
            .child(
                div()
                    .id("search-case-sensitive-btn")
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .when(case_sensitive, |s| s.bg(rgb(t.bg_selection)))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.toggle_case_sensitive(cx);
                    }))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if case_sensitive {
                                rgb(t.text_primary)
                            } else {
                                rgb(t.text_secondary)
                            })
                            .child("Aa"),
                    ),
            )
            // Regex toggle
            .child(
                div()
                    .id("search-regex-btn")
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .when(is_regex, |s| s.bg(rgb(t.bg_selection)))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.toggle_regex(cx);
                    }))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if is_regex {
                                rgb(t.text_primary)
                            } else {
                                rgb(t.text_secondary)
                            })
                            .child(".*"),
                    ),
            )
            // Match counter
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.text_secondary))
                    .min_w(px(40.0))
                    .child(match_text),
            )
            // Previous match button
            .child(
                div()
                    .id("search-prev-btn")
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.prev_match(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/chevron-up.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary)),
                    ),
            )
            // Next match button
            .child(
                div()
                    .id("search-next-btn")
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.next_match(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/chevron-down.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary)),
                    ),
            )
            // Close button
            .child(
                div()
                    .id("search-close-btn")
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgba(0xf14c4c99)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.close(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/close.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary)),
                    ),
            )
    }
}
