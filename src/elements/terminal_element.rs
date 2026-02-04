use crate::settings::settings;
use crate::terminal::terminal::Terminal;
use crate::theme::{theme, ThemeColors};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::grid::Dimensions;
use gpui::*;
use std::ops::Range;
use std::sync::Arc;

/// A search match in the terminal grid
#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub line: i32,
    pub col: usize,
    pub len: usize,
}

/// The kind of link detected in the terminal
#[derive(Clone, Debug)]
pub enum LinkKind {
    /// A web URL (http/https)
    Url,
    /// A file path, optionally with line and column numbers
    FilePath {
        line: Option<u32>,
        col: Option<u32>,
    },
}

/// A detected URL or file path in the terminal grid
#[derive(Clone, Debug)]
pub struct URLMatch {
    pub line: i32,
    pub col: usize,
    pub len: usize,
    pub url: String,
    pub kind: LinkKind,
}

/// Custom GPUI element for rendering a terminal
pub struct TerminalElement {
    terminal: Arc<Terminal>,
    focus_handle: FocusHandle,
    search_matches: Arc<Vec<SearchMatch>>,
    current_match_index: Option<usize>,
    url_matches: Arc<Vec<URLMatch>>,
    hovered_url_index: Option<usize>,
    cursor_visible: bool,
    zoom_level: f32,
}

/// ASCII DEL character - what terminals expect for backspace
const DEL: u8 = 0x7f;

/// macOS function key character range (U+F700-U+F8FF)
/// GPUI sends these for arrow keys, function keys, etc.
/// but we handle those separately via on_key_down -> key_to_bytes
const MACOS_FUNCTION_KEY_RANGE: std::ops::RangeInclusive<char> = '\u{F700}'..='\u{F8FF}';

/// Input handler for terminal text input
struct TerminalInputHandler {
    terminal: Arc<Terminal>,
}

impl TerminalInputHandler {
    /// Send text input to terminal, filtering macOS function keys and handling control characters
    fn send_filtered_input(&self, text: &str) {
        if text.is_empty() {
            return;
        }

        // Filter out macOS function key characters
        let filtered: String = text
            .chars()
            .filter(|&c| !MACOS_FUNCTION_KEY_RANGE.contains(&c))
            .collect();

        if filtered.is_empty() {
            return;
        }

        // Fast path: no control characters, send entire string at once
        if !filtered.chars().any(|c| matches!(c, '\n' | '\r' | '\u{8}')) {
            self.terminal.send_input(&filtered);
            return;
        }

        // Slow path: handle control characters individually
        for c in filtered.chars() {
            match c {
                '\u{8}' => self.terminal.send_bytes(&[DEL]),
                '\n' | '\r' => self.terminal.send_bytes(&[b'\r']),
                _ => {
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    self.terminal.send_input(s);
                }
            }
        }
    }
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, _cx: &mut App) -> Option<Range<usize>> {
        None
    }

    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        self.send_filtered_input(text);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        self.send_filtered_input(new_text);
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut App) {}

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        true
    }
}

impl TerminalElement {
    pub fn new(terminal: Arc<Terminal>, focus_handle: FocusHandle) -> Self {
        Self {
            terminal,
            focus_handle,
            search_matches: Arc::new(Vec::new()),
            current_match_index: None,
            url_matches: Arc::new(Vec::new()),
            hovered_url_index: None,
            cursor_visible: true,
            zoom_level: 1.0,
        }
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
        hovered_url_index: Option<usize>,
    ) -> Self {
        self.url_matches = url_matches;
        self.hovered_url_index = hovered_url_index;
        self
    }

