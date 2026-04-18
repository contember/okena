use super::super::Terminal;
use super::super::types::{DetectedLink, TerminalSize};
use super::NullTransport;
use std::sync::Arc;

/// Helper: create a terminal and write text to it, returns detected URLs
fn detect_urls_in(text: &str, cols: u16) -> Vec<DetectedLink> {
    let transport = Arc::new(NullTransport);
    let size = TerminalSize { cols, rows: 24, cell_width: 8.0, cell_height: 16.0 };
    let terminal = Terminal::new("test".into(), size, transport, "/tmp".into());
    terminal.process_output(text.as_bytes());
    terminal.detect_urls()
}

#[test]
fn detect_url_wrapped_with_padding() {
    // TUI writes a URL with a decoration prefix, URL fills nearly the
    // whole row, then continues on next line with matching indentation.
    // No WRAPLINE flag — the TUI manages wrapping itself.
    // Row 1: "- https://claude.ai/code/sess_ABC" (33 chars)
    // Row 2: "  DEF123" + padding
    // cols=36 so row 1 is nearly full (33+3 >= 36).
    let links = detect_urls_in(
        "- https://claude.ai/code/sess_ABC\r\n  DEF123\r\n",
        36,
    );
    assert_eq!(links.len(), 2, "URL spans two rows: {:?}", links);
    assert_eq!(links[0].text, "https://claude.ai/code/sess_ABCDEF123");
    assert_eq!(links[0].col, 2);
    assert_eq!(links[1].text, "https://claude.ai/code/sess_ABCDEF123");
    assert_eq!(links[1].col, 2);
    assert_eq!(links[1].line, 1);
}

#[test]
fn detect_url_wrapped_with_leading_padding() {
    // TUI adds leading spaces on the continuation line for alignment
    // Row 1: "  https://claude.ai/code/sess_ABC" (33 chars) + padding
    // Row 2: "  DEF123" + padding
    // cols=36 so row 1 is nearly full (33+3 >= 36).
    let links = detect_urls_in(
        "  https://claude.ai/code/sess_ABC\r\n  DEF123\r\n",
        36,
    );
    assert_eq!(links.len(), 2, "URL spans two rows: {:?}", links);
    assert_eq!(links[0].text, "https://claude.ai/code/sess_ABCDEF123");
    assert_eq!(links[0].col, 2); // starts after 2 spaces
    assert_eq!(links[1].text, "https://claude.ai/code/sess_ABCDEF123");
    assert_eq!(links[1].col, 2); // continuation also at col 2
    assert_eq!(links[1].line, 1);
}

#[test]
fn detect_url_not_wrapped_when_next_line_more_indented() {
    // Next line has more leading spaces than the first line —
    // the extra indentation means it's NOT a URL continuation.
    // Reproduces: "   1. zkusí https://api.postmarkapp.com\n      (oficiální API)"
    let links = detect_urls_in(
        "   1. text https://api.postmarkapp.com\r\n      (next line)\r\n",
        50,
    );
    assert_eq!(links.len(), 1, "URL should NOT merge with next line: {:?}", links);
    assert_eq!(links[0].text, "https://api.postmarkapp.com");
}

#[test]
fn detect_url_single_line_not_affected() {
    // Single-line URL should still work normally
    let links = detect_urls_in(
        "visit https://example.com/path here\r\n",
        80,
    );
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].text, "https://example.com/path");
    assert_eq!(links[0].col, 6);
    assert_eq!(links[0].line, 0);
}

#[test]
fn detect_duplicate_urls_get_different_wrap_groups() {
    // Same URL on two separate lines should get different wrap_groups
    // so hovering one doesn't highlight the other.
    let links = detect_urls_in(
        "https://github.com/org/repo/pull/381\r\n\
         https://github.com/org/repo/pull/381\r\n",
        80,
    );
    assert_eq!(links.len(), 2, "Should detect two URLs: {:?}", links);
    assert_ne!(
        links[0].wrap_group, links[1].wrap_group,
        "Duplicate URLs must have different wrap_groups for independent hover"
    );
}

#[test]
fn detect_duplicate_urls_separated_by_blank_line() {
    // Same URL separated by a blank line
    let links = detect_urls_in(
        "https://github.com/org/repo/pull/381\r\n\
         \r\n\
         https://github.com/org/repo/pull/381\r\n",
        80,
    );
    assert_eq!(links.len(), 2, "Should detect two URLs: {:?}", links);
    assert_ne!(
        links[0].wrap_group, links[1].wrap_group,
        "Duplicate URLs must have different wrap_groups"
    );
}

