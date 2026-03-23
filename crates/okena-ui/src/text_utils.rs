//! Text utility functions shared across crates.

/// Whether a character is a "word" character (alphanumeric or underscore).
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Find the word boundaries (start, end) around a given column position.
pub fn find_word_boundaries(text: &str, col: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let col = col.min(chars.len().saturating_sub(1));

    // Scan backwards for start
    let mut start = col;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    // If cursor is on a non-word char, don't extend backwards
    if !is_word_char(chars[col]) {
        start = col;
    }

    // Scan forwards for end
    let mut end = col;
    while end < chars.len() && is_word_char(chars[end]) {
        end += 1;
    }

    (start, end)
}
