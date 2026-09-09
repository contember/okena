use crate::terminal_view_settings;
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_files::theme::theme;
use okena_terminal::terminal::{Terminal, TerminalSize};
use okena_ui::color_utils::tint_color;
use okena_ui::theme::ansi_to_hsla;
use okena_workspace::settings::CursorShape;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use super::terminal_input::TerminalInputHandler;
use super::terminal_rendering::{
    BatchedTextLine, LayoutRect, is_default_bg, requires_independent_shaping,
};

type ResizeViewerSizes = HashMap<String, HashMap<u64, TerminalSize>>;

/// (desired, target, current) dims + viewer count + authority + verdict.
type ResizeGateKey = (u16, u16, u16, u16, u16, u16, usize, bool, bool);

static NEXT_RESIZE_VIEWER_ID: AtomicU64 = AtomicU64::new(1);
static RESIZE_VIEWER_SIZES: OnceLock<Mutex<ResizeViewerSizes>> = OnceLock::new();
static RESIZE_GATE_LOGGED: OnceLock<Mutex<HashMap<String, ResizeGateKey>>> = OnceLock::new();

pub(crate) fn next_resize_viewer_id() -> u64 {
    NEXT_RESIZE_VIEWER_ID.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::super::terminal_rendering::BatchedTextLine;
    use super::{
        RowHasher, RowLayout, TerminalElementState, TerminalGridLayout, TerminalRenderCache,
        TerminalRenderCacheKey, build_terminal_grid_layout, changed_cells,
        deregister_resize_viewer, selected_columns, shared_resize_target,
    };
    use gpui::{Font, FontFeatures, FontStyle, FontWeight, TextRun, px};
    use okena_core::theme::{DARK_THEME, LIGHT_THEME};
    use okena_terminal::terminal::{Terminal, TerminalSize, TerminalTransport};
    use std::hash::Hasher;
    use std::sync::Arc;

    fn size(cols: u16, rows: u16) -> TerminalSize {
        TerminalSize {
            cols,
            rows,
            cell_width: 8.0,
            cell_height: 16.0,
        }
    }

    #[test]
    fn shared_resize_target_uses_per_dimension_minimum() {
        let terminal_id = "shared_resize_target_uses_per_dimension_minimum";

        let (count, target) = shared_resize_target(terminal_id, 1, size(120, 15));
        assert_eq!(count, 1);
        assert_eq!((target.cols, target.rows), (120, 15));

        let (count, target) = shared_resize_target(terminal_id, 2, size(80, 40));
        assert_eq!(count, 2);
        assert_eq!((target.cols, target.rows), (80, 15));

        deregister_resize_viewer(terminal_id, 1);
        deregister_resize_viewer(terminal_id, 2);
    }

    #[test]
    fn shared_resize_target_grows_when_every_viewer_can_fit() {
        let terminal_id = "shared_resize_target_grows_when_every_viewer_can_fit";

        let _ = shared_resize_target(terminal_id, 1, size(80, 15));
        let _ = shared_resize_target(terminal_id, 2, size(80, 20));
        let (count, target) = shared_resize_target(terminal_id, 1, size(100, 25));

        assert_eq!(count, 2);
        assert_eq!((target.cols, target.rows), (80, 20));

        deregister_resize_viewer(terminal_id, 1);
        deregister_resize_viewer(terminal_id, 2);
    }

    #[test]
    fn deregistered_viewer_no_longer_clamps_resize_target() {
        let terminal_id = "deregistered_viewer_no_longer_clamps_resize_target";

        let _ = shared_resize_target(terminal_id, 1, size(80, 15));
        deregister_resize_viewer(terminal_id, 1);
        let (count, target) = shared_resize_target(terminal_id, 2, size(120, 40));

        assert_eq!(count, 1);
        assert_eq!((target.cols, target.rows), (120, 40));

        deregister_resize_viewer(terminal_id, 2);
    }

    fn test_font() -> Font {
        Font {
            family: "Test Mono".into(),
            features: FontFeatures::disable_ligatures(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }

    fn cache_key() -> TerminalRenderCacheKey {
        TerminalRenderCacheKey {
            content_generation: 7,
            selection: None,
            font: test_font(),
            font_size: px(13.0),
            theme: DARK_THEME,
        }
    }

    fn empty_layout() -> TerminalGridLayout {
        TerminalGridLayout {
            rows: Vec::new(),
            cols: 0,
            display_offset: 0,
            cursor_col: 0,
            cursor_visual_line: 0,
            cells_scanned: 0,
        }
    }

    /// A layout `cols` wide whose row `i` renders `texts[i]` and is hashed by
    /// that text alone. An empty string means the row painted nothing.
    fn layout_of(rows: usize, cols: usize, texts: &[&str]) -> TerminalGridLayout {
        assert_eq!(texts.len(), rows);
        let style = TextRun {
            len: 0,
            font: test_font(),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let rows = texts
            .iter()
            .enumerate()
            .map(|(row, text)| {
                let mut hasher = RowHasher::default();
                hasher.write(text.as_bytes());
                let text_lines = text
                    .chars()
                    .next()
                    .map(|first| {
                        let mut line = BatchedTextLine::new(row as i32, 0, first, style.clone());
                        for (offset, c) in text.chars().skip(1).enumerate() {
                            line.append(offset as i32 + 1, c, style.clone());
                        }
                        line
                    })
                    .into_iter()
                    .collect();
                Arc::new(RowLayout {
                    hash: hasher.finish(),
                    text_lines,
                    rects: Vec::new(),
                })
            })
            .collect();
        TerminalGridLayout {
            rows,
            cols,
            display_offset: 0,
            cursor_col: 0,
            cursor_visual_line: 0,
            cells_scanned: texts.len() * cols,
        }
    }

    #[test]
    fn a_first_build_counts_the_whole_screen_as_changed() {
        let next = layout_of(3, 10, &["a", "b", "c"]);
        assert_eq!(changed_cells(None, &next), (30, 30));
    }

    #[test]
    fn an_identical_rebuild_counts_nothing() {
        let previous = layout_of(3, 10, &["a", "b", "c"]);
        let next = layout_of(3, 10, &["a", "b", "c"]);
        assert_eq!(changed_cells(Some(&previous), &next), (0, 0));
    }

    #[test]
    fn one_edited_row_counts_only_that_row() {
        let previous = layout_of(4, 10, &["a", "b", "c", "d"]);
        let next = layout_of(4, 10, &["a", "b", "CHANGED", "d"]);
        assert_eq!(changed_cells(Some(&previous), &next), (10, 10));
    }

    #[test]
    fn a_scrolled_screen_is_fully_changed_per_row_but_cheap_once_shifted() {
        // Every row moved up one and a new row arrived at the bottom.
        let previous = layout_of(4, 10, &["a", "b", "c", "d"]);
        let next = layout_of(4, 10, &["b", "c", "d", "e"]);
        let (per_row, scroll_aware) = changed_cells(Some(&previous), &next);
        assert_eq!(per_row, 40, "a naive row diff sees the whole screen move");
        assert_eq!(scroll_aware, 10, "only the newly arrived row is really new");
    }

    #[test]
    fn a_resize_counts_the_whole_screen() {
        let previous = layout_of(3, 10, &["a", "b", "c"]);
        let next = layout_of(4, 10, &["a", "b", "c", "d"]);
        assert_eq!(changed_cells(Some(&previous), &next), (40, 40));
    }

    #[test]
    fn render_cache_hits_only_for_an_exact_key_with_layout() {
        let key = cache_key();
        let mut cache = TerminalRenderCache::default();
        assert!(cache.get(&key).is_none());

        let stored = cache.store(key.clone(), empty_layout());
        let hit = cache.get(&key).expect("exact key hits");
        assert!(
            Arc::ptr_eq(&stored, &hit),
            "the hit reuses the stored layout"
        );

        let mut changed_generation = key.clone();
        changed_generation.content_generation += 1;
        assert!(cache.get(&changed_generation).is_none());

        let mut changed_selection = key.clone();
        changed_selection.selection = Some(((1, 2), (3, 4)));
        assert!(cache.get(&changed_selection).is_none());

        let mut changed_font = key.clone();
        changed_font.font.weight = FontWeight::BOLD;
        assert!(cache.get(&changed_font).is_none());

        let mut changed_font_size = key.clone();
        changed_font_size.font_size = px(20.0);
        assert!(cache.get(&changed_font_size).is_none());

        let mut changed_theme = key;
        changed_theme.theme = LIGHT_THEME;
        assert!(cache.get(&changed_theme).is_none());

        cache.layout = None;
        assert!(cache.get(&cache_key()).is_none());
    }

    #[test]
    fn render_cache_invalidate_drops_key_and_layout() {
        let key = cache_key();
        let mut cache = TerminalRenderCache::default();
        cache.store(key.clone(), empty_layout());

        cache.invalidate();

        assert!(cache.get(&key).is_none());
        assert!(cache.key.is_none());
        assert!(cache.layout.is_none());
    }

    #[test]
    fn rows_are_offered_for_reuse_only_under_the_same_font_size_and_theme() {
        let key = cache_key();
        let mut cache = TerminalRenderCache::default();
        assert!(cache.reusable_rows(&key).is_none(), "nothing stored yet");
        cache.store(key.clone(), empty_layout());

        let mut new_content = key.clone();
        new_content.content_generation += 1;
        new_content.selection = Some(((1, 2), (3, 4)));
        assert!(cache.reusable_rows(&new_content).is_some());

        let mut changed_theme = key.clone();
        changed_theme.theme = LIGHT_THEME;
        assert!(cache.reusable_rows(&changed_theme).is_none());

        let mut changed_font = key.clone();
        changed_font.font.weight = FontWeight::BOLD;
        assert!(cache.reusable_rows(&changed_font).is_none());

        let mut changed_font_size = key;
        changed_font_size.font_size = px(20.0);
        assert!(cache.reusable_rows(&changed_font_size).is_none());
    }

    #[test]
    fn selected_columns_follow_the_selection_shape() {
        let selection = Some(((2, 1), (5, 3)));
        assert_eq!(selected_columns(selection, 0), None);
        assert_eq!(selected_columns(selection, 1), Some((2, usize::MAX)));
        assert_eq!(selected_columns(selection, 2), Some((0, usize::MAX)));
        assert_eq!(selected_columns(selection, 3), Some((0, 5)));
        assert_eq!(selected_columns(selection, 4), None);
        assert_eq!(selected_columns(Some(((2, 1), (5, 1))), 1), Some((2, 5)));
        assert_eq!(selected_columns(None, 1), None);
    }

    #[test]
    fn selected_columns_normalise_a_backwards_selection() {
        let backwards = Some(((5, 3), (2, 1)));
        assert_eq!(selected_columns(backwards, 1), Some((2, usize::MAX)));
        assert_eq!(selected_columns(backwards, 3), Some((0, 5)));
        assert_eq!(selected_columns(Some(((5, 1), (2, 1))), 1), Some((2, 5)));
    }

    struct NullTransport;

    impl TerminalTransport for NullTransport {
        fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    fn terminal_showing(cols: u16, rows: u16, output: &[u8]) -> Terminal {
        let terminal = Terminal::new(
            "grid".into(),
            size(cols, rows),
            Arc::new(NullTransport),
            String::new(),
        );
        terminal.process_output(output);
        terminal
    }

    fn element_state() -> TerminalElementState {
        let font = test_font();
        TerminalElementState {
            cell_width: px(8.0),
            line_height: px(16.0),
            font_size: px(13.0),
            font_bold: Font {
                weight: FontWeight::BOLD,
                ..font.clone()
            },
            font_italic: Font {
                style: FontStyle::Italic,
                ..font.clone()
            },
            font_bold_italic: Font {
                weight: FontWeight::BOLD,
                style: FontStyle::Italic,
                ..font.clone()
            },
            font,
        }
    }

    fn build(
        terminal: &Terminal,
        selection: Option<((usize, i32), (usize, i32))>,
        previous: Option<&TerminalGridLayout>,
    ) -> TerminalGridLayout {
        build_terminal_grid_layout(terminal, selection, &DARK_THEME, &element_state(), previous).1
    }

    /// Per row, whether `next` shares `previous`'s `RowLayout` instead of a rebuilt one.
    fn reused_rows(previous: &TerminalGridLayout, next: &TerminalGridLayout) -> Vec<bool> {
        assert_eq!(previous.rows.len(), next.rows.len());
        previous
            .rows
            .iter()
            .zip(&next.rows)
            .map(|(before, after)| Arc::ptr_eq(before, after))
            .collect()
    }

    #[test]
    fn an_identical_rebuild_shares_every_row() {
        let terminal = terminal_showing(10, 3, b"one\r\ntwo\r\nthree");
        let first = build(&terminal, None, None);
        assert_eq!(
            first.cells_scanned, 30,
            "a first build scans the whole screen"
        );

        let second = build(&terminal, None, Some(&first));

        assert_eq!(reused_rows(&first, &second), vec![true, true, true]);
        assert_eq!(second.cells_scanned, 0);
    }

    #[test]
    fn editing_one_row_rebuilds_only_that_row() {
        let terminal = terminal_showing(10, 3, b"one\r\ntwo\r\nthree");
        let first = build(&terminal, None, None);

        terminal.process_output(b"\x1b[2;1HTWO");
        let second = build(&terminal, None, Some(&first));

        assert_eq!(reused_rows(&first, &second), vec![true, false, true]);
        assert_eq!(second.cells_scanned, 10);
        assert_eq!(second.rows[1].text_lines[0].text, "TWO");
    }

    #[test]
    fn a_selection_change_rebuilds_only_the_rows_it_touches() {
        let terminal = terminal_showing(10, 5, b"a\r\nb\r\nc\r\nd\r\ne");
        let unselected = build(&terminal, None, None);

        let selected = build(&terminal, Some(((0, 1), (3, 2))), Some(&unselected));
        assert_eq!(
            reused_rows(&unselected, &selected),
            vec![true, false, false, true, true]
        );

        // Row 2 stays selected but its span changes from an end row to a start row.
        let moved = build(&terminal, Some(((0, 2), (3, 3))), Some(&selected));
        assert_eq!(
            reused_rows(&selected, &moved),
            vec![true, false, false, false, true]
        );
    }

    #[test]
    fn a_resize_rebuilds_every_row() {
        let terminal = terminal_showing(10, 3, b"one\r\ntwo\r\nthree");
        let first = build(&terminal, None, None);

        let wider = terminal_showing(12, 3, b"one\r\ntwo\r\nthree");
        let second = build(&wider, None, Some(&first));
        assert_eq!(reused_rows(&first, &second), vec![false, false, false]);
        assert_eq!(second.cells_scanned, 36);

        let taller = terminal_showing(10, 4, b"one\r\ntwo\r\nthree");
        let third = build(&taller, None, Some(&first));
        assert!(third.rows.iter().all(|row| {
            first
                .rows
                .iter()
                .all(|previous| !Arc::ptr_eq(previous, row))
        }));
        assert_eq!(third.cells_scanned, 40);
    }

    #[test]
    fn an_underline_colour_change_alone_rebuilds_the_row() {
        let terminal = terminal_showing(10, 2, b"\x1b[4mab\x1b[0m\r\nx");
        let first = build(&terminal, None, None);

        terminal.process_output(b"\x1b[1;1H\x1b[4m\x1b[58;2;255;0;0mab\x1b[0m");
        let second = build(&terminal, None, Some(&first));

        assert_eq!(reused_rows(&first, &second), vec![false, true]);
        assert_ne!(
            first.rows[0].text_lines[0].styles[0].underline,
            second.rows[0].text_lines[0].styles[0].underline,
            "the rebuilt row carries the new underline colour"
        );
    }
}

pub(crate) fn deregister_resize_viewer(terminal_id: &str, viewer_id: u64) {
    let mut sizes = resize_viewer_sizes().lock();
    if let Some(viewers) = sizes.get_mut(terminal_id) {
        viewers.remove(&viewer_id);
        if viewers.is_empty() {
            sizes.remove(terminal_id);
            resize_gate_logged().lock().remove(terminal_id);
        }
    }
}

fn resize_viewer_sizes() -> &'static Mutex<ResizeViewerSizes> {
    RESIZE_VIEWER_SIZES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn resize_gate_logged() -> &'static Mutex<HashMap<String, ResizeGateKey>> {
    RESIZE_GATE_LOGGED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Diagnostic for resizes the paint gate drops: names which of the two guards
/// (`is_resize_owner_local` / the multi-viewer minimum) held it back.
/// Deduplicated per terminal so a pane that stays blocked logs once, not once
/// per frame.
#[allow(clippy::too_many_arguments)]
fn log_resize_gate(
    terminal_id: &str,
    viewer_id: u64,
    viewers: usize,
    desired: TerminalSize,
    target: TerminalSize,
    current: TerminalSize,
    owner_local: bool,
    will_send: bool,
) {
    let key = (
        desired.cols,
        desired.rows,
        target.cols,
        target.rows,
        current.cols,
        current.rows,
        viewers,
        owner_local,
        will_send,
    );
    {
        let mut seen = resize_gate_logged().lock();
        if seen.get(terminal_id) == Some(&key) {
            return;
        }
        seen.insert(terminal_id.to_string(), key);
    }

    let verdict = match (will_send, owner_local) {
        (true, _) => "SEND",
        (false, false) => "BLOCKED authority",
        (false, true) => "BLOCKED clamp",
    };
    let line = format!(
        "resize gate: {verdict} terminal={terminal_id} viewer={viewer_id} viewers={viewers} \
         desired={}x{} target={}x{} current={}x{} owner_local={owner_local}",
        desired.cols, desired.rows, target.cols, target.rows, current.cols, current.rows,
    );
    // A dropped resize is the rare event we are hunting, so it goes to `info` —
    // that reaches okena.log on disk and outlives the console's 10k-line ring.
    if will_send {
        log::debug!("{line}");
    } else {
        log::info!("{line}");
    }
}

fn shared_resize_target(
    terminal_id: &str,
    viewer_id: u64,
    desired_size: TerminalSize,
) -> (usize, TerminalSize) {
    let mut sizes = resize_viewer_sizes().lock();
    let viewers = sizes.entry(terminal_id.to_string()).or_default();
    viewers.insert(viewer_id, desired_size);

    let viewer_count = viewers.len();
    let min_cols = viewers
        .values()
        .map(|size| size.cols)
        .min()
        .unwrap_or(desired_size.cols);
    let min_rows = viewers
        .values()
        .map(|size| size.rows)
        .min()
        .unwrap_or(desired_size.rows);

    (
        viewer_count,
        TerminalSize {
            cols: min_cols,
            rows: min_rows,
            cell_width: desired_size.cell_width,
            cell_height: desired_size.cell_height,
        },
    )
}

/// A search match in the terminal grid
#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub line: i32,
    pub col: usize,
    pub len: usize,
}

/// The kind of link detected in the terminal
#[derive(Clone, Debug, PartialEq)]
pub enum LinkKind {
    /// A web URL (http/https)
    Url,
    /// A file path, optionally with line and column numbers
    FilePath { line: Option<u32>, col: Option<u32> },
}

/// A detected URL or file path in the terminal grid
#[derive(Clone, Debug)]
pub struct URLMatch {
    pub line: i32,
    pub col: usize,
    pub len: usize,
    pub url: String,
    pub kind: LinkKind,
    /// Group ID: segments of the same wrapped URL share the same group
    pub link_group: usize,
}

/// Custom GPUI element for rendering a terminal
pub struct TerminalElement {
    terminal: Arc<Terminal>,
    focus_handle: FocusHandle,
    resize_viewer_id: u64,
    render_cache: Arc<Mutex<TerminalRenderCache>>,
    search_matches: Arc<Vec<SearchMatch>>,
    current_match_index: Option<usize>,
    url_matches: Arc<Vec<URLMatch>>,
    hovered_url_group: Option<usize>,
    cursor_visible: bool,
    cursor_style: CursorShape,
    zoom_level: f32,
    /// Optional background tint color (u32 RGB) blended softly into the terminal background.
    bg_tint: Option<u32>,
}

impl TerminalElement {
    pub fn new(terminal: Arc<Terminal>, focus_handle: FocusHandle, resize_viewer_id: u64) -> Self {
        Self {
            terminal,
            focus_handle,
            resize_viewer_id,
            render_cache: Arc::new(Mutex::new(TerminalRenderCache::default())),
            search_matches: Arc::new(Vec::new()),
            current_match_index: None,
            url_matches: Arc::new(Vec::new()),
            hovered_url_group: None,
            cursor_visible: true,
            cursor_style: CursorShape::Block,
            zoom_level: 1.0,
            bg_tint: None,
        }
    }

    pub(crate) fn with_render_cache(
        mut self,
        render_cache: Arc<Mutex<TerminalRenderCache>>,
    ) -> Self {
        self.render_cache = render_cache;
        self
    }

    pub fn with_bg_tint(mut self, tint: Option<u32>) -> Self {
        self.bg_tint = tint;
        self
    }

    pub fn with_zoom(mut self, zoom_level: f32) -> Self {
        self.zoom_level = zoom_level;
        self
    }

    pub fn with_search(
        mut self,
        search_matches: Arc<Vec<SearchMatch>>,
        current_match_index: Option<usize>,
    ) -> Self {
        self.search_matches = search_matches;
        self.current_match_index = current_match_index;
        self
    }

    pub fn with_urls(
        mut self,
        url_matches: Arc<Vec<URLMatch>>,
        hovered_url_group: Option<usize>,
    ) -> Self {
        self.url_matches = url_matches;
        self.hovered_url_group = hovered_url_group;
        self
    }

    pub fn with_cursor_visible(mut self, visible: bool) -> Self {
        self.cursor_visible = visible;
        self
    }

    pub fn with_cursor_style(mut self, style: CursorShape) -> Self {
        self.cursor_style = style;
        self
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// State for terminal element layout
pub struct TerminalElementState {
    cell_width: Pixels,
    line_height: Pixels,
    font_size: Pixels,
    font: Font,
    /// Pre-computed font variants to avoid cloning in hot path
    font_bold: Font,
    font_italic: Font,
    font_bold_italic: Font,
}

#[derive(Clone, Debug, PartialEq)]
struct TerminalRenderCacheKey {
    content_generation: u64,
    selection: Option<((usize, i32), (usize, i32))>,
    /// Only the regular font: `state.font_bold` / `_italic` / `_bold_italic` are
    /// derived from it by overriding weight and style, so it alone pins all four.
    /// A separately configurable bold face would have to be added here too.
    font: Font,
    /// Rows keep their shaped lines, so the size they were shaped at pins them.
    font_size: Pixels,
    theme: ThemeColors,
}

impl TerminalRenderCacheKey {
    /// Whether rows built under `other` still paint the same under `self`.
    fn same_row_inputs(&self, other: &Self) -> bool {
        self.font == other.font && self.font_size == other.font_size && self.theme == other.theme
    }
}

/// One visual row, shared between consecutive layouts while `hash` holds.
#[derive(Debug)]
struct RowLayout {
    /// See `row_hash`.
    hash: u64,
    text_lines: Vec<BatchedTextLine>,
    rects: Vec<LayoutRect>,
}

#[derive(Debug)]
struct TerminalGridLayout {
    rows: Vec<Arc<RowLayout>>,
    cols: usize,
    display_offset: i32,
    cursor_col: usize,
    cursor_visual_line: i32,
    /// Cells of the rows this build actually rebuilt; reused rows cost none.
    cells_scanned: usize,
}

impl TerminalGridLayout {
    fn screen_lines(&self) -> usize {
        self.rows.len()
    }

    fn cells(&self) -> usize {
        self.rows.len().saturating_mul(self.cols)
    }

    fn rects(&self) -> impl Iterator<Item = &LayoutRect> {
        self.rows.iter().flat_map(|row| row.rects.iter())
    }

    fn text_lines(&self) -> impl Iterator<Item = &BatchedTextLine> {
        self.rows.iter().flat_map(|row| row.text_lines.iter())
    }
}

#[derive(Debug, Default)]
pub(crate) struct TerminalRenderCache {
    key: Option<TerminalRenderCacheKey>,
    layout: Option<Arc<TerminalGridLayout>>,
}

impl TerminalRenderCache {
    fn get(&self, key: &TerminalRenderCacheKey) -> Option<Arc<TerminalGridLayout>> {
        if self.key.as_ref() != Some(key) {
            return None;
        }
        self.layout.clone()
    }

    fn store(
        &mut self,
        key: TerminalRenderCacheKey,
        layout: TerminalGridLayout,
    ) -> Arc<TerminalGridLayout> {
        let layout = Arc::new(layout);
        self.key = Some(key);
        self.layout = Some(layout.clone());
        layout
    }

    /// The layout still held from the last build, before this one replaces it.
    fn previous_layout(&self) -> Option<Arc<TerminalGridLayout>> {
        self.layout.clone()
    }

    /// The last layout's rows, when a build under `key` may reuse them: a font,
    /// size or theme change repaints every row and must not be served old ones.
    fn reusable_rows(&self, key: &TerminalRenderCacheKey) -> Option<Arc<TerminalGridLayout>> {
        let previous_key = self.key.as_ref()?;
        if !previous_key.same_row_inputs(key) {
            return None;
        }
        self.layout.clone()
    }

    pub(crate) fn invalidate(&mut self) {
        self.key = None;
        self.layout = None;
    }
}

/// Builds the grid layout and reports the `content_generation` it was built
/// from. The generation is sampled inside `with_content`, i.e. under the same
/// `term` lock that every grid mutation takes — reading it after the lock is
/// released would let a layout be filed under a generation it never saw, and
/// that pane would then hold a stale frame until some other input changed.
fn build_terminal_grid_layout(
    terminal: &Terminal,
    selection: Option<((usize, i32), (usize, i32))>,
    t: &ThemeColors,
    state: &TerminalElementState,
    previous: Option<&TerminalGridLayout>,
) -> (u64, TerminalGridLayout) {
    terminal.with_content(|term| {
        let content_generation = terminal.content_generation();
        let grid = term.grid();
        let screen_lines = grid.screen_lines();
        let cols = grid.columns();
        let display_offset = grid.display_offset() as i32;
        let cursor_point = grid.cursor.point;
        let previous_rows = previous
            .filter(|previous| previous.rows.len() == screen_lines && previous.cols == cols)
            .map(|previous| previous.rows.as_slice());

        let mut rows = Vec::with_capacity(screen_lines);
        let mut rebuilt_rows = 0usize;
        for row in 0..screen_lines {
            let visual_line = row as i32;
            let buffer_line = visual_line - display_offset;
            let cells = &grid[Line(buffer_line)];
            let selected = selected_columns(selection, buffer_line);
            let hash = row_hash(cells, cols, selected);
            let reusable = previous_rows
                .and_then(|rows| rows.get(row))
                .filter(|row| row.hash == hash);
            match reusable {
                Some(row) => rows.push(row.clone()),
                None => {
                    rebuilt_rows += 1;
                    rows.push(Arc::new(build_row(
                        cells,
                        visual_line,
                        cols,
                        selected,
                        hash,
                        t,
                        state,
                    )));
                }
            }
        }

        let layout = TerminalGridLayout {
            rows,
            cols,
            display_offset,
            cursor_col: cursor_point.column.0,
            cursor_visual_line: cursor_point.line.0 + display_offset,
            cells_scanned: rebuilt_rows.saturating_mul(cols),
        };
        (content_generation, layout)
    })
}

/// Columns of `buffer_line` covered by `selection`, inclusive; `None` when the
/// row lies outside it. Open ends extend to `usize::MAX`.
fn selected_columns(
    selection: Option<((usize, i32), (usize, i32))>,
    buffer_line: i32,
) -> Option<(usize, usize)> {
    let ((start_col, start_row), (end_col, end_row)) = selection?;
    let (start_row, start_col, end_row, end_col) =
        if start_row < end_row || (start_row == end_row && start_col <= end_col) {
            (start_row, start_col, end_row, end_col)
        } else {
            (end_row, end_col, start_row, start_col)
        };
    if buffer_line < start_row || buffer_line > end_row {
        return None;
    }
    let first = if buffer_line == start_row {
        start_col
    } else {
        0
    };
    let last = if buffer_line == end_row {
        end_col
    } else {
        usize::MAX
    };
    Some((first, last))
}

/// Everything that decides how a row paints besides the theme and fonts the cache
/// key pins: each cell's glyph, colours, flags and underline colour, plus the
/// selection span. Zero-width chars and hyperlinks are not painted by the grid.
fn row_hash(cells: &Row<Cell>, cols: usize, selected: Option<(usize, usize)>) -> u64 {
    let mut hasher = RowHasher::default();
    selected.hash(&mut hasher);
    for col in 0..cols {
        let cell = &cells[Column(col)];
        hasher.write_u32(u32::from(cell.c));
        hasher.write_u16(cell.flags.bits());
        hash_color(&cell.fg, &mut hasher);
        hash_color(&cell.bg, &mut hasher);
        match cell.underline_color() {
            Some(color) => {
                hasher.write_u8(1);
                hash_color(&color, &mut hasher);
            }
            None => hasher.write_u8(0),
        }
    }
    hasher.finish()
}

fn hash_color(color: &Color, hasher: &mut impl Hasher) {
    std::mem::discriminant(color).hash(hasher);
    match color {
        Color::Named(named) => std::mem::discriminant(named).hash(hasher),
        Color::Spec(rgb) => (rgb.r, rgb.g, rgb.b).hash(hasher),
        Color::Indexed(index) => index.hash(hasher),
    }
}

/// FxHash-style word mixer: the hash pass runs over every cell of every
/// rebuilt screen, and SipHash would cost a good part of the scan it replaces.
#[derive(Default)]
struct RowHasher(u64);

impl Hasher for RowHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_u8(*byte);
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.write_u64(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

fn build_row(
    cells: &Row<Cell>,
    visual_line: i32,
    cols: usize,
    selected: Option<(usize, usize)>,
    hash: u64,
    t: &ThemeColors,
    state: &TerminalElementState,
) -> RowLayout {
    let mut text_lines: Vec<BatchedTextLine> = Vec::new();
    let mut rects: Vec<LayoutRect> = Vec::new();
    let mut current_rect: Option<LayoutRect> = None;
    let mut current_line: Option<BatchedTextLine> = None;

    for col in 0..cols {
        let cell = &cells[Column(col)];
        let col_i32 = col as i32;

        let mut fg = cell.fg;
        let mut bg = cell.bg;

        if cell.flags.contains(Flags::BOLD) {
            fg = match fg {
                Color::Named(NamedColor::Black) => Color::Named(NamedColor::BrightBlack),
                Color::Named(NamedColor::Red) => Color::Named(NamedColor::BrightRed),
                Color::Named(NamedColor::Green) => Color::Named(NamedColor::BrightGreen),
                Color::Named(NamedColor::Yellow) => Color::Named(NamedColor::BrightYellow),
                Color::Named(NamedColor::Blue) => Color::Named(NamedColor::BrightBlue),
                Color::Named(NamedColor::Magenta) => Color::Named(NamedColor::BrightMagenta),
                Color::Named(NamedColor::Cyan) => Color::Named(NamedColor::BrightCyan),
                Color::Named(NamedColor::White) => Color::Named(NamedColor::BrightWhite),
                Color::Indexed(idx @ 0..=7) => Color::Indexed(idx + 8),
                other => other,
            };
        }

        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }

        let is_selected = selected.is_some_and(|(first, last)| col >= first && col <= last);

        let bg_color = if is_selected {
            Some(rgb(t.selection_bg).into())
        } else if !is_default_bg(&bg, t) {
            Some(ansi_to_hsla(t, &bg))
        } else {
            None
        };

        if let Some(color) = bg_color {
            let can_extend = current_rect.as_ref().is_some_and(|rect| {
                rect.line == visual_line
                    && rect.start_col + rect.num_cells as i32 == col_i32
                    && rect.color == color
            });
            if can_extend {
                if let Some(rect) = current_rect.as_mut() {
                    rect.extend();
                }
            } else {
                if let Some(previous) = current_rect.take() {
                    rects.push(previous);
                }
                current_rect = Some(LayoutRect::new(visual_line, col_i32, color));
            }
        } else if let Some(rect) = current_rect.take() {
            rects.push(rect);
        }

        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        if cell.c == ' ' && !cell.flags.intersects(Flags::UNDERLINE | Flags::STRIKEOUT) {
            continue;
        }

        let mut fg_color = if is_selected {
            rgb(t.selection_fg).into()
        } else {
            ansi_to_hsla(t, &fg)
        };

        if cell.flags.contains(Flags::DIM) && !cell.flags.contains(Flags::BOLD) {
            fg_color.l = (fg_color.l * 0.66).clamp(0.0, 1.0);
        }

        let is_bold = cell.flags.contains(Flags::BOLD);
        let is_italic = cell.flags.contains(Flags::ITALIC);
        let font = match (is_bold, is_italic) {
            (true, true) => state.font_bold_italic.clone(),
            (true, false) => state.font_bold.clone(),
            (false, true) => state.font_italic.clone(),
            (false, false) => state.font.clone(),
        };

        let text_style = TextRun {
            len: cell.c.len_utf8(),
            font,
            color: fg_color,
            background_color: None,
            underline: if cell.flags.intersects(Flags::ALL_UNDERLINES) {
                let line_color = cell
                    .underline_color()
                    .map(|color| ansi_to_hsla(t, &color))
                    .unwrap_or(fg_color);
                Some(UnderlineStyle {
                    color: Some(line_color),
                    thickness: px(1.0),
                    wavy: cell.flags.contains(Flags::UNDERCURL),
                })
            } else {
                None
            },
            strikethrough: if cell.flags.contains(Flags::STRIKEOUT) {
                Some(StrikethroughStyle {
                    color: Some(fg_color),
                    thickness: px(1.0),
                })
            } else {
                None
            },
        };

        if requires_independent_shaping(cell.c) {
            if let Some(line) = current_line.take() {
                text_lines.push(line);
            }
            text_lines.push(BatchedTextLine::new(
                visual_line,
                col_i32,
                cell.c,
                text_style,
            ));
        } else if let Some(line) = current_line.as_mut() {
            line.append(col_i32, cell.c, text_style);
        } else {
            current_line = Some(BatchedTextLine::new(
                visual_line,
                col_i32,
                cell.c,
                text_style,
            ));
        }
    }
    if let Some(line) = current_line {
        text_lines.push(line);
    }
    if let Some(rect) = current_rect {
        rects.push(rect);
    }

    RowLayout {
        hash,
        text_lines,
        rects,
    }
}

/// How much of the screen a rebuild actually changed: `(per row, after the
/// best whole-screen vertical shift)`.
///
/// The second number is what a renderer that recognised scrolling would still
/// have to redo. When output streams into a full screen every row shifts up, so
/// the first number reads as "everything changed" while the second stays small
/// — the difference decides whether line damage or scroll reuse is the fix.
///
/// Diagnostic only: rows are compared by their `row_hash`.
fn changed_cells(
    previous: Option<&TerminalGridLayout>,
    next: &TerminalGridLayout,
) -> (usize, usize) {
    let Some(previous) = previous else {
        return (next.cells(), next.cells());
    };
    if previous.rows.len() != next.rows.len() || previous.cols != next.cols {
        return (next.cells(), next.cells());
    }

    let hash_at = |layout: &TerminalGridLayout, row: i32| -> Option<u64> {
        usize::try_from(row)
            .ok()
            .and_then(|row| layout.rows.get(row))
            .map(|row| row.hash)
    };
    let rows = next.rows.len() as i32;
    let changed_rows = |shift: i32| {
        (0..rows)
            .filter(|&row| hash_at(previous, row + shift) != hash_at(next, row))
            .count()
    };

    let per_row = changed_rows(0);
    let best = (-MAX_TRACKED_SCROLL..=MAX_TRACKED_SCROLL)
        .map(changed_rows)
        .min()
        .unwrap_or(per_row);
    (
        per_row.saturating_mul(next.cols),
        best.saturating_mul(next.cols),
    )
}

/// Vertical shift the scroll-aware measurement searches. Wide enough for the
/// usual streaming case (a few lines at a time), cheap enough to run per paint.
const MAX_TRACKED_SCROLL: i32 = 8;

impl Element for TerminalElement {
    type RequestLayoutState = TerminalElementState;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // Get font settings from global settings, apply per-terminal zoom
        let app_settings = terminal_view_settings(cx);
        let font_size = px(app_settings.font_size * self.zoom_level);
        let line_height_multiplier = app_settings.line_height;
        let font_family = app_settings.font_family.clone();

        // Use configured font family with fallbacks
        #[cfg(target_os = "macos")]
        let font = Font {
            family: font_family.into(),
            features: FontFeatures::disable_ligatures(),
            fallbacks: Some(FontFallbacks::from_fonts(vec![
                "JetBrains Mono".into(),
                "Menlo".into(),
                "SF Mono".into(),
                "Monaco".into(),
            ])),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };

        #[cfg(not(target_os = "macos"))]
        let font = Font {
            family: font_family.into(),
            features: FontFeatures::disable_ligatures(),
            fallbacks: Some(FontFallbacks::from_fonts(vec![
                "JetBrains Mono".into(),
                "DejaVu Sans Mono".into(),
                "Liberation Mono".into(),
                "Ubuntu Mono".into(),
                "Noto Sans Mono".into(),
                "monospace".into(),
            ])),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };

        // Pre-compute font variants to avoid cloning in hot path
        let font_bold = Font {
            weight: FontWeight::BOLD,
            ..font.clone()
        };
        let font_italic = Font {
            style: FontStyle::Italic,
            ..font.clone()
        };
        let font_bold_italic = Font {
            weight: FontWeight::BOLD,
            style: FontStyle::Italic,
            ..font.clone()
        };

        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);

        // Use advance() for proper cell width (like Zed)
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map(|size| size.width)
            .unwrap_or(font_size * 0.6);

        // Line height from settings
        let line_height = font_size * line_height_multiplier;

        let style = Style {
            size: Size {
                width: relative(1.0).into(),
                height: relative(1.0).into(),
            },
            ..Default::default()
        };

        let layout_id = window.request_layout(style, [], cx);

        (
            layout_id,
            TerminalElementState {
                cell_width,
                line_height,
                font_size,
                font,
                font_bold,
                font_italic,
                font_bold_italic,
            },
        )
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let render_probe = okena_core::render_probe::terminal_paint();

        // Get theme colors
        let t = theme(cx);

        // Register input handler
        let input_handler = TerminalInputHandler {
            terminal: self.terminal.clone(),
            viewer_id: self.resize_viewer_id,
        };
        window.handle_input(&self.focus_handle, input_handler, cx);

        // Remote readers enqueue bytes without advancing `content_generation`;
        // drain before sampling the cache key so newly arrived output can never
        // be mistaken for an unchanged frame.
        self.terminal.process_pending_output();

        let cell_width = state.cell_width;
        let line_height = state.line_height;
        let font_size = state.font_size;
        let cell_width_f = f32::from(cell_width);
        let line_height_f = f32::from(line_height);

        // Calculate terminal size and resize if needed
        let available_width = f32::from(bounds.size.width);
        let available_height = f32::from(bounds.size.height);

        let new_cols = ((available_width - 0.5) / cell_width_f).floor().max(1.0) as u16;
        let new_rows = ((available_height - 0.5) / line_height_f).floor().max(1.0) as u16;

        let desired_size = TerminalSize {
            cols: new_cols,
            rows: new_rows,
            cell_width: cell_width_f,
            cell_height: line_height_f,
        };
        let (n_viewers, resize_size) = shared_resize_target(
            &self.terminal.terminal_id,
            self.resize_viewer_id,
            desired_size,
        );

        let current_size = self.terminal.resize_state.lock().size;
        let cols_rows_changed =
            resize_size.cols != current_size.cols || resize_size.rows != current_size.rows;
        let cell_size_changed = (cell_width_f - current_size.cell_width).abs() > 0.001
            || (line_height_f - current_size.cell_height).abs() > 0.001;

        // Multi-window resize gate: when the same terminal is rendered in
        // more than one visible pane, resize to the per-dimension minimum
        // desired by all live viewers. This avoids ping-pong between
        // differently shaped windows while still allowing growth once every
        // visible viewer can fit the larger dimension.
        let target = if n_viewers <= 1 {
            desired_size
        } else {
            resize_size
        };
        // Anything to decide? Keeps the steady-state paint off the authority
        // lock, as before — the extra check only fires when this pane or a
        // co-viewer disagrees with the live size.
        let contested = cols_rows_changed
            || desired_size.cols != current_size.cols
            || desired_size.rows != current_size.rows;
        let owner_local = contested && self.terminal.is_resize_owner_local();
        let will_send = cols_rows_changed && owner_local;

        if contested {
            log_resize_gate(
                &self.terminal.terminal_id,
                self.resize_viewer_id,
                n_viewers,
                desired_size,
                target,
                current_size,
                owner_local,
                will_send,
            );
        }

        if will_send {
            self.terminal.resize(target);
        } else if cell_size_changed {
            let mut rs = self.terminal.resize_state.lock();
            rs.size.cell_width = cell_width_f;
            rs.size.cell_height = line_height_f;
        }

        // Paint background using theme color (different for focused vs unfocused)
        let is_focused = self.focus_handle.is_focused(window);
        let base_bg = if is_focused {
            t.term_background
        } else {
            t.term_background_unfocused
        };
        let bg_color = match self.bg_tint {
            Some(tint) => tint_color(base_bg, tint, 0.025),
            None => base_bg,
        };
        window.paint_quad(fill(bounds, rgb(bg_color)));

        // Get selection bounds
        let selection = self.terminal.selection_bounds();

        // Capture cursor state for the closure. An app-set cursor shape
        // (DECSCUSR, e.g. vim/helix toggling bar in insert mode) wins over
        // the user preference.
        let cursor_visible = self.cursor_visible;
        let cursor_style = match self.terminal.app_cursor_shape() {
            Some(okena_terminal::terminal::AppCursorShape::Block) => CursorShape::Block,
            Some(okena_terminal::terminal::AppCursorShape::Bar) => CursorShape::Bar,
            Some(okena_terminal::terminal::AppCursorShape::Underline) => CursorShape::Underline,
            None => self.cursor_style,
        };

        let mut cache_key = TerminalRenderCacheKey {
            content_generation: self.terminal.content_generation(),
            selection,
            font: state.font.clone(),
            font_size,
            theme: t,
        };
        let mut render_cache = self.render_cache.lock();
        let cached_layout = render_cache.get(&cache_key);
        let grid_cache_hit = cached_layout.is_some();
        let mut cells_changed: Option<(usize, usize)> = None;
        let layout = match cached_layout {
            Some(layout) => layout,
            None => {
                let previous = render_cache.reusable_rows(&cache_key);
                let (content_generation, layout) = build_terminal_grid_layout(
                    &self.terminal,
                    selection,
                    &t,
                    state,
                    previous.as_deref(),
                );
                if okena_core::render_probe::enabled() {
                    let previous = render_cache.previous_layout();
                    cells_changed = Some(changed_cells(previous.as_deref(), &layout));
                }
                // File the layout under the generation observed while building it:
                // `with_content` drains pending remote output first, so the value
                // sampled before the call can already be one behind.
                cache_key.content_generation = content_generation;
                render_cache.store(cache_key, layout)
            }
        };
        drop(render_cache);

        // One layer for the grid: every quad shares its draw order instead of
        // paying a BoundsTree insert each; stable sorting keeps their paint order.
        window.paint_layer(bounds, |window| {
            // Phase 2: Paint backgrounds
            for rect in layout.rects() {
                rect.paint(bounds.origin, cell_width, line_height, window);
            }

            // Phase 2.5: Paint search highlights
            // search_match.line is an absolute grid line; convert to visual row
            for (idx, search_match) in self.search_matches.iter().enumerate() {
                let visual_line = search_match.line + layout.display_offset;
                if visual_line < 0 || visual_line >= layout.screen_lines() as i32 {
                    continue;
                }

                let is_current = self.current_match_index == Some(idx);
                let highlight_color = if is_current {
                    let c = rgb(t.search_current_bg);
                    Hsla::from(Rgba {
                        r: c.r,
                        g: c.g,
                        b: c.b,
                        a: 0.7,
                    })
                } else {
                    let c = rgb(t.search_match_bg);
                    Hsla::from(Rgba {
                        r: c.r,
                        g: c.g,
                        b: c.b,
                        a: 0.5,
                    })
                };

                let position = point(
                    px(
                        (f32::from(bounds.origin.x) + search_match.col as f32 * cell_width_f)
                            .floor(),
                    ),
                    bounds.origin.y + line_height * visual_line as f32,
                );
                let size = size(
                    px((cell_width_f * search_match.len as f32).ceil()),
                    line_height,
                );

                window.paint_quad(fill(Bounds::new(position, size), highlight_color));
            }

            // Phase 2.6: Paint URL underlines
            for url_match in self.url_matches.iter() {
                let is_hovered = self.hovered_url_group == Some(url_match.link_group);

                if url_match.line < 0 || url_match.line >= layout.screen_lines() as i32 {
                    continue;
                }

                let url_x =
                    px((f32::from(bounds.origin.x) + url_match.col as f32 * cell_width_f).floor());
                let url_y = bounds.origin.y + line_height * url_match.line as f32;
                let url_width = px((cell_width_f * url_match.len as f32).ceil());

                if is_hovered {
                    let hover_bg = Hsla::from(Rgba {
                        r: 0.0,
                        g: 0.48,
                        b: 0.8,
                        a: 0.2,
                    });
                    let hover_bounds = Bounds {
                        origin: point(url_x, url_y),
                        size: size(url_width, line_height),
                    };
                    window.paint_quad(fill(hover_bounds, hover_bg));

                    let underline_color = rgb(t.border_active);
                    let underline_y = url_y + line_height - px(2.0);
                    let underline_bounds = Bounds {
                        origin: point(url_x, underline_y),
                        size: size(url_width, px(1.0)),
                    };
                    window.paint_quad(fill(underline_bounds, underline_color));
                } else {
                    let underline_color = Hsla::from(Rgba {
                        r: 0.5,
                        g: 0.5,
                        b: 0.5,
                        a: 0.5,
                    });
                    let underline_y = url_y + line_height - px(2.0);
                    let underline_bounds = Bounds {
                        origin: point(url_x, underline_y),
                        size: size(url_width, px(1.0)),
                    };
                    window.paint_quad(fill(underline_bounds, underline_color));
                }
            }

            // Phase 3: Paint text runs. Each shaped line opens its own nested layer
            // (GPUI `paint_line`), which keeps its decorations below its glyphs as before.
            for line in layout.text_lines() {
                line.paint(
                    bounds.origin,
                    cell_width,
                    line_height,
                    font_size,
                    window,
                    cx,
                );
            }
        });

        // Phase 4: Paint cursor
        if cursor_visible
            && layout.cursor_visual_line >= 0
            && layout.cursor_visual_line < layout.screen_lines() as i32
        {
            let cursor_x =
                px((f32::from(bounds.origin.x) + layout.cursor_col as f32 * cell_width_f).floor());
            let cursor_y = px((f32::from(bounds.origin.y)
                + layout.cursor_visual_line as f32 * line_height_f)
                .floor());

            let cursor_rgba = rgb(t.cursor);
            let cursor_color = Hsla::from(Rgba {
                r: cursor_rgba.r,
                g: cursor_rgba.g,
                b: cursor_rgba.b,
                a: 0.8,
            });

            let cursor_bounds = match cursor_style {
                CursorShape::Block => Bounds {
                    origin: point(cursor_x, cursor_y),
                    size: size(cell_width, line_height),
                },
                CursorShape::Bar => Bounds {
                    origin: point(cursor_x, cursor_y),
                    size: size(px(2.0), line_height),
                },
                CursorShape::Underline => Bounds {
                    origin: point(cursor_x, cursor_y + line_height - px(2.0)),
                    size: size(cell_width, px(2.0)),
                },
            };
            window.paint_quad(fill(cursor_bounds, cursor_color));
        }

        let cells_scanned = if grid_cache_hit {
            0
        } else {
            layout.cells_scanned
        };
        let text_runs = layout.text_lines().count();
        let background_rects = layout.rects().count();

        // Phase 5: Paint fog overlay for unfocused terminals
        if !is_focused {
            let bg_rgba = rgb(bg_color);
            let fog = Hsla::from(Rgba {
                r: bg_rgba.r,
                g: bg_rgba.g,
                b: bg_rgba.b,
                a: 0.2,
            });
            window.paint_quad(fill(bounds, fog));
        }

        render_probe.finish(okena_core::render_probe::TerminalPaintStats {
            live_viewers: n_viewers,
            grid_cache_hit,
            cells_scanned,
            cells_changed: cells_changed.map(|(per_row, _)| per_row),
            cells_changed_scroll_aware: cells_changed.map(|(_, shifted)| shifted),
            text_runs,
            background_rects,
        });

        let painted_samples = okena_core::latency_probe::client_painted(
            &self.terminal.terminal_id,
            self.resize_viewer_id,
        );
        if !painted_samples.is_empty() {
            let terminal_id = self.terminal.terminal_id.clone();
            let viewer_id = self.resize_viewer_id;
            window.on_next_frame(move |_window, _cx| {
                okena_core::latency_probe::client_frame_completed(
                    &terminal_id,
                    viewer_id,
                    &painted_samples,
                );
            });
        }
    }
}