#[test]
fn detect_duplicate_url_wrapped_then_whole() {
    // First URL wraps across two lines (TUI-style padding),
    // second URL appears whole on a later line.
    // This reproduces the real scenario from PR creation output.
    let url = "https://github.com/contember/webmaster/pull/381";
    let links = detect_urls_in(
        &format!(
            "Summary\r\n\
             prefix {url}\r\n\
             \r\n\
             PR created:\r\n\
             {url}\r\n"
        ),
        50,
    );
    let url_links: Vec<&DetectedLink> = links.iter()
        .filter(|l| l.text == url)
        .collect();
    // Wrapped URL produces 2 segments + standalone URL = 3 total
    assert!(url_links.len() >= 3, "Expected wrapped (2 segments) + standalone (1): {:?}", url_links);
    let wrapped_group = url_links[0].wrap_group;
    // All wrapped segments share the same group
    assert_eq!(url_links[0].wrap_group, url_links[1].wrap_group,
        "Wrapped segments should share wrap_group");
    // Standalone URL has a different group
    let standalone = url_links.last().unwrap();
    assert_ne!(wrapped_group, standalone.wrap_group,
        "Standalone URL must have different wrap_group than wrapped one");
}

#[test]
fn detect_duplicate_url_after_colon_prefix() {
    // "PR created:" ends with ':' which is a url_char.
    // The next line starts with a URL. Visual wrap detection should NOT
    // merge them — or if it does, they must still get different wrap_groups.
    let url = "https://github.com/org/repo/pull/381";
    let links = detect_urls_in(
        &format!(
            "{url}\r\n\
             \r\n\
             PR created:\r\n\
             {url}\r\n"
        ),
        80,
    );
    let url_links: Vec<&DetectedLink> = links.iter()
        .filter(|l| l.text == url)
        .collect();
    assert_eq!(url_links.len(), 2, "Should have exactly 2 URL matches: {:?}", url_links);
    assert_ne!(
        url_links[0].wrap_group, url_links[1].wrap_group,
        "URLs must have different wrap_groups even when preceded by colon"
    );
}

#[test]
fn detect_url_not_wrapped_when_next_line_starts_with_word() {
    // "Press ENTER..." is natural language text, not a URL continuation.
    // The URL ends with alphanumeric chars and "Press" starts with one,
    // but the word-followed-by-space heuristic should prevent merging.
    let links = detect_urls_in(
        "Login at:\r\nhttps://www.npmjs.com/login?next=/login/cli/d907c402-4ad4-474c-a183-16ae52157acf\r\nPress ENTER to open in the browser...\r\n",
        100,
    );
    assert_eq!(links.len(), 1, "Should detect exactly one URL: {:?}", links);
    assert_eq!(
        links[0].text,
        "https://www.npmjs.com/login?next=/login/cli/d907c402-4ad4-474c-a183-16ae52157acf"
    );
}

#[test]
fn detect_url_not_wrapped_when_next_line_word_after_wrapline() {
    // URL wraps via WRAPLINE (fills terminal width), then next line
    // after the wrap tail starts with a word — should not merge.
    let url = "https://www.npmjs.com/login?next=/login/cli/d907c402-4ad4-474c-a183-16ae52157acf";
    let links = detect_urls_in(
        &format!("{url}\r\nPress ENTER to open in the browser...\r\n"),
        60, // force URL to wrap via WRAPLINE
    );
    let url_links: Vec<&DetectedLink> = links.iter()
        .filter(|l| l.text == url)
        .collect();
    assert!(
        !url_links.is_empty(),
        "Should detect the URL: {:?}",
        links
    );
    // "Press" should NOT be part of any detected link
    assert!(
        links.iter().all(|l| !l.text.contains("Press")),
        "No link should contain 'Press': {:?}",
        links
    );
}

#[test]
fn detect_url_not_merged_with_remote_prefix() {
    // Git push output: URL on a line that doesn't fill the terminal width.
    // The "remote:" on the next line must NOT be merged as a continuation.
    let links = detect_urls_in(
        "remote:       https://github.com/contember/dotaz/pull/new/fixes\r\nremote:\r\n",
        80,
    );
    assert_eq!(links.len(), 1, "Should detect exactly one URL: {:?}", links);
    assert_eq!(links[0].text, "https://github.com/contember/dotaz/pull/new/fixes");
}

#[test]
fn detect_url_not_merged_with_label_suffix() {
    // Even when the URL line nearly fills the terminal, a continuation
    // ending with ':' (label pattern) must not be merged.
    let links = detect_urls_in(
        "https://github.com/contember/dotaz/pull/new/fixes\r\nremote:\r\n",
        52, // URL is 50 chars, nearly fills 52-col terminal
    );
    assert_eq!(links.len(), 1, "Label-like 'remote:' must not be merged: {:?}", links);
    assert_eq!(links[0].text, "https://github.com/contember/dotaz/pull/new/fixes");
}

#[test]
fn detect_url_wrapped_with_trailing_text() {
    // URL wraps across two lines, continuation line has non-URL text after
    // the URL part (e.g. " — S3 bucket").  The first token of the
    // continuation contains '/' so it should still be recognised as a URL
    // continuation.
    let links = detect_urls_in(
        "    - #61 https://github.com/contember/npi-infrastru\r\n    cture/pull/61 \u{2014} S3 bucket\r\n",
        55,
    );
    let url_links: Vec<&DetectedLink> = links.iter()
        .filter(|l| l.text == "https://github.com/contember/npi-infrastructure/pull/61")
        .collect();
    assert!(
        !url_links.is_empty(),
        "Should detect the full wrapped URL: {:?}",
        links
    );
}

