//! In-file search for the file viewer.

use crate::file_search::Cancel;
use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::icon_button::icon_button_sized;
use okena_ui::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use okena_ui::tokens::ui_text_md;
use std::ops::Range;

use super::FileViewer;

/// Background color for non-current search matches.
const SEARCH_MATCH_BG: Rgba = Rgba {
    r: 1.0,
    g: 0.85,
    b: 0.0,
    a: 0.18,
};

/// Background color for the current (active) search match.
const SEARCH_CURRENT_MATCH_BG: Rgba = Rgba {
    r: 1.0,
    g: 0.6,
    b: 0.0,
    a: 0.4,
};

/// A single match location in the file.
pub(crate) struct SearchMatch {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
}

/// State for the in-file search bar.
pub(crate) struct FileSearchState {
    pub input: Entity<SimpleInputState>,
    pub matches: Vec<SearchMatch>,
    pub current_match_index: Option<usize>,
    pub case_sensitive: bool,
}

impl FileViewer {
    /// Open the in-file search bar. If already open, refocus and select all.
    pub(super) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.active_tab();
        if tab.is_empty() {
            return;
        }

        // If search is already open, just refocus and select all
        if let Some(ref state) = self.search_state {
            state.input.update(cx, |input, cx| {
                input.select_all(cx);
                input.focus(window, cx);
            });
            return;
        }

        // Pre-fill with selected text if any
        let selected_text = self.get_selected_text().unwrap_or_default();

        let input = cx.new(|cx| {
            let mut state = SimpleInputState::new(cx)
                .placeholder("Search...")
                .icon("icons/search.svg");
            if !selected_text.is_empty() {
                // Take only the first line of selection
                let first_line = selected_text.lines().next().unwrap_or("");
                state.set_value(first_line, cx);
            }
            state
        });

        input.update(cx, |input, cx| {
            input.focus(window, cx);
        });

        cx.subscribe(&input, |this: &mut Self, _, _: &InputChangedEvent, cx| {
            this.perform_file_search(cx);
        })
        .detach();

        self.search_state = Some(FileSearchState {
            input,
            matches: Vec::new(),
            current_match_index: None,
            case_sensitive: false,
        });

