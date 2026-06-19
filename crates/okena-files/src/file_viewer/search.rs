//! In-file search for the file viewer — thin glue over the shared
//! [`crate::in_page_search`] engine. Each cell is a source line; the cell id is
//! the line index, which is also the `source_scroll_handle` item index.

use crate::in_page_search::{self, InPageSearch, SearchBarCallbacks};
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::simple_input::InputChangedEvent;
use std::ops::Range;
use std::rc::Rc;

use super::FileViewer;

impl FileViewer {
    /// Open the in-file search bar. If already open, refocus and select all.
    pub(super) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab().is_empty() {
            return;
        }

        // If search is already open, just refocus and select all
        if let Some(ref search) = self.search_state {
            search.input.update(cx, |input, cx| {
                input.select_all(cx);
                input.focus(window, cx);
            });
            return;
        }

        // Pre-fill with the current selection (first line only).
        let selected_text = self.get_selected_text();
        let search = InPageSearch::new(selected_text.as_deref(), window, cx);

        cx.subscribe(
            &search.input,
            |this: &mut Self, _, _: &InputChangedEvent, cx| {
                this.perform_file_search(cx);
            },
        )
        .detach();

        self.search_state = Some(search);
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
        let Some(search) = self.search_state.as_ref() else {
            return;
        };
        let query = search.input.read(cx).value().to_string();
        let case_sensitive = search.case_sensitive();

        // Compute into a local Vec so the `search` borrow is released before the
        // `active_tab` borrow.
        let matches = in_page_search::compute_matches(
            &query,
            case_sensitive,
            self.active_tab()
                .highlighted_lines
                .iter()
                .map(|l| l.plain_text.as_str()),
        );

        if let Some(search) = self.search_state.as_mut() {
            search.set_matches(matches);
        }
        self.scroll_to_current_search_match();
        cx.notify();
    }

    /// Navigate to the next search match.
    pub(super) fn next_search_match(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.search_state.as_mut() {
            search.next_match();
        }
        self.scroll_to_current_search_match();
        cx.notify();
    }

    /// Navigate to the previous search match.
    pub(super) fn prev_search_match(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.search_state.as_mut() {
            search.prev_match();
        }
        self.scroll_to_current_search_match();
        cx.notify();
    }

    /// Scroll the active tab to make the current search match visible.
    fn scroll_to_current_search_match(&self) {
        if let Some(search) = self.search_state.as_ref()
            && let Some(cell) = search.current_cell()
        {
            self.active_tab()
                .source_scroll_handle
                .scroll_to_item(cell, ScrollStrategy::Top);
        }
    }

    /// Toggle case sensitivity and re-run search.
    pub(super) fn toggle_search_case_sensitive(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.search_state.as_mut() {
            search.toggle_case();
        }
        self.perform_file_search(cx);
    }

    /// Get background highlight ranges for search matches on a given line.
    pub(super) fn search_bg_ranges_for_line(
        &self,
        line_index: usize,
        t: &ThemeColors,
    ) -> Vec<(Range<usize>, Hsla)> {
        match self.search_state.as_ref() {
            Some(search) => search.ranges_for_cell(line_index, t),
            None => Vec::new(),
        }
    }

    /// Render the search bar UI.
    pub(super) fn render_search_bar(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let Some(search) = self.search_state.as_ref() else {
            return div().id("file-search-bar-empty").into_any_element();
        };
        in_page_search::render_search_bar(
            search,
            t,
            cx,
            SearchBarCallbacks {
                on_next: Rc::new(|this: &mut Self, cx| this.next_search_match(cx)),
                on_prev: Rc::new(|this: &mut Self, cx| this.prev_search_match(cx)),
                on_toggle_case: Rc::new(|this: &mut Self, cx| {
                    this.toggle_search_case_sensitive(cx)
                }),
                on_close: Rc::new(|this: &mut Self, window, cx| this.close_search(window, cx)),
            },
        )
    }
}