#[test]
fn detect_url_wrapped_tui_narrow_layout() {
    // TUI uses a narrower layout than the terminal width.
    // URL doesn't reach the terminal edge but does reach the end
    // of the TUI's visible content.  Phase 2 should still extend.
    // Terminal is 55 cols, but TUI content only uses ~42 cols.
    let links = detect_urls_in(
        "\u{2514}  https://github.com/NPI-Cloud/npi-inf\r\n   rastructure/pull/64\r\n",
        55,
    );
    let url_links: Vec<&DetectedLink> = links.iter()
        .filter(|l| l.text == "https://github.com/NPI-Cloud/npi-infrastructure/pull/64")
        .collect();
    assert!(
        url_links.len() >= 2,
        "URL should span two rows even when TUI layout is narrower than terminal: {:?}",
        links
    );
}

#[test]
fn detect_url_not_extended_by_list_marker() {
    // URL on its own line followed by a list item starting with "- ".
    // The "-" is a url_char but it's a list marker, not a URL
    // continuation.  Must not extend.
    let links = detect_urls_in(
        "  https://github.com/contember/dotaz/pull/2\r\n  - Format check passes\r\n",
        55,
    );
    assert_eq!(links.len(), 1, "Should not extend into list marker: {:?}", links);
    assert_eq!(links[0].text, "https://github.com/contember/dotaz/pull/2");
}

#[test]
fn detect_url_extension_stops_after_trailing_trim() {
    // URL continuation ends with ")" which gets trimmed.  The "2." on
    // the following line must NOT be absorbed as another extension.
    // Simulates prose: "...npi-inf +\nrastructure/pull/65)\n2. https://..."
    let links = detect_urls_in(
        "  https://github.com/NPI-Cloud/npi-inf\r\n  rastructure/pull/65)\r\n  2. next item\r\n",
        42,
    );
    let url_links: Vec<&DetectedLink> = links.iter()
        .filter(|l| l.text.starts_with("https://github.com/NPI-Cloud/npi-inf"))
        .collect();
    // Should have 2 segments (line 0 + line 1), NOT 3
    assert_eq!(
        url_links.len(), 2,
        "Should not extend past trimmed ')' into '2.': {:?}",
        links
    );
    assert_eq!(
        url_links[0].text,
        "https://github.com/NPI-Cloud/npi-infrastructure/pull/65"
    );
}

#[test]
fn detect_url_not_extended_into_numbered_list_item() {
    // Numbered list where each item has a URL.  The `2` from "2. https://..."
    // must NOT be absorbed as a continuation of the first URL.
    let links = detect_urls_in(
        "1. https://github.com/contember/dotaz/pull/2\r\n2. https://github.com/NPI-Cloud/npi-infrastr\r\n   ucture/pull/65\r\n",
        46,
    );
    // First URL should be exactly pull/2, not pull/22
    let first: Vec<&DetectedLink> = links.iter()
        .filter(|l| l.text == "https://github.com/contember/dotaz/pull/2")
        .collect();
    assert!(
        !first.is_empty(),
        "First URL should be pull/2, not absorb '2' from next line: {:?}",
        links
    );
    // Second URL should also be detected
    let second: Vec<&DetectedLink> = links.iter()
        .filter(|l| l.text.contains("npi-infrastructure/pull/65"))
        .collect();
    assert!(
        !second.is_empty(),
        "Second URL should be detected: {:?}",
        links
    );
}

#[test]
fn detect_url_not_extended_by_prose_word() {
    // URL on a dash-list line, next line is also a dash-list item
    // with prose text.  "next" is a url_char word but must NOT be
    // absorbed as URL continuation.
    let links = detect_urls_in(
        "- https://github.com/contember/dotaz/pull/2\r\n- next item without URL\r\n",
        46,
    );
    assert_eq!(links.len(), 1, "Should not extend into 'next': {:?}", links);
    assert_eq!(links[0].text, "https://github.com/contember/dotaz/pull/2");
}

#[test]
fn detect_url_uuid_continuation_with_trailing_prose() {
    // URL wraps mid-UUID, continuation line has prose after the UUID
    // fragment.  The UUID part (digits + hex letters + dashes) must
    // still be recognised as URL continuation despite spaces in the
    // line — digits distinguish it from a prose word.
    let links = detect_urls_in(
        "  http://localhost:19400/s/1f41d02d-6105-45fb-b3\r\n  b1-4b56ae4d869f \u{2014} take your time.\r\n",
        50,
    );
    let url_links: Vec<&DetectedLink> = links.iter()
        .filter(|l| l.text == "http://localhost:19400/s/1f41d02d-6105-45fb-b3b1-4b56ae4d869f")
        .collect();
    assert!(
        url_links.len() >= 2,
        "UUID continuation should be detected across wrapped lines: {:?}",
        links
    );
}
