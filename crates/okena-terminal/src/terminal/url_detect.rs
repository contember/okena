/// Trim trailing punctuation from a URL/path, handling balanced parentheses.
///
/// Ghostty-style: strip trailing `.,:;!?)` but keep closing parens if they have
/// a matching opening paren inside the URL (e.g. Wikipedia links).
pub(super) fn trim_url_trailing(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut end = bytes.len();

    loop {
        if end == 0 {
            break;
        }
        let c = bytes[end - 1];
        match c {
            b'.' | b',' | b':' | b';' | b'!' | b'?' => {
                end -= 1;
            }
            b')' => {
                // Only strip closing paren if unbalanced
                let open = s[..end].matches('(').count();
                let close = s[..end].matches(')').count();
                if close > open {
                    end -= 1;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }

    &s[..end]
}

/// Parse optional `:line:col` suffix from a file path string.
/// Returns (display_text_including_suffix, optional_line, optional_col).
pub(super) fn parse_path_line_col(s: &str) -> (String, Option<u32>, Option<u32>) {
    // Try to match :line:col at the end
    if let Some(colon_pos) = s.rfind(':') {
        let after = &s[colon_pos + 1..];
        if let Ok(num) = after.parse::<u32>() {
            let before = &s[..colon_pos];
            // Check for another :line before this
            if let Some(colon_pos2) = before.rfind(':') {
                let after2 = &before[colon_pos2 + 1..];
                if let Ok(line_num) = after2.parse::<u32>() {
                    // path:line:col
                    return (s.to_string(), Some(line_num), Some(num));
                }
            }
            // path:line
            return (s.to_string(), Some(num), None);
        }
    }
    (s.to_string(), None, None)
}