        self.perform_file_search(cx);
        cx.notify();
    }

    /// Close the search bar and clear highlights.
    pub(super) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_state = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Run the search against the active tab's content.
    pub(super) fn perform_file_search(&mut self, cx: &mut Context<Self>) {
        let state = match self.search_state.as_mut() {
            Some(s) => s,
            None => return,
        };

        let query = state.input.read(cx).value().to_string();
        if query.is_empty() {
            state.matches.clear();
            state.current_match_index = None;
            cx.notify();
            return;
        }

        let case_sensitive = state.case_sensitive;
        let query_lower = if case_sensitive {
            query.clone()
        } else {
            query.to_lowercase()
        };

        let tab = self.active_tab();
        let mut matches = Vec::new();

        for (line_idx, line) in tab.highlighted_lines.iter().enumerate() {
            let text = &line.plain_text;
            let search_text = if case_sensitive {
                text.clone()
            } else {
                text.to_lowercase()
            };

            let mut start = 0;
            while let Some(pos) = search_text[start..].find(&query_lower) {
                let abs_pos = start + pos;
                matches.push(SearchMatch {
                    line: line_idx,
                    start_col: abs_pos,
                    end_col: abs_pos + query.len(),
                });
                start = abs_pos + 1;
            }
        }

        let current_match_index = if matches.is_empty() {
            None
        } else {
            Some(0)
        };

        if let Some(state) = self.search_state.as_mut() {
            state.matches = matches;
            state.current_match_index = current_match_index;
        }

        self.scroll_to_current_search_match();
        cx.notify();
    }

    /// Navigate to the next search match.
    pub(super) fn next_search_match(&mut self, cx: &mut Context<Self>) {
        let state = match self.search_state.as_mut() {
            Some(s) => s,
            None => return,
        };
        if state.matches.is_empty() {
            return;
        }
        let next = match state.current_match_index {
            Some(idx) => (idx + 1) % state.matches.len(),
            None => 0,
        };
        state.current_match_index = Some(next);
        self.scroll_to_current_search_match();
        cx.notify();
    }

    /// Navigate to the previous search match.
    pub(super) fn prev_search_match(&mut self, cx: &mut Context<Self>) {
        let state = match self.search_state.as_mut() {
            Some(s) => s,
            None => return,
        };
        if state.matches.is_empty() {
            return;
        }
        let prev = match state.current_match_index {
            Some(idx) => {
                if idx == 0 {
                    state.matches.len() - 1
                } else {
                    idx - 1
                }
            }
            None => state.matches.len() - 1,
        };
        state.current_match_index = Some(prev);
        self.scroll_to_current_search_match();
        cx.notify();
    }

    /// Scroll the active tab to make the current search match visible.
    fn scroll_to_current_search_match(&self) {
        let state = match self.search_state.as_ref() {
            Some(s) => s,
            None => return,
        };
        if let Some(idx) = state.current_match_index {
            if let Some(m) = state.matches.get(idx) {
                self.active_tab()
                    .source_scroll_handle
                    .scroll_to_item(m.line, ScrollStrategy::Top);
            }
        }
    }

    /// Toggle case sensitivity and re-run search.
    pub(super) fn toggle_search_case_sensitive(&mut self, cx: &mut Context<Self>) {
        if let Some(ref mut state) = self.search_state {
            state.case_sensitive = !state.case_sensitive;
        }
        self.perform_file_search(cx);
    }

    /// Get background highlight ranges for search matches on a given line.
    pub(super) fn search_bg_ranges_for_line(&self, line_index: usize) -> Vec<(Range<usize>, Hsla)> {
        let state = match self.search_state.as_ref() {
            Some(s) => s,
            None => return Vec::new(),
        };

        let current_idx = state.current_match_index;
        state
            .matches
            .iter()
            .enumerate()
            .filter(|(_, m)| m.line == line_index)
            .map(|(i, m)| {
                let color = if Some(i) == current_idx {
                    SEARCH_CURRENT_MATCH_BG
                } else {
                    SEARCH_MATCH_BG
                };
                (m.start_col..m.end_col, color.into())
            })
            .collect()
    }

    /// Render the search bar UI.
    pub(super) fn render_search_bar(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(state) = self.search_state.as_ref() else {
            return div().id("file-search-bar-empty").into_any_element();
        };
        let match_count = state.matches.len();
        let current_idx = state.current_match_index.map(|i| i + 1).unwrap_or(0);
        let match_text = if match_count > 0 {
            format!("{}/{}", current_idx, match_count)
        } else {
            "0/0".to_string()
        };
        let case_sensitive = state.case_sensitive;
        let input = &state.input;

        div()
            .id("file-search-bar")
            .h(px(36.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .bg(rgb(t.bg_header))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .id("file-search-input-wrapper")
                    .key_context("FileViewerSearch")
                    .flex_1()
                    .min_w(px(100.0))
                    .max_w(px(300.0))
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.border_active))
                    .rounded(px(4.0))
                    .child(SimpleInput::new(input).text_size(ui_text_md(cx)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_action(
                        cx.listener(|this, _: &Cancel, window, cx| {
                            this.close_search(window, cx);
                        }),
                    )
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        cx.stop_propagation();
                        match event.keystroke.key.as_str() {
                            "enter" => {
                                if event.keystroke.modifiers.shift {
                                    this.prev_search_match(cx);
                                } else {
                                    this.next_search_match(cx);
                                }
                            }
                            _ => {}
                        }
                    })),
            )
            .child(
                div()
                    .id("file-search-case-btn")
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
                        this.toggle_search_case_sensitive(cx);
                    }))
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if case_sensitive {
                                rgb(t.text_primary)
                            } else {
                                rgb(t.text_secondary)
                            })
                            .child("Aa"),
                    ),
            )
            .child(
                div()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_secondary))
                    .min_w(px(40.0))
                    .child(match_text),
            )
            .child(
                icon_button_sized("file-search-prev-btn", "icons/chevron-up.svg", 24.0, 14.0, t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.prev_search_match(cx);
                    })),
            )
            .child(
                icon_button_sized(
                    "file-search-next-btn",
                    "icons/chevron-down.svg",
                    24.0,
                    14.0,
                    t,
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.next_search_match(cx);
                })),
            )
            .child(
                icon_button_sized("file-search-close-btn", "icons/close.svg", 24.0, 14.0, t)
                    .hover(|s| s.bg(gpui::rgba(0xf14c4c99)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_search(window, cx);
                    })),
            )
            .into_any_element()
    }
}
