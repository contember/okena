//! Single-line text that shows its full content in a tooltip, but only while
//! the layout has cut it off with an ellipsis.

use gpui::{ElementId, InteractiveText, SharedString, StyledText};
use gpui_component::tooltip::Tooltip;

/// Text element whose tooltip shows `text` in full when it does not fit.
///
/// The ellipsis itself comes from the parent: give it `.overflow_hidden()`,
/// `.whitespace_nowrap()` and `.text_ellipsis()` (or just `.truncate()`).
/// Without nowrap a long text wraps and the clipped second line gets neither
/// the "…" nor this tooltip, because wrapping leaves the text intact. Whether
/// the text was cut is read from the laid-out line when the mouse rests on it,
/// so a title that fits gets no tooltip and a resize needs no bookkeeping.
///
/// `note` is appended as a second line of the tooltip. Use it when the parent
/// has its own tooltip: the hovered text's tooltip wins, so it must carry the
/// parent's message too.
pub fn truncated_text(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    note: Option<SharedString>,
) -> InteractiveText {
    let text = text.into();
    let styled = StyledText::new(text.clone());
    let layout = styled.layout().clone();
    InteractiveText::new(id, styled).tooltip(move |_ix, window, cx| {
        if layout.text() == text.as_ref() {
            return None;
        }
        let content = match &note {
            Some(note) => format!("{text}\n{note}"),
            None => text.to_string(),
        };
        Some(Tooltip::new(content).build(window, cx))
    })
}
