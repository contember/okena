//! Text utility functions shared across crates.

/// Whether a character is a "word" character (alphanumeric or underscore).
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Find the word boundaries (start, end) around a given byte offset.
///
/// Both `byte_col` and the returned `(start, end)` are **byte offsets** into `text`,
/// guaranteed to land on UTF-8 char boundaries.
pub fn find_word_boundaries(text: &str, byte_col: usize) -> (usize, usize) {
    if text.is_empty() {
        return (0, 0);
    }

    // Clamp to a valid char boundary at or before byte_col
    let byte_col = byte_col.min(text.len());
    let col = if text.is_char_boundary(byte_col) {
        byte_col
    } else {
        // Walk backwards to find a valid char boundary
        let mut b = byte_col;
        while b > 0 && !text.is_char_boundary(b) {
            b -= 1;
        }
        b
    };

    // Get the char at `col` (if col == text.len(), there is no char)
    let cur_char = text[col..].chars().next();
    let on_word = cur_char.map_or(false, |c| is_word_char(c));

    // Scan backwards for start (byte offset)
    let mut start = col;
    if on_word {
        while start > 0 {
            // Find the previous char boundary
            let mut prev = start - 1;
            while prev > 0 && !text.is_char_boundary(prev) {
                prev -= 1;
            }
            let prev_char = text[prev..].chars().next().unwrap();
            if is_word_char(prev_char) {
                start = prev;
            } else {
                break;
            }
        }
    }

    // Scan forwards for end (byte offset)
    let mut end = col;
    while end < text.len() {
        let next_char = text[end..].chars().next().unwrap();
        if is_word_char(next_char) {
            end += next_char.len_utf8();
        } else {
            break;
        }
    }

    (start, end)
}
