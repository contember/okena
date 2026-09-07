//! In-file search for the file viewer — thin glue over the shared
//! [`crate::in_page_search`] engine. Each cell is a rendered source row; the cell
//! id is also the `source_scroll_handle` item index.

use crate::in_page_search::{self, InPageSearch, SearchBarCallbacks, SearchMatch};
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::simple_input::InputChangedEvent;
use std::ops::Range;
use std::rc::Rc;

use super::{FileViewer, SourceRow};

fn map_matches_to_source_rows(matches: Vec<SearchMatch>, rows: &[SourceRow]) -> Vec<SearchMatch> {
    matches
        .into_iter()
        .filter_map(|match_| {
            let first = rows.partition_point(|row| row.logical_line < match_.cell);
            let end = rows.partition_point(|row| row.logical_line <= match_.cell);
            let matching_rows = rows.get(first..end)?;
            let local_row = matching_rows
                .partition_point(|row| row.byte_range.start <= match_.start)
                .saturating_sub(1);
            let row = matching_rows.get(local_row)?;
            Some(SearchMatch {
                cell: first + local_row,
                start: match_.start - row.byte_range.start,
                end: match_.end - row.byte_range.start,
            })
        })
        .collect()
}

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
        let logical_matches = in_page_search::compute_matches(
            &query,
            case_sensitive,
            self.active_tab()
                .highlighted_lines
                .iter()
                .map(|line| line.plain_text.as_str()),
        );
        let matches = map_matches_to_source_rows(logical_matches, &self.active_tab().source_rows);

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

#[cfg(test)]
mod tests {
    use super::map_matches_to_source_rows;
    use crate::file_viewer::SourceRow;
    use crate::in_page_search::SearchMatch;

    #[test]
    fn maps_matches_to_wrapped_rows_without_splitting_the_match() {
        let rows = vec![
            SourceRow {
                logical_line: 0,
                byte_range: 0..4,
                columns: 4,
            },
            SourceRow {
                logical_line: 0,
                byte_range: 4..8,
                columns: 4,
            },
            SourceRow {
                logical_line: 1,
                byte_range: 0..3,
                columns: 3,
            },
        ];
        let matches = map_matches_to_source_rows(
            vec![
                SearchMatch {
                    cell: 0,
                    start: 5,
                    end: 7,
                },
                SearchMatch {
                    cell: 0,
                    start: 3,
                    end: 6,
                },
                SearchMatch {
                    cell: 1,
                    start: 1,
                    end: 2,
                },
            ],
            &rows,
        );

        assert_eq!(matches.len(), 3);
        assert_eq!(
            (matches[0].cell, matches[0].start, matches[0].end),
            (1, 1, 3)
        );
        assert_eq!(
            (matches[1].cell, matches[1].start, matches[1].end),
            (0, 3, 6)
        );
        assert_eq!(
            (matches[2].cell, matches[2].start, matches[2].end),
            (2, 1, 2)
        );
    }
}
