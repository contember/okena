//! Header pill tone and wording — spec §10. Pure; the render pass only paints it.

use super::super::labels::status as words;
use super::super::model::AnalysisStatus;

/// Dot colour of the pill. `Busy` draws a spinner glyph instead of a dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PillTone {
    Green,
    Amber,
    Red,
    Busy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PillView {
    pub tone: PillTone,
    pub text: String,
    /// A `details` link that opens the popover.
    pub has_details: bool,
}

/// Anything short of a full clean run stays amber or red — never green.
pub(crate) fn pill_view(status: &AnalysisStatus) -> PillView {
    match status {
        AnalysisStatus::LoadingInventory => PillView {
            tone: PillTone::Busy,
            text: words::LOADING_INVENTORY.to_string(),
            has_details: false,
        },
        AnalysisStatus::AnalyzingStructure => PillView {
            tone: PillTone::Busy,
            text: words::ANALYZING_STRUCTURE.to_string(),
            has_details: false,
        },
        AnalysisStatus::Ready { files, languages } => PillView {
            tone: PillTone::Green,
            text: words::ready_sentence(*files, languages),
            has_details: false,
        },
        AnalysisStatus::Limited { analyzed, total } => PillView {
            tone: PillTone::Amber,
            text: words::limited_sentence(*analyzed, *total),
            has_details: true,
        },
        AnalysisStatus::ReadyWithFailures { failed } => PillView {
            tone: PillTone::Amber,
            text: words::failures_sentence(*failed),
            has_details: true,
        },
        // The message is too long for the header; the hover tooltip carries it.
        AnalysisStatus::Unavailable { .. } => PillView {
            tone: PillTone::Red,
            text: words::UNAVAILABLE.to_string(),
            has_details: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::model::AnalysisStatus;
    use super::{PillTone, pill_view};

    /// Every `AnalysisStatus` variant name; none of them may reach the screen.
    const DEBUG_NAMES: [&str; 6] = [
        "LoadingInventory",
        "AnalyzingStructure",
        "Ready",
        "Limited",
        "ReadyWithFailures",
        "Unavailable",
    ];

    fn all_states() -> Vec<AnalysisStatus> {
        vec![
            AnalysisStatus::LoadingInventory,
            AnalysisStatus::AnalyzingStructure,
            AnalysisStatus::Ready {
                files: 385,
                languages: vec!["TS".into(), "TSX".into(), "Rust".into()],
            },
            AnalysisStatus::Limited {
                analyzed: 200,
                total: 385,
            },
            AnalysisStatus::ReadyWithFailures { failed: 3 },
            AnalysisStatus::Unavailable {
                message: "tree-sitter query failed".into(),
            },
        ]
    }

    #[test]
    fn every_state_has_its_spec_tone_text_and_details_link() {
        let expected = [
            (PillTone::Busy, "Loading inventory\u{2026}", false),
            (PillTone::Busy, "Analyzing structure\u{2026}", false),
            (
                PillTone::Green,
                "Structure ready \u{00B7} 385 files \u{00B7} TS, TSX, Rust",
                false,
            ),
            (
                PillTone::Amber,
                "Structure limited \u{00B7} 200 of 385 files",
                true,
            ),
            (
                PillTone::Amber,
                "Structure ready \u{00B7} 3 files failed to parse",
                true,
            ),
            (
                PillTone::Red,
                "Structure unavailable \u{00B7} diff still works",
                false,
            ),
        ];
        for (status, (tone, text, has_details)) in all_states().iter().zip(expected) {
            let view = pill_view(status);
            assert_eq!(view.tone, tone, "tone for {status:?}");
            assert_eq!(view.text, text, "text for {status:?}");
            assert_eq!(view.has_details, has_details, "details for {status:?}");
        }
    }

    #[test]
    fn a_capped_or_failed_run_is_never_green() {
        for analyzed in [0_u64, 1, 199, 384] {
            let view = pill_view(&AnalysisStatus::Limited {
                analyzed,
                total: 385,
            });
            assert_eq!(view.tone, PillTone::Amber);
            assert!(view.has_details);
        }
        for failed in [1_u64, 3, 4_200] {
            let view = pill_view(&AnalysisStatus::ReadyWithFailures { failed });
            assert_eq!(view.tone, PillTone::Amber);
            assert!(view.has_details);
        }
    }

    #[test]
    fn languages_are_joined_with_commas() {
        let view = pill_view(&AnalysisStatus::Ready {
            files: 2,
            languages: vec!["Rust".into(), "TSX".into()],
        });
        assert!(view.text.ends_with("Rust, TSX"), "{}", view.text);
    }

    #[test]
    fn the_failure_message_stays_out_of_the_header() {
        let view = pill_view(&AnalysisStatus::Unavailable {
            message: "tree-sitter query failed".into(),
        });
        assert!(
            !view.text.contains("tree-sitter query failed"),
            "{}",
            view.text
        );
        assert_eq!(view.text, "Structure unavailable \u{00B7} diff still works");
    }

    #[test]
    fn no_state_leaks_a_debug_enum_name() {
        for status in all_states() {
            let text = pill_view(&status).text;
            for name in DEBUG_NAMES {
                assert!(!text.contains(name), "{text} leaks {name}");
            }
        }
    }
}
