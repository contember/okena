//! File view: header, symbol bar, details, outline — spec §9.

mod bar;
mod header;
mod outline;
mod structure;
mod text;
mod token_diff;

use super::super::DiffViewer;
use super::super::line_render::{WORD_BG_ALPHA, rgba as tint};
use super::super::types::DisplayItem;
use super::labels::format_signed;
use super::model::{FileEntry, Reason, ReasonKind};
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_review::OutlineFact;
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

    /// Base and head outlines of the open file; `None` when both are empty.
    pub(super) fn review_open_outline(&self) -> Option<(&[OutlineFact], &[OutlineFact])> {
        let index = self.review_open_entry()?.structure_index?;
        let file = self.smart_review.structure.ready()?.files().get(index)?;
        let outlines = (file.old_outline(), file.new_outline());
        (!outlines.0.is_empty() || !outlines.1.is_empty()).then_some(outlines)
    }

    /// Base and head line of the diff row at the top of the viewport.
    pub(super) fn review_viewport_lines(&self) -> (Option<u32>, Option<u32>) {
        let Some(file) = self.current_file.as_ref() else {
            return (None, None);
        };
        let top = self
            .scroll_handle
            .0
            .borrow()
            .base_handle
            .logical_scroll_top()
            .0;
        match file.items.get(top) {
            Some(DisplayItem::Line(line)) => (
                line_number(line.old_line_num),
                line_number(line.new_line_num),
            ),
            Some(DisplayItem::Expander(expander)) => (
                line_number(Some(expander.old_range.0)),
                line_number(Some(expander.new_range.0)),
            ),
            None => (None, None),
        }
    }

    /// Spec §12: a narrow content column drops the labels that repeat elsewhere.
    pub(super) fn review_content_is_narrow(&self) -> bool {
        let width = self.review_ui.content_width;
        width > 0.0 && width < NARROW_CONTENT
    }
}

fn line_number(value: Option<usize>) -> Option<u32> {
    value.and_then(|line| u32::try_from(line).ok())
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
