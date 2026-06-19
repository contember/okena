//! In-page search ("search in page") for the diff viewer — thin glue over the
//! shared [`okena_files::in_page_search`] engine.
//!
//! Cell scheme (the host owns the cell → view mapping):
//! - **Unified**: one cell per `current_file.items` entry; cell id = item index =
//!   `uniform_list` scroll item. Expanders contribute an empty (never-matching)
//!   cell so indices stay aligned with the rendered list.
//! - **Side-by-side**: two cells per row — `row*2 + 0` (left), `row*2 + 1`
//!   (right); scroll item = `cell / 2`.

use gpui::*;
use okena_core::theme::ThemeColors;
use okena_core::types::DiffViewMode;
use okena_files::in_page_search::{self, InPageSearch, SearchBarCallbacks, SearchMatch};
use okena_ui::simple_input::InputChangedEvent;
use std::ops::Range;
use std::rc::Rc;

use super::types::{DisplayItem, SideBySideSide};
use super::DiffViewer;

impl DiffViewer {
    /// Open (or refocus) the in-page search bar.
    pub(super) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ref search) = self.search {
            search.input.update(cx, |input, cx| {
                input.select_all(cx);
                input.focus(window, cx);
            });
            return;
        }
        let search = InPageSearch::new(None, window, cx);
        cx.subscribe(
            &search.input,
            |_this: &mut Self, _, _: &InputChangedEvent, cx| {
                // Recompute happens in render via the signature check; just
                // request a re-render here.
                cx.notify();
            },
        )
        .detach();
        self.search = Some(search);
        self.search_sig = None;
        cx.notify();
    }

    /// Close search and return focus to the diff viewer.
    pub(super) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        self.search_sig = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    pub(super) fn next_search_match(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.search.as_mut() {
            search.next_match();
        }
        self.scroll_to_search_match();
        cx.notify();
    }

    pub(super) fn prev_search_match(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.search.as_mut() {
            search.prev_match();
        }
        self.scroll_to_search_match();
        cx.notify();
    }

    pub(super) fn toggle_search_case(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.search.as_mut() {
            search.toggle_case();
        }
        self.search_sig = None; // case is part of the signature
        cx.notify();
    }

    fn scroll_to_search_match(&self) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        let Some(cell) = search.current_cell() else {
            return;
        };
        let item = match self.effective_view_mode() {
            DiffViewMode::Unified => cell,
            DiffViewMode::SideBySide => cell / 2,
        };
        self.scroll_handle.scroll_to_item(item, ScrollStrategy::Center);
    }

    /// Recompute matches if content / query / case changed, then render the bar.
    /// Returns `None` when search is closed.
    pub(super) fn build_search_bar(
        &mut self,
        view_mode: DiffViewMode,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (query, case) = self
            .search
            .as_ref()
            .map(|s| (s.input.read(cx).value().to_string(), s.case_sensitive()))?;

        let items_len = self.current_file.as_ref().map(|f| f.items.len()).unwrap_or(0);
        let sbs_len = self.side_by_side_lines.len();
        let sig = (
            self.selected_file_index,
            self.commit_index,
            self.diff_mode.clone(),
            self.ignore_whitespace,
            view_mode,
            items_len,
            sbs_len,
            query.clone(),
            case,
        );
        if self.search_sig.as_ref() != Some(&sig) {
            let matches = self.compute_matches(view_mode, &query, case);
            if let Some(search) = self.search.as_mut() {
                search.set_matches(matches);
            }
            self.search_sig = Some(sig);
        }

        let search = self.search.as_ref()?;
        Some(in_page_search::render_search_bar(
            search,
            t,
            cx,
            SearchBarCallbacks {
                on_next: Rc::new(|this: &mut Self, cx| this.next_search_match(cx)),
                on_prev: Rc::new(|this: &mut Self, cx| this.prev_search_match(cx)),
                on_toggle_case: Rc::new(|this: &mut Self, cx| this.toggle_search_case(cx)),
                on_close: Rc::new(|this: &mut Self, window, cx| this.close_search(window, cx)),
            },
        ))
    }

    fn compute_matches(&self, view_mode: DiffViewMode, query: &str, case: bool) -> Vec<SearchMatch> {
        match view_mode {
            DiffViewMode::Unified => {
                let items = self
                    .current_file
                    .as_ref()
                    .map(|f| f.items.as_slice())
                    .unwrap_or(&[]);
                in_page_search::compute_matches(
                    query,
                    case,
                    items.iter().map(|it| match it {
                        DisplayItem::Line(l) => l.plain_text.as_str(),
                        DisplayItem::Expander(_) => "",
                    }),
                )
            }
            DiffViewMode::SideBySide => in_page_search::compute_matches(
                query,
                case,
                self.side_by_side_lines.iter().flat_map(|sbs| {
                    [
                        sbs.left.as_ref().map(|c| c.plain_text.as_str()).unwrap_or(""),
                        sbs.right.as_ref().map(|c| c.plain_text.as_str()).unwrap_or(""),
                    ]
                }),
            ),
        }
    }

    /// Search highlight ranges for a unified line (cell id = item index).
    pub(super) fn search_ranges_unified(
        &self,
        item_index: usize,
        t: &ThemeColors,
    ) -> Vec<(Range<usize>, Hsla)> {
        self.search
            .as_ref()
            .map(|s| s.ranges_for_cell(item_index, t))
            .unwrap_or_default()
    }

    /// Search highlight ranges for a side-by-side cell (cell id = row*2 + side).
    pub(super) fn search_ranges_sbs(
        &self,
        sbs_line_index: usize,
        side: SideBySideSide,
        t: &ThemeColors,
    ) -> Vec<(Range<usize>, Hsla)> {
        let side_idx = match side {
            SideBySideSide::Left => 0,
            SideBySideSide::Right => 1,
        };
        self.search
            .as_ref()
            .map(|s| s.ranges_for_cell(sbs_line_index * 2 + side_idx, t))
            .unwrap_or_default()
    }
}