    pub fn with_cursor_visible(mut self, visible: bool) -> Self {
        self.cursor_visible = visible;
        self
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// A batched text run that combines multiple adjacent cells with the same style (like Zed)
#[derive(Debug)]
struct BatchedTextRun {
    start_line: i32,
    start_col: i32,
    text: String,
    cell_count: usize,
    style: TextRun,
}

impl BatchedTextRun {
    fn new(start_line: i32, start_col: i32, c: char, style: TextRun) -> Self {
        let mut text = String::with_capacity(100);
        text.push(c);
        BatchedTextRun {
            start_line,
            start_col,
            text,
            cell_count: 1,
            style,
        }
    }

    fn can_append(&self, other_style: &TextRun, line: i32, col: i32) -> bool {
        self.start_line == line
            && self.start_col + self.cell_count as i32 == col
            && self.style.font == other_style.font
            && self.style.color == other_style.color
            && self.style.background_color == other_style.background_color
            && self.style.underline == other_style.underline
            && self.style.strikethrough == other_style.strikethrough
    }

    fn append_char(&mut self, c: char) {
        self.text.push(c);
        self.cell_count += 1;
        self.style.len += c.len_utf8();
    }

    fn paint(
        &self,
        origin: Point<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        font_size: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pos = Point::new(
            origin.x + self.start_col as f32 * cell_width,
            origin.y + self.start_line as f32 * line_height,
        );

        // Create style for the entire text run
        let run_style = TextRun {
            len: self.text.len(),
            font: self.style.font.clone(),
            color: self.style.color,
            background_color: self.style.background_color,
            underline: self.style.underline.clone(),
            strikethrough: self.style.strikethrough.clone(),
        };

        // Shape and paint entire run at once, passing cell_width for fixed-width spacing
        // This is how Zed does it - allows proper glyph caching while maintaining grid alignment
        let _ = window
            .text_system()
            .shape_line(
                self.text.clone().into(),
                font_size,
                &[run_style],
                Some(cell_width),
            )
            .paint(
                pos,
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
    }
}

/// A layout rectangle for background colors (like Zed)
#[derive(Clone, Debug)]
struct LayoutRect {
    line: i32,
    start_col: i32,
    num_cells: usize,
    color: Hsla,
}

impl LayoutRect {
    fn new(line: i32, col: i32, color: Hsla) -> Self {
        LayoutRect {
            line,
            start_col: col,
            num_cells: 1,
            color,
        }
    }

    fn extend(&mut self) {
        self.num_cells += 1;
    }

    fn paint(&self, origin: Point<Pixels>, cell_width: Pixels, line_height: Pixels, window: &mut Window) {
        let position = point(
            px((f32::from(origin.x) + self.start_col as f32 * f32::from(cell_width)).floor()),
            origin.y + line_height * self.line as f32,
        );
        let size = size(
            px((f32::from(cell_width) * self.num_cells as f32).ceil()),
            line_height,
        );

        window.paint_quad(fill(Bounds::new(position, size), self.color));
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
        let app_settings = settings(cx);
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
        let cell_width = text_system.advance(font_id, font_size, 'm')
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
        // Get theme colors
        let t = theme(cx);

        // Register input handler
        let input_handler = TerminalInputHandler {
            terminal: self.terminal.clone(),
        };
        window.handle_input(&self.focus_handle, input_handler, cx);

        let cell_width = state.cell_width;
        let line_height = state.line_height;
        let font_size = state.font_size;
        let cell_width_f = f32::from(cell_width);
        let line_height_f = f32::from(line_height);

        // Calculate terminal size and resize if needed
        // Use floor() for consistent sizing - this ensures we don't overflow the available space
        // We subtract a small epsilon (0.5px) before floor to handle floating point precision issues
        // and ensure partial cells don't cause rendering artifacts
        let available_width = f32::from(bounds.size.width);
        let available_height = f32::from(bounds.size.height);


        // Calculate columns and rows, ensuring we have at least 1 of each
        // Use floor to ensure we never overflow the container bounds
        let new_cols = ((available_width - 0.5) / cell_width_f).floor().max(1.0) as u16;
        let new_rows = ((available_height - 0.5) / line_height_f).floor().max(1.0) as u16;

        let current_size = self.terminal.size.lock().clone();
        let cols_rows_changed = new_cols != current_size.cols || new_rows != current_size.rows;
        let cell_size_changed = (cell_width_f - current_size.cell_width).abs() > 0.001
            || (line_height_f - current_size.cell_height).abs() > 0.001;

        if cols_rows_changed {
            // Full resize: grid dimensions changed, need to resize terminal and PTY
            let new_size = crate::terminal::terminal::TerminalSize {
                cols: new_cols,
                rows: new_rows,
                cell_width: cell_width_f,
                cell_height: line_height_f,
            };
            self.terminal.resize(new_size);
        } else if cell_size_changed {
            // Only cell dimensions changed (e.g., zoom) - just update the size struct
            // This ensures hover detection uses the same cell_width as rendering
            // without triggering unnecessary grid/PTY resizes
            let mut size = self.terminal.size.lock();
            size.cell_width = cell_width_f;
            size.cell_height = line_height_f;
        }

        // Paint background using theme color (different for focused vs unfocused)
        let is_focused = self.focus_handle.is_focused(window);
        let bg_color = if is_focused {
            t.term_background
        } else {
            t.term_background_unfocused
        };
        window.paint_quad(fill(bounds, rgb(bg_color)));

        // Get selection bounds
        let selection = self.terminal.selection_bounds();

        // Capture cursor visibility for the closure
        let cursor_visible = self.cursor_visible;

        self.terminal.with_content(|term| {
            let grid = term.grid();
            let screen_lines = grid.screen_lines();
            let cols = grid.columns();
            let display_offset = grid.display_offset() as i32;

            let origin = bounds.origin;

            // Phase 1: Layout grid - collect batched runs and background rects (like Zed)
            let mut batched_runs: Vec<BatchedTextRun> = Vec::new();
            let mut rects: Vec<LayoutRect> = Vec::new();
            let mut current_batch: Option<BatchedTextRun> = None;
            let mut current_rect: Option<LayoutRect> = None;

            for row in 0..screen_lines {
                // visual_line is the row position on screen for rendering
                let visual_line = row as i32;
                // buffer_line is the actual grid line to fetch (accounts for scroll)
                // When display_offset > 0, we're scrolled up into history (negative lines)
                let buffer_line = visual_line - display_offset;

                // Flush batch at line boundaries
                if let Some(batch) = current_batch.take() {
                    batched_runs.push(batch);
                }
                // Flush rect at line boundaries
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

                    // Handle background colors
                    let mut fg = cell.fg.clone();
                    let mut bg = cell.bg.clone();
                    if cell.flags.contains(Flags::INVERSE) {
                        std::mem::swap(&mut fg, &mut bg);
                    }

                    // Check if selected
                    // Use buffer_line (not visual row) to compare against selection bounds
                    // since selection is stored in buffer coordinates
                    let is_selected = if let Some(((start_col, start_row), (end_col, end_row))) = selection {
                        let (start_row, start_col, end_row, end_col) = if start_row < end_row || (start_row == end_row && start_col <= end_col) {
                            (start_row, start_col, end_row, end_col)
                        } else {
                            (end_row, end_col, start_row, start_col)
                        };
                        // Compare buffer_line (which accounts for scroll) against selection bounds
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

                    // Background rect batching
                    let bg_color = if is_selected {
                        Some(rgb(t.selection_bg).into())
                    } else if !is_default_bg(&bg, &t) {
                        Some(t.ansi_to_hsla(&bg))
                    } else {
                        None
                    };

                    if let Some(color) = bg_color {
                        if let Some(ref mut rect) = current_rect {
                            if rect.line == visual_line && rect.start_col + rect.num_cells as i32 == col_i32 && rect.color == color {
                                rect.extend();
                            } else {
                                rects.push(current_rect.take().unwrap());
                                current_rect = Some(LayoutRect::new(visual_line, col_i32, color));
                            }
                        } else {
                            current_rect = Some(LayoutRect::new(visual_line, col_i32, color));
                        }
                    } else if let Some(rect) = current_rect.take() {
                        rects.push(rect);
                    }

                    // Skip spacers and blanks
                    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        continue;
                    }
                    if cell.c == ' ' && !cell.flags.intersects(Flags::UNDERLINE | Flags::STRIKEOUT) {
                        continue;
                    }

                    // Create text style
                    let fg_color = if is_selected {
                        rgb(t.selection_fg).into()
                    } else {
                        t.ansi_to_hsla(&fg)
                    };

                    // Use pre-computed font variants to avoid repeated cloning
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
                            Some(UnderlineStyle {
                                color: Some(fg_color),
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

                    // Batch text runs
                    if let Some(ref mut batch) = current_batch {
                        if batch.can_append(&text_style, visual_line, col_i32) {
                            batch.append_char(cell.c);
                        } else {
                            batched_runs.push(current_batch.take().unwrap());
                            current_batch = Some(BatchedTextRun::new(visual_line, col_i32, cell.c, text_style));
                        }
                    } else {
                        current_batch = Some(BatchedTextRun::new(visual_line, col_i32, cell.c, text_style));
                    }
                }
            }

            // Flush remaining batches
            if let Some(batch) = current_batch {
                batched_runs.push(batch);
            }
            if let Some(rect) = current_rect {
                rects.push(rect);
            }

            // Phase 2: Paint backgrounds
            for rect in &rects {
                rect.paint(origin, cell_width, line_height, window);
            }

            // Phase 2.5: Paint search highlights
            for (idx, search_match) in self.search_matches.iter().enumerate() {
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
                    px((f32::from(origin.x) + search_match.col as f32 * cell_width_f).floor()),
                    origin.y + line_height * search_match.line as f32,
                );
                let size = size(
                    px((cell_width_f * search_match.len as f32).ceil()),
                    line_height,
                );

                window.paint_quad(fill(Bounds::new(position, size), highlight_color));
            }

            // Phase 2.6: Paint URL underlines
            for (idx, url_match) in self.url_matches.iter().enumerate() {
                // Only highlight the specific hovered URL instance, not all instances of the same URL
                let is_hovered = self.hovered_url_index == Some(idx);

                // Only draw visible URLs (within screen bounds)
                if url_match.line < 0 || url_match.line >= screen_lines as i32 {
                    continue;
                }

                let url_x = px((f32::from(origin.x) + url_match.col as f32 * cell_width_f).floor());
                let url_y = origin.y + line_height * url_match.line as f32;
                let url_width = px((cell_width_f * url_match.len as f32).ceil());

                if is_hovered {
                    // Hovered URL: background highlight + solid underline
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

                    // Solid underline for hovered URL
                    let underline_color = rgb(t.border_active);
                    let underline_y = url_y + line_height - px(2.0);
                    let underline_bounds = Bounds {
                        origin: point(url_x, underline_y),
                        size: size(url_width, px(1.0)),
                    };
                    window.paint_quad(fill(underline_bounds, underline_color));
                } else {
                    // Non-hovered URL: subtle dotted underline (rendered as dashed)
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
            for batch in &batched_runs {
                batch.paint(origin, cell_width, line_height, font_size, window, cx);
            }

            // Phase 4: Paint cursor (only if visible within current viewport and cursor_visible is true)
            // When scrolled into history (display_offset > 0), the cursor is at the bottom
            // of the active area and may be outside the visible viewport
            if cursor_visible {
                let cursor_point = term.grid().cursor.point;
                let cursor_visual_line = cursor_point.line.0 + display_offset;

                // Only paint cursor if it's within the visible viewport
                if cursor_visual_line >= 0 && cursor_visual_line < screen_lines as i32 {
                    let cursor_x = px((f32::from(origin.x) + cursor_point.column.0 as f32 * cell_width_f).floor());
                    let cursor_y = px((f32::from(origin.y) + cursor_visual_line as f32 * line_height_f).floor());

                    let cursor_bounds = Bounds {
                        origin: point(cursor_x, cursor_y),
                        size: size(cell_width, line_height),
                    };

                    // Block cursor with transparency
                    let cursor_rgba = rgb(t.cursor);
                    let cursor_color = Hsla::from(Rgba {
                        r: cursor_rgba.r,
                        g: cursor_rgba.g,
                        b: cursor_rgba.b,
                        a: 0.8,
                    });
                    window.paint_quad(fill(cursor_bounds, cursor_color));
                }
            }
        });
    }
}

/// Check if a color is the default background (should be transparent)
fn is_default_bg(color: &Color, t: &ThemeColors) -> bool {
    match color {
        Color::Named(NamedColor::Background) => true,
        Color::Indexed(idx) if *idx == 0 => false, // Black is not default bg
        Color::Spec(rgb_color) => {
            // Check if it matches the theme's terminal background
            let bg_r = ((t.term_background >> 16) & 0xFF) as u8;
            let bg_g = ((t.term_background >> 8) & 0xFF) as u8;
            let bg_b = (t.term_background & 0xFF) as u8;
            rgb_color.r == bg_r && rgb_color.g == bg_g && rgb_color.b == bg_b
        }
        _ => false,
    }
}
