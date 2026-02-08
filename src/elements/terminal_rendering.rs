use crate::theme::ThemeColors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use gpui::*;

/// A batched text run that combines multiple adjacent cells with the same style (like Zed)
#[derive(Debug)]
pub(crate) struct BatchedTextRun {
    pub start_line: i32,
    pub start_col: i32,
    pub text: String,
    pub cell_count: usize,
    pub style: TextRun,
}

impl BatchedTextRun {
    pub fn new(start_line: i32, start_col: i32, c: char, style: TextRun) -> Self {
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

    pub fn can_append(&self, other_style: &TextRun, line: i32, col: i32) -> bool {
        self.start_line == line
            && self.start_col + self.cell_count as i32 == col
            && self.style.font == other_style.font
            && self.style.color == other_style.color
            && self.style.background_color == other_style.background_color
            && self.style.underline == other_style.underline
            && self.style.strikethrough == other_style.strikethrough
    }

    pub fn append_char(&mut self, c: char) {
        self.text.push(c);
        self.cell_count += 1;
        self.style.len += c.len_utf8();
    }

    pub fn paint(
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
pub(crate) struct LayoutRect {
    pub line: i32,
    pub start_col: i32,
    pub num_cells: usize,
    pub color: Hsla,
}

impl LayoutRect {
    pub fn new(line: i32, col: i32, color: Hsla) -> Self {
        LayoutRect {
            line,
            start_col: col,
            num_cells: 1,
            color,
        }
    }

    pub fn extend(&mut self) {
        self.num_cells += 1;
    }

    pub fn paint(&self, origin: Point<Pixels>, cell_width: Pixels, line_height: Pixels, window: &mut Window) {
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

/// Check if a color is the default background (should be transparent)
pub(crate) fn is_default_bg(color: &Color, t: &ThemeColors) -> bool {
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
