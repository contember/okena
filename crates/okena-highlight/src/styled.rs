//! Turning highlighted spans into a GPUI `StyledText`.
//!
//! Shared by every code surface: the file viewer, the diff viewer, and markdown
//! code blocks all colour the same span data the same way.

use crate::syntax::HighlightedSpan;
use gpui::*;

/// Build a StyledText with optional background highlights (e.g. selection or word-level diff).
/// Splits syntax color highlights at background range boundaries to produce
/// non-overlapping highlights (required by `StyledText::compute_runs`).
pub fn build_styled_text_with_backgrounds(
    spans: &[HighlightedSpan],
    bg_ranges: &[(std::ops::Range<usize>, Hsla)],
) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();

    for span in spans {
        text.push_str(&span.text);
    }

    if bg_ranges.is_empty() {
        // Fast path: no background highlights, just syntax colors
        let mut offset = 0;
        for span in spans {
            let start = offset;
            offset += span.text.len();
            if start < offset {
                highlights.push((
                    start..offset,
                    HighlightStyle {
                        color: Some(span.color.into()),
                        ..Default::default()
                    },
                ));
            }
        }
    } else {
        // Split syntax spans at background range boundaries so no highlights overlap
        let mut offset = 0;
        for span in spans {
            let span_start = offset;
            let span_end = offset + span.text.len();
            offset = span_end;

            if span_start >= span_end {
                continue;
            }

            // Collect boundary points from bg_ranges that fall within this span
            let mut boundaries = vec![span_start];
            for (br, _) in bg_ranges {
                if br.start > span_start && br.start < span_end {
                    boundaries.push(br.start);
                }
                if br.end > span_start && br.end < span_end {
                    boundaries.push(br.end);
                }
            }
            boundaries.push(span_end);
            boundaries.sort();
            boundaries.dedup();

            for window in boundaries.windows(2) {
                let sub_start = window[0];
                let sub_end = window[1];
                if sub_start >= sub_end {
                    continue;
                }

                let mut style = HighlightStyle {
                    color: Some(span.color.into()),
                    ..Default::default()
                };

                // Apply background if this sub-range falls within any background range
                for (br, bg_color) in bg_ranges {
                    if sub_start >= br.start && sub_end <= br.end {
                        style.background_color = Some(*bg_color);
                        break;
                    }
                }

                highlights.push((sub_start..sub_end, style));
            }
        }
    }

    StyledText::new(text).with_highlights(highlights)
}
