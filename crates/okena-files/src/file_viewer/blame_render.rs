//! Render the per-line blame gutter column shown next to line numbers when
//! the user enables blame. Mirrors the layout of `git blame` / GitHub:
//! short hash, author, relative date.

use super::{BlameLoadState, FileViewer, FileViewerEvent};
use crate::blame::{BlameKind, BlameLine};
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::ui_text;

/// Total fixed width of the blame column in monospace character units.
/// Layout: 7 hash + 1 space + 8 author + 1 space + 4 date + 1 padding.
const BLAME_COL_CHARS: f32 = 22.0;
/// Pixels of padding on the right edge of the blame column, before the code.
const BLAME_COL_PADDING_RIGHT: f32 = 12.0;

impl FileViewer {
    /// Render the blame cell for a single line. Returns an empty div with the
    /// correct width when no blame data exists yet (skeleton/loading).
    pub(super) fn render_blame_cell(
        &self,
        line_number_0_based: usize,
        line_height: f32,
        char_width: f32,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.blame_visible {
            return None;
        }
        let tab = self.active_tab();
        let col_width = BLAME_COL_CHARS * char_width;

        match &tab.blame {
            BlameLoadState::Error(_) => None,
            BlameLoadState::NotLoaded | BlameLoadState::Loading => Some(
                div()
                    .w(px(col_width + BLAME_COL_PADDING_RIGHT))
                    .h(px(line_height))
                    .flex_shrink_0()
                    .into_any_element(),
            ),
            BlameLoadState::Loaded(lines) => {
                let current = lines.get(line_number_0_based)?;
                let prev = if line_number_0_based == 0 {
                    None
                } else {
                    lines.get(line_number_0_based - 1)
                };
                let show_header = prev
                    .map(|p| !std::sync::Arc::ptr_eq(&p.commit, &current.commit))
                    .unwrap_or(true);

                Some(render_blame_row(
                    current,
                    show_header,
                    col_width,
                    line_height,
                    t,
                    cx,
                ))
            }
        }
    }
}

fn render_blame_row(
    entry: &BlameLine,
    show_header: bool,
    col_width: f32,
    line_height: f32,
    t: &ThemeColors,
    cx: &mut Context<FileViewer>,
) -> AnyElement {
    if !show_header {
        return div()
            .w(px(col_width + BLAME_COL_PADDING_RIGHT))
            .h(px(line_height))
            .flex_shrink_0()
            .into_any_element();
    }

    match entry.kind {
        BlameKind::Uncommitted => render_uncommitted_cell(col_width, line_height, t, cx),
        BlameKind::Committed => render_committed_cell(entry, col_width, line_height, t, cx),
    }
}

fn render_uncommitted_cell(
    col_width: f32,
    line_height: f32,
    t: &ThemeColors,
    cx: &mut Context<FileViewer>,
) -> AnyElement {
    div()
        .w(px(col_width + BLAME_COL_PADDING_RIGHT))
        .h(px(line_height))
        .pr(px(BLAME_COL_PADDING_RIGHT))
        .flex()
        .items_center()
        .flex_shrink_0()
        .text_size(ui_text(11.0, cx))
        .text_color(rgb(t.text_muted))
        .child("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500} uncommitted   ")
        .into_any_element()
}

fn render_committed_cell(
    entry: &BlameLine,
    col_width: f32,
    line_height: f32,
    t: &ThemeColors,
    cx: &mut Context<FileViewer>,
) -> AnyElement {
    use gpui_component::tooltip::Tooltip;

    let hash = entry.commit.short_hash.clone();
    let author_full = entry.commit.author.clone();
    let author = truncate(&author_full, 8);
    let date = relative_date(entry.commit.timestamp);
    let summary = entry.commit.summary.clone();
    let email = entry.commit.author_email.clone();
    let absolute_date = absolute_date_string(entry.commit.timestamp);
    let full_hash = entry.commit.hash.clone();
    let tip_summary = summary.clone();

    // Fade older commits toward the muted text color so recent edits pop.
    let alpha = age_alpha(entry.commit.timestamp);
    let hash_color = t.term_yellow;
    let author_color = mix_alpha(t.text_secondary, alpha);
    let date_color = mix_alpha(t.text_muted, alpha);

    let element_id: ElementId =
        ElementId::Name(format!("blame-{}-{}", full_hash, entry.line_number).into());

    div()
        .id(element_id)
        .w(px(col_width + BLAME_COL_PADDING_RIGHT))
        .h(px(line_height))
        .pr(px(BLAME_COL_PADDING_RIGHT))
        .flex()
        .items_center()
        .gap(px(6.0))
        .flex_shrink_0()
        .text_size(ui_text(11.0, cx))
        .cursor_pointer()
        .hover(|s| s.bg(rgba_color(t.bg_hover, 0.35)))
        .on_click({
            let h = full_hash.clone();
            cx.listener(move |_, _, _, cx| {
                cx.emit(FileViewerEvent::OpenCommit(h.clone()));
            })
        })
        .tooltip(move |window, cx| {
            Tooltip::new(format!(
                "{}\n{} <{}>\n{}",
                tip_summary, author_full, email, absolute_date
            ))
            .build(window, cx)
        })
        .child(
            div()
                .w(px(7.0 * 7.0))
                .text_color(rgb(hash_color))
                .font_family("monospace")
                .child(hash),
        )
        .child(div().text_color(author_color).child(author))
        .child(div().text_color(date_color).child(date))
        .into_any_element()
}

