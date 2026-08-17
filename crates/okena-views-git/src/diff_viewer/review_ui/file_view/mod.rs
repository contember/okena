//! File view: header, symbol bar, details, outline — spec §9.

mod bar;
mod header;
mod outline;
mod structure;
mod text;
mod token_diff;

use super::super::DiffViewer;
use super::super::line_render::{WORD_BG_ALPHA, rgba as tint};
use super::super::types::DiffViewMode;
use super::labels::format_signed;
use super::model::{FileEntry, Reason, ReasonKind};
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_review::{OutlineFact, StructuredFile};
use okena_ui::tokens::{ui_text_ms, ui_text_sm};

/// Under this width the header hides the language line and the symbol bar keeps
/// two chips — spec §12.
const NARROW_CONTENT: f32 = 1_000.0;

impl DiffViewer {
    pub(crate) fn render_file_header(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        header::render(self, t, cx)
    }

    pub(crate) fn render_symbol_bar(
        &mut self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        bar::render(self, t, cx)
    }

    pub(crate) fn render_outline_popover(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        outline::render(self, t, cx)
    }

    /// The open file's entry, once the model knows about it.
    pub(super) fn review_open_entry(&self) -> Option<&FileEntry> {
        let model = self.review_ui.model.as_ref()?;
        let key = self.smart_review.selected_file.as_ref()?;
        model
            .file_index(key)
            .and_then(|index| model.files.get(index))
    }

    /// The open file as structure analysis saw it.
    pub(super) fn review_open_structured_file(&self) -> Option<&StructuredFile> {
        let index = self.review_open_entry()?.structure_index?;
        self.smart_review.structure.ready()?.files().get(index)
    }

    /// Base and head outlines of the open file; `None` when both are empty.
    pub(super) fn review_open_outline(&self) -> Option<(&[OutlineFact], &[OutlineFact])> {
        let file = self.review_open_structured_file()?;
        let outlines = (file.old_outline(), file.new_outline());
        (!outlines.0.is_empty() || !outlines.1.is_empty()).then_some(outlines)
    }

    /// The changed symbol the bar names: the selection while it still holds,
    /// else the one the viewport is looking at — spec §9.
    pub(super) fn review_current_symbol_index(&self) -> Option<usize> {
        let entry = self.review_open_entry()?;
        if entry.symbols.is_empty() {
            return None;
        }
        let selected = self
            .review_ui
            .selected_symbol
            .as_ref()
            .filter(|symbol| symbol.file == entry.key)
            .and_then(|symbol| {
                entry
                    .symbols
                    .iter()
                    .position(|candidate| candidate.change_index == symbol.change_index)
            });
        let viewport = self.review_viewport();
        structure::followed_symbol(&entry.symbols, selected, &viewport)
    }

    /// The rows the diff list shows right now, as base/head lines.
    fn review_viewport(&self) -> structure::Viewport {
        let top = self.review_viewport_top();
        let bottom = self.review_viewport_bottom(top);
        structure::Viewport {
            top: self.review_row_lines(top),
            bottom: bottom.map(|row| self.review_row_lines(row)),
        }
    }

    fn review_row_lines(&self, row: usize) -> (Option<u32>, Option<u32>) {
        let items = self
            .current_file
            .as_ref()
            .map_or(&[][..], |file| file.items.as_slice());
        structure::top_row_lines(
            items,
            &self.side_by_side_lines,
            self.effective_view_mode(),
            row,
        )
    }

    /// Index of the first row the diff list shows.
    fn review_viewport_top(&self) -> usize {
        let item_count = self.review_diff_item_count();
        let state = self.scroll_handle.0.borrow();
        // A pending scroll is where the list is about to be, which is what the
        // bar should already name.
        if let Some(deferred) = state.deferred_scroll_to_item {
            return deferred.item_index.min(item_count.saturating_sub(1));
        }
        let Some(size) = state.last_item_size else {
            return 0;
        };
        structure::top_item_index(
            -f32::from(state.base_handle.offset().y),
            f32::from(size.contents.height),
            item_count,
        )
    }

    /// Index of the last row the diff list shows; `None` before the list has
    /// been laid out (or while a scroll is pending and the top is a guess).
    fn review_viewport_bottom(&self, top: usize) -> Option<usize> {
        let item_count = self.review_diff_item_count();
        let state = self.scroll_handle.0.borrow();
        if state.deferred_scroll_to_item.is_some() {
            return None;
        }
        let size = state.last_item_size?;
        let visible = structure::visible_rows(
            f32::from(state.base_handle.bounds().size.height),
            f32::from(size.contents.height),
            item_count,
        );
        Some(
            top.saturating_add(visible.saturating_sub(1))
                .min(item_count.checked_sub(1)?),
        )
    }

    /// How many rows the diff list renders in the mode currently on screen.
    fn review_diff_item_count(&self) -> usize {
        match self.effective_view_mode() {
            DiffViewMode::Unified => self
                .current_file
                .as_ref()
                .map_or(0, |file| file.items.len()),
            DiffViewMode::SideBySide => self.side_by_side_lines.len(),
        }
    }

    /// Spec §12: a narrow content column drops the labels that repeat elsewhere.
    pub(super) fn review_content_is_narrow(&self) -> bool {
        let width = self.review_ui.content_width;
        width > 0.0 && width < NARROW_CONTENT
    }
}

/// Chip tint per reason kind — spec §6 wording, spec §7 chip style.
pub(super) fn reason_tone(kind: ReasonKind, t: &ThemeColors) -> u32 {
    match kind {
        ReasonKind::PublicRemoved | ReasonKind::Removed | ReasonKind::DeletedImpl => {
            t.diff_removed_fg
        }
        ReasonKind::PublicSignature | ReasonKind::ExportedSignature => t.term_blue,
        ReasonKind::Calls => t.term_cyan,
        ReasonKind::New | ReasonKind::NewPublic => t.diff_added_fg,
        ReasonKind::Moved => t.term_magenta,
        ReasonKind::NoTestChanges | ReasonKind::Complex => t.warning,
        ReasonKind::LargeChurn => t.term_yellow,
        ReasonKind::Body
        | ReasonKind::CiConfig
        | ReasonKind::Lockfile
        | ReasonKind::Submodule
        | ReasonKind::Binary
        | ReasonKind::NotAnalyzed => t.text_muted,
    }
}

/// A reason chip: small, rounded, tinted by kind.
pub(super) fn chip(reason: &Reason, t: &ThemeColors, cx: &App) -> AnyElement {
    let tone = reason_tone(reason.kind, t);
    div()
        .flex_none()
        .px(px(5.0))
        .rounded(px(3.0))
        .bg(tint(tone, WORD_BG_ALPHA))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(tone))
        .child(reason.label.clone())
        .into_any_element()
}

/// `+388` and `−41`; the side that changed nothing is left out — spec §2.
pub(super) fn churn(added: u64, deleted: u64, t: &ThemeColors, cx: &App) -> Vec<AnyElement> {
    let (plus, minus) = format_signed(added, deleted);
    let mut out = Vec::new();
    if added > 0 {
        out.push(word(plus, t.diff_added_fg, cx));
    }
    if deleted > 0 {
        out.push(word(minus, t.diff_removed_fg, cx));
    }
    out
}

/// One piece of metadata text on a header or bar row.
pub(super) fn word(text: impl Into<SharedString>, color: u32, cx: &App) -> AnyElement {
    div()
        .flex_none()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(color))
        .child(text.into())
        .into_any_element()
}
