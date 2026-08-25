use crate::terminal_view_settings;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
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
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use super::terminal_input::TerminalInputHandler;
use super::terminal_rendering::{BatchedTextLine, LayoutRect, is_default_bg};

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
    use super::{
        TerminalGridLayout, TerminalRenderCache, TerminalRenderCacheKey, deregister_resize_viewer,
        shared_resize_target,
    };
    use gpui::{Font, FontFeatures, FontStyle, FontWeight};
    use okena_core::theme::{DARK_THEME, LIGHT_THEME};
    use okena_terminal::terminal::TerminalSize;
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

    fn cache_key() -> TerminalRenderCacheKey {
        TerminalRenderCacheKey {
            content_generation: 7,
            selection: None,
            font: Font {
                family: "Test Mono".into(),
                features: FontFeatures::disable_ligatures(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            },
            theme: DARK_THEME,
        }
    }

    fn empty_layout() -> TerminalGridLayout {
        TerminalGridLayout {
            text_lines: Vec::new(),
            rects: Vec::new(),
            screen_lines: 0,
            display_offset: 0,
            cursor_col: 0,
            cursor_visual_line: 0,
            cells_scanned: 0,
        }
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
    theme: ThemeColors,
}

#[derive(Debug)]
struct TerminalGridLayout {
    text_lines: Vec<BatchedTextLine>,
    rects: Vec<LayoutRect>,
    screen_lines: usize,
    display_offset: i32,
    cursor_col: usize,
    cursor_visual_line: i32,
    cells_scanned: usize,
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
) -> (u64, TerminalGridLayout) {
    let (content_generation, text_lines, rects, screen_lines, display_offset, cursor_point, cols) =
        terminal.with_content(|term| {
            let content_generation = terminal.content_generation();
            let grid = term.grid();
            let screen_lines = grid.screen_lines();
            let cols = grid.columns();
            let display_offset = grid.display_offset() as i32;
            let cursor_point = grid.cursor.point;

            let mut text_lines: Vec<BatchedTextLine> = Vec::new();
            let mut rects: Vec<LayoutRect> = Vec::new();
            let mut current_rect: Option<LayoutRect> = None;

            for row in 0..screen_lines {
                let visual_line = row as i32;
                let buffer_line = visual_line - display_offset;
                let mut current_line: Option<BatchedTextLine> = None;

                if let Some(rect) = current_rect.take() {
                    rects.push(rect);
                }

                for col in 0..cols {
                    let cell_point = alacritty_terminal::index::Point {
                        line: Line(buffer_line),
                        column: Column(col),
                    };
                    let cell = &grid[cell_point];
                    let col_i32 = col as i32;

                    let mut fg = cell.fg;
                    let mut bg = cell.bg;

                    if cell.flags.contains(Flags::BOLD) {
                        fg = match fg {
                            Color::Named(NamedColor::Black) => {
                                Color::Named(NamedColor::BrightBlack)
                            }
                            Color::Named(NamedColor::Red) => Color::Named(NamedColor::BrightRed),
                            Color::Named(NamedColor::Green) => {
                                Color::Named(NamedColor::BrightGreen)
                            }
                            Color::Named(NamedColor::Yellow) => {
                                Color::Named(NamedColor::BrightYellow)
                            }
                            Color::Named(NamedColor::Blue) => Color::Named(NamedColor::BrightBlue),
                            Color::Named(NamedColor::Magenta) => {
                                Color::Named(NamedColor::BrightMagenta)
                            }
                            Color::Named(NamedColor::Cyan) => Color::Named(NamedColor::BrightCyan),
                            Color::Named(NamedColor::White) => {
                                Color::Named(NamedColor::BrightWhite)
                            }
                            Color::Indexed(idx @ 0..=7) => Color::Indexed(idx + 8),
                            other => other,
                        };
                    }

                    if cell.flags.contains(Flags::INVERSE) {
                        std::mem::swap(&mut fg, &mut bg);
                    }

                    let is_selected =
                        if let Some(((start_col, start_row), (end_col, end_row))) = selection {
                            let (start_row, start_col, end_row, end_col) = if start_row < end_row
                                || (start_row == end_row && start_col <= end_col)
                            {
                                (start_row, start_col, end_row, end_col)
                            } else {
                                (end_row, end_col, start_row, start_col)
                            };
                            if buffer_line >= start_row && buffer_line <= end_row {
                                if start_row == end_row {
                                    col >= start_col && col <= end_col
                                } else if buffer_line == start_row {
                                    col >= start_col
                                } else if buffer_line == end_row {
                                    col <= end_col
                                } else {
                                    true
                                }
                            } else {
                                false
                            }
                        } else {
                            false
                        };

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
                    if cell.c == ' ' && !cell.flags.intersects(Flags::UNDERLINE | Flags::STRIKEOUT)
                    {
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

                    if let Some(line) = current_line.as_mut() {
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
            }

            if let Some(rect) = current_rect {
                rects.push(rect);
            }

            (
                content_generation,
                text_lines,
                rects,
                screen_lines,
                display_offset,
                cursor_point,
                cols,
            )
        });
    let layout = TerminalGridLayout {
        text_lines,
        rects,
        screen_lines,
        display_offset,
        cursor_col: cursor_point.column.0,
        cursor_visual_line: cursor_point.line.0 + display_offset,
        cells_scanned: screen_lines.saturating_mul(cols),
    };
    (content_generation, layout)
}

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
            theme: t,
        };
        let mut render_cache = self.render_cache.lock();
        let cached_layout = render_cache.get(&cache_key);
        let grid_cache_hit = cached_layout.is_some();
        let layout = match cached_layout {
            Some(layout) => layout,
            None => {
                let (content_generation, layout) =
                    build_terminal_grid_layout(&self.terminal, selection, &t, state);
                // File the layout under the generation observed while building it:
                // `with_content` drains pending remote output first, so the value
                // sampled before the call can already be one behind.
                cache_key.content_generation = content_generation;
                render_cache.store(cache_key, layout)
            }
        };
        drop(render_cache);

        // Phase 2: Paint backgrounds
        for rect in &layout.rects {
            rect.paint(bounds.origin, cell_width, line_height, window);
        }

        // Phase 2.5: Paint search highlights
        // search_match.line is an absolute grid line; convert to visual row
        for (idx, search_match) in self.search_matches.iter().enumerate() {
            let visual_line = search_match.line + layout.display_offset;
            if visual_line < 0 || visual_line >= layout.screen_lines as i32 {
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
                px((f32::from(bounds.origin.x) + search_match.col as f32 * cell_width_f).floor()),
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

            if url_match.line < 0 || url_match.line >= layout.screen_lines as i32 {
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

        // Phase 3: Paint text runs
        for line in &layout.text_lines {
            line.paint(
                bounds.origin,
                cell_width,
                line_height,
                font_size,
                window,
                cx,
            );
        }

        // Phase 4: Paint cursor
        if cursor_visible
            && layout.cursor_visual_line >= 0
            && layout.cursor_visual_line < layout.screen_lines as i32
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
        let text_runs = layout.text_lines.len();
        let background_rects = layout.rects.len();

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
            cells_changed: None,
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