/// Truncate a string to at most `max` chars, padding with spaces to keep
/// fixed-width alignment.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        let mut out: String = s.into();
        while out.chars().count() < max {
            out.push(' ');
        }
        out
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    }
}

fn relative_date(ts: i64) -> String {
    if ts == 0 {
        return "      ".to_string();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let diff = (now - ts).max(0) as u64;
    let s = if diff < 60 {
        "now".to_string()
    } else if diff < 3600 {
        format!("{}m", diff / 60)
    } else if diff < 86400 {
        format!("{}h", diff / 3600)
    } else if diff < 86400 * 30 {
        format!("{}d", diff / 86400)
    } else if diff < 86400 * 365 {
        format!("{}mo", diff / (86400 * 30))
    } else {
        format!("{}y", diff / (86400 * 365))
    };
    // Right-align to 4 chars for visual stability.
    if s.chars().count() < 4 {
        let pad = 4 - s.chars().count();
        format!("{}{}", " ".repeat(pad), s)
    } else {
        s
    }
}

fn absolute_date_string(ts: i64) -> String {
    if ts == 0 {
        return String::new();
    }
    // Lightweight YYYY-MM-DD HH:MM formatter without pulling chrono.
    use std::time::{Duration, UNIX_EPOCH};
    let when = UNIX_EPOCH + Duration::from_secs(ts as u64);
    let secs = when
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day, hh, mm) = unix_to_ymdhm(secs);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, month, day, hh, mm)
}

fn unix_to_ymdhm(mut secs: u64) -> (i32, u32, u32, u32, u32) {
    let days = secs / 86400;
    secs %= 86400;
    let hh = (secs / 3600) as u32;
    let mm = ((secs % 3600) / 60) as u32;
    let (year, month, day) = days_to_ymd(days as i64);
    (year, month, day, hh, mm)
}

/// Days-since-1970-01-01 → (year, month, day). Howard Hinnant's algorithm.
fn days_to_ymd(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = (y + if m <= 2 { 1 } else { 0 }) as i32;
    (year, m, d)
}

/// Linear fade based on commit age — recent edits stay full-strength,
/// year-old commits drop to ~0.45.
fn age_alpha(ts: i64) -> f32 {
    if ts == 0 {
        return 1.0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = ((now - ts).max(0) / 86400) as f32;
    let raw = 1.0 - (days / 365.0) * 0.55;
    raw.clamp(0.45, 1.0)
}

fn mix_alpha(rgb_color: u32, alpha: f32) -> Rgba {
    let r = ((rgb_color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((rgb_color >> 8) & 0xFF) as f32 / 255.0;
    let b = (rgb_color & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: alpha }
}

fn rgba_color(rgb_color: u32, alpha: f32) -> Rgba {
    mix_alpha(rgb_color, alpha)
}

#[cfg(test)]
mod tests {
    // `use gpui::*` at file scope shadows std's `#[test]` with gpui's test
    // macro; this module needs the std one for plain unit tests.
    use core::prelude::v1::test;

    use super::{age_alpha, days_to_ymd, relative_date, truncate, unix_to_ymdhm};

    #[test]
    fn truncate_pads_short() {
        assert_eq!(truncate("ab", 4), "ab  ");
    }

    #[test]
    fn truncate_ellipsizes_long() {
        assert_eq!(truncate("abcdefgh", 4), "abc\u{2026}");
    }

    #[test]
    fn truncate_keeps_exact_fit() {
        assert_eq!(truncate("abcd", 4), "abcd");
    }

    #[test]
    fn relative_date_buckets() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(relative_date(now).trim(), "now");
        assert_eq!(relative_date(now - 120).trim(), "2m");
        assert_eq!(relative_date(now - 7200).trim(), "2h");
        assert_eq!(relative_date(now - 86400 * 5).trim(), "5d");
        assert_eq!(relative_date(now - 86400 * 60).trim(), "2mo");
        assert_eq!(relative_date(now - 86400 * 400).trim(), "1y");
    }

    #[test]
    fn days_to_ymd_epoch_origin() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(31), (1970, 2, 1));
        assert_eq!(days_to_ymd(365), (1971, 1, 1));
    }

    #[test]
    fn unix_to_ymdhm_known_value() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        assert_eq!(unix_to_ymdhm(1704067200), (2024, 1, 1, 0, 0));
    }

    #[test]
    fn age_alpha_clamps() {
        assert!((age_alpha(0) - 1.0).abs() < f32::EPSILON);
        let ancient = age_alpha(1);
        assert!(ancient >= 0.45);
    }
}
