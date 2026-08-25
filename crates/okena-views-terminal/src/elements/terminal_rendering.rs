use alacritty_terminal::vte::ansi::{Color, NamedColor};
use gpui::*;
use okena_core::theme::ThemeColors;

/// A terminal row painted as one shaped line with multiple style runs.
#[derive(Debug)]
pub(crate) struct BatchedTextLine {
    pub line: i32,
    pub start_col: i32,
    pub text: String,
    pub styles: Vec<TextRun>,
    next_col: i32,
}

impl BatchedTextLine {
    pub fn new(line: i32, start_col: i32, c: char, style: TextRun) -> Self {
        let mut text = String::with_capacity(100);
        text.push(c);
        let mut style = style;
        style.len = c.len_utf8();
        Self {
            line,
            start_col,
            text,
            styles: vec![style],
            next_col: start_col + 1,
        }
    }

    pub fn append(&mut self, col: i32, c: char, style: TextRun) {
        debug_assert!(col >= self.next_col);
        if col > self.next_col {
            let gap = " ".repeat((self.next_col..col).count());
            let mut gap_style = style.clone();
            gap_style.background_color = None;
            gap_style.underline = None;
            gap_style.strikethrough = None;
            self.append_text(&gap, gap_style);
        }
        let mut encoded = [0; 4];
        self.append_text(c.encode_utf8(&mut encoded), style);
        self.next_col = col + 1;
    }

    fn append_text(&mut self, text: &str, mut style: TextRun) {
        self.text.push_str(text);
        style.len = text.len();
        if let Some(last) = self.styles.last_mut()
            && same_style(last, &style)
        {
            last.len += style.len;
        } else {
            self.styles.push(style);
        }
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
            origin.y + self.line as f32 * line_height,
        );

        let _ = window
            .text_system()
            .shape_line(
                self.text.clone().into(),
                font_size,
                &self.styles,
                Some(cell_width),
            )
            .paint(pos, line_height, TextAlign::Left, None, window, cx);
    }
}

fn same_style(left: &TextRun, right: &TextRun) -> bool {
    left.font == right.font
        && left.color == right.color
        && left.background_color == right.background_color
        && left.underline == right.underline
        && left.strikethrough == right.strikethrough
}

#[cfg(test)]
mod tests {
    use super::BatchedTextLine;
    use gpui::{StrikethroughStyle, TextRun, UnderlineStyle, px, rgb};

    fn style(color: u32) -> TextRun {
        TextRun {
            color: rgb(color).into(),
            ..TextRun::default()
        }
    }

    #[test]
    fn adjacent_cells_with_the_same_style_share_one_run() {
        let text_style = style(0x11_22_33);
        let mut line = BatchedTextLine::new(2, 3, 'a', text_style.clone());

        line.append(4, 'b', text_style);

        assert_eq!(line.line, 2);
        assert_eq!(line.start_col, 3);
        assert_eq!(line.text, "ab");
        assert_eq!(line.styles.len(), 1);
        assert_eq!(line.styles[0].len, 2);
        assert_eq!(line.next_col, 5);
    }

    #[test]
    fn skipped_cells_become_spaces_without_an_extra_paint_run() {
        let text_style = style(0x11_22_33);
        let mut line = BatchedTextLine::new(0, 1, 'a', text_style.clone());

        line.append(4, 'b', text_style);

        assert_eq!(line.text, "a  b");
        assert_eq!(line.styles.len(), 1);
        assert_eq!(line.styles[0].len, 4);
        assert_eq!(line.next_col, 5);
    }

    #[test]
    fn style_changes_remain_separate_inside_one_line() {
        let first = style(0x11_22_33);
        let second = style(0x44_55_66);
        let mut line = BatchedTextLine::new(0, 0, 'a', first);

        line.append(1, 'b', second);

        assert_eq!(line.text, "ab");
        assert_eq!(line.styles.len(), 2);
        assert_eq!(line.styles[0].len, 1);
        assert_eq!(line.styles[1].len, 1);
    }

    #[test]
    fn gaps_do_not_inherit_text_decorations() {
        let mut decorated = style(0x11_22_33);
        decorated.underline = Some(UnderlineStyle {
            color: None,
            thickness: px(1.0),
            wavy: false,
        });
        decorated.strikethrough = Some(StrikethroughStyle {
            color: None,
            thickness: px(1.0),
        });
        let mut line = BatchedTextLine::new(0, 0, 'a', decorated.clone());

        line.append(2, 'b', decorated);

        assert_eq!(line.text, "a b");
        assert_eq!(line.styles.len(), 3);
        assert!(line.styles[0].underline.is_some());
        assert!(line.styles[1].underline.is_none());
        assert!(line.styles[1].strikethrough.is_none());
        assert!(line.styles[2].underline.is_some());
    }

    #[test]
    fn a_wide_character_gap_preserves_following_column_alignment() {
        let text_style = style(0x11_22_33);
        let mut line = BatchedTextLine::new(0, 0, '界', text_style.clone());

        line.append(2, 'x', text_style);

        assert_eq!(line.text, "界 x");
        assert_eq!(line.styles.iter().map(|run| run.len).sum::<usize>(), 5);
        assert_eq!(line.next_col, 3);
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

    pub fn paint(
        &self,
        origin: Point<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        window: &mut Window,
    ) {
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
