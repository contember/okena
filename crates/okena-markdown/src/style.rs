//! Document style for the markdown renderer: colors, type scale, rhythm.
//!
//! Everything here is derived from the active `ThemeColors` so custom themes
//! keep working — there is no separate markdown palette.

use gpui::{App, Pixels, px};
use okena_core::theme::ThemeColors;
use okena_ui::color_utils::{raised_surface, raised_surface_border, tint_color};
use okena_ui::tokens::ui_text;

use super::types::Node;

/// Max width of the document column. The caller centers the column in whatever
/// width is left, so wide panes get margins instead of very long lines.
pub const DOC_MAX_WIDTH: Pixels = px(860.0);

/// Body size and leading, before the `ui_font_size` scale is applied.
const BODY_PT: f32 = 14.0;
const BODY_LEADING: f32 = 1.65;
/// Monospace reads a little larger than proportional text at the same nominal
/// size, so inline code sits just below the body size.
const INLINE_CODE_PT: f32 = 12.5;
/// Heading leading — tight enough that a wrapped heading still reads as one unit.
const HEADING_LEADING: f32 = 1.3;

pub(crate) fn body_size(cx: &App) -> Pixels {
    ui_text(BODY_PT, cx)
}

pub(crate) fn body_line_height(cx: &App) -> Pixels {
    ui_text(BODY_PT * BODY_LEADING, cx)
}

pub(crate) fn inline_code_size(cx: &App) -> Pixels {
    ui_text(INLINE_CODE_PT, cx)
}

/// Table cells run at UI size, with their own leading — body leading would make
/// a dense table too airy.
pub(crate) fn table_line_height(cx: &App) -> Pixels {
    ui_text(18.0, cx)
}

/// Size and line height for a heading level. H1 dominates, H2 opens a section,
/// H3 stays close to body size so it reads as a label, not a banner — size and
/// the space around a heading carry the hierarchy, weight only separates
/// headings from body text.
///
/// Every level is bold: intermediate weights depend on the font shipping that
/// face, and a heading that silently falls back to regular stops reading as one.
pub(crate) fn heading_style(level: u8, cx: &App) -> (Pixels, Pixels) {
    let pt = match level {
        1 => 28.0,
        2 => 21.0,
        3 => 17.0,
        4 => 15.0,
        5 => 14.0,
        _ => 13.0,
    };
    (ui_text(pt, cx), ui_text(pt * HEADING_LEADING, cx))
}

/// Markdown colors derived from the app theme.
#[derive(Clone, Copy)]
pub(crate) struct MdColors {
    /// Headings — one step brighter than body copy.
    pub heading: u32,
    /// Body copy. Full text contrast, not the dimmer secondary tone.
    pub body: u32,
    /// Secondary content: list markers, quotes, metadata keys, captions.
    pub muted: u32,
    /// Raised surface for code blocks and metadata cards.
    pub surface: u32,
    /// Border of that surface — only just visible against it.
    pub surface_border: u32,
    /// Inline code: a hint of a surface, not a badge.
    pub inline_code_bg: u32,
    /// Horizontal rules and other structural lines.
    pub rule: u32,
    /// Link text.
    pub link: u32,
}

impl MdColors {
    pub(crate) fn new(t: &ThemeColors) -> Self {
        // Push toward the theme's foreground extreme, so headings land brighter
        // than body text on dark themes and darker on light ones.
        let extreme = if t.is_dark() { 0xffffff } else { 0x000000 };
        Self {
            heading: tint_color(t.text_primary, extreme, 0.35),
            body: t.text_primary,
            muted: t.text_secondary,
            surface: raised_surface(t.bg_secondary, t.text_primary),
            surface_border: raised_surface_border(t.bg_secondary, t.border),
            inline_code_bg: tint_color(t.bg_secondary, t.text_primary, 0.08),
            rule: tint_color(t.bg_secondary, t.border, 0.8),
            // The plain blue is a terminal color picked to sit on the terminal
            // background; the bright variant holds up against document text.
            link: t.term_bright_blue,
        }
    }
}

/// Vertical space `(above, below)` a block. Headings carry most of their space
/// above so a section reads as attached to the heading that opens it — that
/// gap, not a rule, is what separates sections.
pub(crate) fn node_spacing(node: &Node, is_first: bool) -> (Pixels, Pixels) {
    let (above, below) = match node {
        Node::Heading { level, .. } => match level {
            1 => (30.0, 10.0),
            2 => (24.0, 8.0),
            3 => (18.0, 6.0),
            _ => (14.0, 4.0),
        },
        Node::Paragraph { .. } | Node::List { .. } => (0.0, 12.0),
        Node::CodeBlock { .. } | Node::Table { .. } | Node::Blockquote { .. } => (2.0, 14.0),
        Node::HorizontalRule => (10.0, 10.0),
        Node::Frontmatter { .. } => (0.0, 16.0),
    };
    (px(if is_first { 0.0 } else { above }), px(below))
}
