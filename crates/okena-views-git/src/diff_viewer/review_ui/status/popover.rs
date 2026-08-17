//! Rows of the details popover — spec §10. Pure; the render pass only paints them.

use super::super::labels::status as words;
use super::super::model::{AnalysisStatus, OmissionRow, ReviewModel};

/// One popover line: the sentence on the left, count and detail on the right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PopoverRow {
    pub sentence: String,
    pub detail: String,
    pub warn: bool,
}

/// What was reached first, then what needs attention, then the rest.
pub(crate) fn popover_rows(model: &ReviewModel) -> Vec<PopoverRow> {
    let coverage = &model.coverage;
    let mut rows = Vec::with_capacity(model.omissions.len() + 2);
    if coverage.analyzed_files > 0 {
        rows.push(PopoverRow {
            sentence: words::ANALYZED_ROW.to_string(),
            detail: words::analyzed_detail(coverage.analyzed_files, &coverage.languages),
            warn: false,
        });
    }
    // The pill only says "diff still works"; the reason lives here.
    if let AnalysisStatus::Unavailable { message } = &model.status {
        rows.push(PopoverRow {
            sentence: words::FAILED_ROW.to_string(),
            detail: message.clone(),
            warn: true,
        });
    }
    rows.extend(model.omissions.iter().filter(|row| row.warn).map(omission));
    rows.extend(model.omissions.iter().filter(|row| !row.warn).map(omission));
    rows
}

fn omission(row: &OmissionRow) -> PopoverRow {
    PopoverRow {
        sentence: row.sentence.clone(),
        detail: words::omission_detail(row.count, &row.detail),
        warn: row.warn,
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::model::{
        AnalysisStatus, CoverageSummary, DirNode, Facts, OmissionRow, ReviewModel,
    };
    use super::popover_rows;

    /// Every `AnalysisStatus` variant name; none of them may reach the screen.
    const DEBUG_NAMES: [&str; 6] = [
        "LoadingInventory",
        "AnalyzingStructure",
        "Ready",
        "Limited",
        "ReadyWithFailures",
        "Unavailable",
    ];

    fn model(status: AnalysisStatus, omissions: Vec<OmissionRow>) -> ReviewModel {
        ReviewModel {
            files: Vec::new(),
            root: DirNode::default(),
            volume: Vec::new(),
            total_changed_lines: 0,
            facts: Facts::default(),
            attention: Vec::new(),
            status,
            omissions,
            commits: Vec::new(),
            coverage: CoverageSummary {
                analyzed_files: 200,
                total_files: 385,
                languages: vec!["TypeScript".into(), "TSX".into()],
                partial: true,
                ..CoverageSummary::default()
            },
            small_change: false,
        }
    }

    fn omission(sentence: &str, count: u64, detail: &str, warn: bool) -> OmissionRow {
        OmissionRow {
            sentence: sentence.to_string(),
            count,
            detail: detail.to_string(),
            warn,
        }
    }

    #[test]
    fn analyzed_leads_then_the_rows_that_need_attention() {
        let rows = popover_rows(&model(
            AnalysisStatus::Limited {
                analyzed: 200,
                total: 385,
            },
            vec![
                omission("Skipped \u{2014} mode-only change", 9, "", false),
                omission("Not analyzed \u{2014} file limit (200)", 152, "", true),
                omission("Unsupported language", 21, ".astro 14, .js 5", false),
                omission("Failed to parse", 3, "parsing: unexpected token", true),
            ],
        ));
        let sentences: Vec<&str> = rows.iter().map(|row| row.sentence.as_str()).collect();
        assert_eq!(
            sentences,
            [
                "Analyzed",
                "Not analyzed \u{2014} file limit (200)",
                "Failed to parse",
                "Skipped \u{2014} mode-only change",
                "Unsupported language",
            ]
        );
        assert_eq!(rows[0].detail, "200 files \u{00B7} TypeScript, TSX");
        assert!(!rows[0].warn);
        assert!(rows[1].warn && rows[2].warn);
        assert_eq!(
            rows[2].detail,
            "3 files \u{00B7} parsing: unexpected token",
            "failure rows keep their stage and message"
        );
        assert!(!rows[3].warn && !rows[4].warn);
    }

    #[test]
    fn an_unavailable_run_shows_its_message_as_a_warn_row() {
        let mut model = model(
            AnalysisStatus::Unavailable {
                message: "tree-sitter query failed".into(),
            },
            Vec::new(),
        );
        model.coverage.analyzed_files = 0;
        let rows = popover_rows(&model);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sentence, "Structure analysis failed");
        assert_eq!(rows[0].detail, "tree-sitter query failed");
        assert!(rows[0].warn);
    }

    #[test]
    fn nothing_analyzed_means_no_analyzed_row() {
        let mut model = model(AnalysisStatus::AnalyzingStructure, Vec::new());
        model.coverage.analyzed_files = 0;
        assert!(popover_rows(&model).is_empty());
    }

    #[test]
    fn no_row_leaks_a_debug_enum_name() {
        let states = [
            AnalysisStatus::LoadingInventory,
            AnalysisStatus::AnalyzingStructure,
            AnalysisStatus::Ready {
                files: 385,
                languages: vec!["Rust".into()],
            },
            AnalysisStatus::Limited {
                analyzed: 200,
                total: 385,
            },
            AnalysisStatus::ReadyWithFailures { failed: 3 },
            AnalysisStatus::Unavailable {
                message: "worker exited".into(),
            },
        ];
        for status in states {
            for row in popover_rows(&model(status, Vec::new())) {
                for name in DEBUG_NAMES {
                    assert!(!row.sentence.contains(name), "{} leaks {name}", row.sentence);
                    assert!(!row.detail.contains(name), "{} leaks {name}", row.detail);
                }
            }
        }
    }
}
