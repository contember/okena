//! Dense smart-review lens rendering and its pure row models.

use super::DiffViewer;
use super::review::{LoadState, ReviewFileKey, ReviewLens};
use gpui::prelude::*;
use gpui::*;
use okena_core::review::{
    FileRole, ReviewCoverage, ReviewFileFact, ReviewFileStatus, ReviewInventory,
    ReviewNavigationTarget,
};
use okena_core::theme::ThemeColors;
use okena_files::theme::theme;
use okena_review::{
    CallChangeKind, CallDiffChange, FileAnalysisStatus, OutlineFact, ReviewStructure,
    StructuralMetric, StructuredFile, SymbolChange, SymbolChangeKind,
};
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowTone {
    Neutral,
    Added,
    Removed,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReviewRow {
    primary: String,
    secondary: String,
    trailing: String,
    tone: RowTone,
    target: Option<ReviewRowTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReviewRowTarget {
    File(ReviewFileKey),
    Evidence(ReviewNavigationTarget),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReviewSection {
    title: String,
    rows: Vec<ReviewRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReviewRenderModel {
    summary: Vec<(String, String)>,
    sections: Vec<ReviewSection>,
}

#[derive(Clone, Debug)]
pub(crate) struct ReviewRenderCache {
    content_revision: u64,
    selection_revision: u64,
    lens: ReviewLens,
    model: Arc<ReviewRenderModel>,
    flat_rows: Arc<Vec<FlatReviewRow>>,
    sidebar_rows: Arc<Vec<ReviewRow>>,
    roles: Arc<Vec<FileRole>>,
}

impl ReviewRenderCache {
    fn matches(&self, content_revision: u64, selection_revision: u64, lens: ReviewLens) -> bool {
        self.content_revision == content_revision
            && self.selection_revision == selection_revision
            && self.lens == lens
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FlatReviewRow {
    Header { title: String, count: usize },
    Empty,
    Entry(ReviewRow),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SmartDiffViewState {
    Idle,
    Loading,
    Failed(String),
    Empty,
    NoSelection,
    SourceIdle,
    SourceLoading,
    SourceFailed(String),
    DisplayLoading,
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DiffPhase {
    Idle,
    Loading,
    Failed(String),
    Empty,
    Files,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SourcePhase {
    Idle,
    Loading,
    Failed(String),
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoverageDataset {
    Inventory,
    Structure,
}

fn coverage_dataset_for_lens(lens: ReviewLens) -> CoverageDataset {
    match lens {
        ReviewLens::Inventory | ReviewLens::Diff => CoverageDataset::Inventory,
        ReviewLens::Structure | ReviewLens::CallDiff => CoverageDataset::Structure,
    }
}

fn classify_smart_diff_state(
    diff: DiffPhase,
    selected_in_diff: bool,
    source: SourcePhase,
    display_ready: bool,
) -> SmartDiffViewState {
    match diff {
        DiffPhase::Idle => SmartDiffViewState::Idle,
        DiffPhase::Loading => SmartDiffViewState::Loading,
        DiffPhase::Failed(error) => SmartDiffViewState::Failed(error),
        DiffPhase::Empty => SmartDiffViewState::Empty,
        DiffPhase::Files if !selected_in_diff => SmartDiffViewState::NoSelection,
        DiffPhase::Files => match source {
            SourcePhase::Idle => SmartDiffViewState::SourceIdle,
            SourcePhase::Loading => SmartDiffViewState::SourceLoading,
            SourcePhase::Failed(error) => SmartDiffViewState::SourceFailed(error),
            SourcePhase::Ready if !display_ready => SmartDiffViewState::DisplayLoading,
            SourcePhase::Ready => SmartDiffViewState::Ready,
        },
    }
}

fn exact_display_ready(
    canonical: Option<&ReviewFileKey>,
    file_key: Option<&ReviewFileKey>,
    source_matches: bool,
    cache_ready: bool,
    display_ready: bool,
) -> bool {
    canonical.is_some() && canonical == file_key && source_matches && cache_ready && display_ready
}

fn flatten_sections(sections: &[ReviewSection]) -> Vec<FlatReviewRow> {
    let mut rows = Vec::new();
    for section in sections {
        rows.push(FlatReviewRow::Header {
            title: section.title.clone(),
            count: section.rows.len(),
        });
        if section.rows.is_empty() {
            rows.push(FlatReviewRow::Empty);
        } else {
            rows.extend(section.rows.iter().cloned().map(FlatReviewRow::Entry));
        }
    }
    rows
}

fn role_label(role: FileRole) -> &'static str {
    match role {
        FileRole::Implementation => "Implementation",
        FileRole::Test => "Tests",
        FileRole::Fixture => "Fixtures",
        FileRole::Snapshot => "Snapshots",
        FileRole::Example => "Examples",
        FileRole::Documentation => "Documentation",
        FileRole::Generated => "Generated",
        FileRole::Vendored => "Vendored",
        FileRole::Lockfile => "Lockfiles",
        FileRole::Configuration => "Configuration",
        FileRole::Unclassified => "Unclassified",
    }
}

fn status_label(status: ReviewFileStatus) -> &'static str {
    match status {
        ReviewFileStatus::Added => "added",
        ReviewFileStatus::Deleted => "deleted",
        ReviewFileStatus::Modified => "modified",
        ReviewFileStatus::Renamed => "renamed",
        ReviewFileStatus::Copied => "copied",
        ReviewFileStatus::TypeChanged => "type changed",
        ReviewFileStatus::ModeChanged => "mode changed",
        ReviewFileStatus::SubmoduleChanged => "submodule changed",
        ReviewFileStatus::Unmerged => "unmerged",
        ReviewFileStatus::Unknown => "unknown",
    }
}

fn status_tone(status: ReviewFileStatus) -> RowTone {
    match status {
        ReviewFileStatus::Added | ReviewFileStatus::Copied => RowTone::Added,
        ReviewFileStatus::Deleted => RowTone::Removed,
        ReviewFileStatus::Modified
        | ReviewFileStatus::Renamed
        | ReviewFileStatus::TypeChanged
        | ReviewFileStatus::ModeChanged
        | ReviewFileStatus::SubmoduleChanged
        | ReviewFileStatus::Unknown => RowTone::Neutral,
        ReviewFileStatus::Unmerged => RowTone::Warning,
    }
}

fn file_path(file: &ReviewFileFact) -> String {
    match (&file.old_path, &file.new_path) {
        (Some(old), Some(new)) if old != new => format!("{old} → {new}"),
        (_, Some(new)) => new.clone(),
        (Some(old), None) => old.clone(),
        (None, None) => "(unknown path)".to_string(),
    }
}

fn inventory_model(
    inventory: &ReviewInventory,
    role_filter: Option<FileRole>,
) -> ReviewRenderModel {
    let totals = &inventory.totals;
    let summary = vec![
        ("Commits".into(), totals.commits.to_string()),
        ("Files".into(), totals.files.to_string()),
        ("Added".into(), format!("+{}", totals.lines_added)),
        ("Deleted".into(), format!("-{}", totals.lines_deleted)),
        (
            "Coverage".into(),
            format!(
                "{} / {}",
                inventory.coverage.analyzed_items(),
                inventory.coverage.total_items()
            ),
        ),
    ];

    let commits = inventory
        .commits
        .iter()
        .map(|commit| ReviewRow {
            primary: commit.subject.clone(),
            secondary: format!("{} · {}", commit.author_name, commit.timestamp),
            trailing: commit.oid.as_str().chars().take(8).collect(),
            tone: RowTone::Neutral,
            target: None,
        })
        .collect();
    let files = inventory
        .files
        .iter()
        .filter(|file| role_filter.is_none_or(|role| file.classification.role() == role))
        .map(|file| {
            let added = file
                .lines_added
                .map_or_else(|| "-".into(), |n| format!("+{n}"));
            let deleted = file
                .lines_deleted
                .map_or_else(|| "-".into(), |n| format!("-{n}"));
            let flags = [
                file.binary.then_some("binary"),
                file.submodule.as_ref().map(|_| "submodule"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            ReviewRow {
                primary: file_path(file),
                secondary: format!(
                    "{} · {} · {} · {:?}{}",
                    role_label(file.classification.role()),
                    status_label(file.status),
                    file.classification.rule_id().as_str(),
                    file.provenance,
                    if flags.is_empty() {
                        String::new()
                    } else {
                        format!(" · {flags}")
                    }
                ),
                trailing: format!("{added} {deleted}"),
                tone: status_tone(file.status),
                target: Some(ReviewRowTarget::File(ReviewFileKey::from_inventory(file))),
            }
        })
        .collect();

    ReviewRenderModel {
        summary,
        sections: vec![
            ReviewSection {
                title: "Role distribution".into(),
                rows: [
                    FileRole::Implementation,
                    FileRole::Test,
                    FileRole::Fixture,
                    FileRole::Snapshot,
                    FileRole::Example,
                    FileRole::Documentation,
                    FileRole::Generated,
                    FileRole::Vendored,
                    FileRole::Lockfile,
                    FileRole::Configuration,
                    FileRole::Unclassified,
                ]
                .into_iter()
                .filter_map(|role| {
                    let count = inventory
                        .files
                        .iter()
                        .filter(|file| file.classification.role() == role)
                        .count();
                    (count > 0).then(|| ReviewRow {
                        primary: role_label(role).into(),
                        secondary: "files".into(),
                        trailing: count.to_string(),
                        tone: RowTone::Neutral,
                        target: None,
                    })
                })
                .collect(),
            },
            ReviewSection {
                title: "Status distribution".into(),
                rows: [
                    ReviewFileStatus::Added,
                    ReviewFileStatus::Deleted,
                    ReviewFileStatus::Modified,
                    ReviewFileStatus::Renamed,
                    ReviewFileStatus::Copied,
                    ReviewFileStatus::TypeChanged,
                    ReviewFileStatus::ModeChanged,
                    ReviewFileStatus::SubmoduleChanged,
                    ReviewFileStatus::Unmerged,
                    ReviewFileStatus::Unknown,
                ]
                .into_iter()
                .filter_map(|status| {
                    let count = inventory
                        .files
                        .iter()
                        .filter(|file| file.status == status)
                        .count();
                    (count > 0).then(|| ReviewRow {
                        primary: status_label(status).into(),
                        secondary: "files".into(),
                        trailing: count.to_string(),
                        tone: status_tone(status),
                        target: None,
                    })
                })
                .collect(),
            },
            ReviewSection {
                title: "Commit ledger".into(),
                rows: commits,
            },
            ReviewSection {
                title: "Files".into(),
                rows: files,
            },
        ],
    }
}

fn analysis_status_label(status: FileAnalysisStatus) -> &'static str {
    match status {
        FileAnalysisStatus::Parsed => "parsed",
        FileAnalysisStatus::Partial => "partial",
        FileAnalysisStatus::Pending => "pending",
        FileAnalysisStatus::Unsupported => "unsupported",
        FileAnalysisStatus::Failed => "failed",
        FileAnalysisStatus::Skipped => "skipped",
    }
}

fn matches_key(file: &StructuredFile, key: &ReviewFileKey) -> bool {
    file.old_path() == key.old_path.as_deref() && file.new_path() == key.new_path.as_deref()
}

fn qualified_symbol_name(qualified_path: &[String], name: &str) -> String {
    let mut parts = qualified_path.to_vec();
    if parts.last().is_none_or(|part| part != name) {
        parts.push(name.to_string());
    }
    parts.join("::")
}

fn symbol_name(change: &SymbolChange) -> String {
    change
        .new_fact()
        .or_else(|| change.old())
        .map(|fact| qualified_symbol_name(fact.key().qualified_path(), fact.key().name()))
        .unwrap_or_else(|| "(unknown symbol)".into())
}

fn symbol_change_row(change: &SymbolChange) -> ReviewRow {
    let dimensions = match (change.signature_change().is_some(), change.body_changed()) {
        (true, true) => "signature + body",
        (true, false) => "signature",
        (false, true) => "body",
        (false, false) => match change.kind() {
            SymbolChangeKind::Added => "added",
            SymbolChangeKind::Removed => "removed",
            SymbolChangeKind::Modified => "modified",
        },
    };
    let signature = change.signature_change().map_or_else(
        || {
            change
                .new_fact()
                .or_else(|| change.old())
                .map(|fact| fact.normalized_signature().to_string())
                .unwrap_or_default()
        },
        |signature| {
            format!(
                "{} → {}",
                signature.old_signature(),
                signature.new_signature()
            )
        },
    );
    ReviewRow {
        primary: symbol_name(change),
        secondary: if signature.is_empty() {
            format!(
                "{dimensions} · -{} +{}",
                change.changed_old_lines(),
                change.changed_new_lines()
            )
        } else {
            format!(
                "{dimensions} · -{} +{} · {signature}",
                change.changed_old_lines(),
                change.changed_new_lines()
            )
        },
        trailing: format!("{}:{}", change.navigation().path, change.navigation().line),
        tone: match change.kind() {
            SymbolChangeKind::Added => RowTone::Added,
            SymbolChangeKind::Removed => RowTone::Removed,
            SymbolChangeKind::Modified => RowTone::Neutral,
        },
        target: Some(ReviewRowTarget::Evidence(change.navigation().clone())),
    }
}

fn hotspot_row(hotspot: &okena_review::StructuralHotspot) -> ReviewRow {
    let (label, value) = match hotspot.metric() {
        StructuralMetric::FunctionLineCount { lines } => ("function lines", lines.to_string()),
        StructuralMetric::ChangedLines { old, new } => ("changed lines", format!("-{old} +{new}")),
        StructuralMetric::ParameterCount { parameters } => ("parameters", parameters.to_string()),
        StructuralMetric::SyntacticNestingDepth { depth } => ("nesting", depth.to_string()),
        StructuralMetric::TypeMemberCount { members } => ("members", members.to_string()),
    };
    ReviewRow {
        primary: qualified_symbol_name(
            hotspot.symbol().key().qualified_path(),
            hotspot.symbol().key().name(),
        ),
        secondary: format!("{label} · {value}"),
        trailing: format!(
            "{}:{}",
            hotspot.navigation().path,
            hotspot.navigation().line
        ),
        tone: RowTone::Neutral,
        target: Some(ReviewRowTarget::Evidence(hotspot.navigation().clone())),
    }
}

fn outline_rows(side: &str, roots: &[OutlineFact]) -> Vec<ReviewRow> {
    let mut rows = Vec::new();
    let mut stack = roots
        .iter()
        .rev()
        .map(|fact| (fact, 0_usize))
        .collect::<Vec<_>>();
    while let Some((fact, depth)) = stack.pop() {
        rows.push(ReviewRow {
            primary: format!("{}{}", "  ".repeat(depth), fact.symbol().key().name()),
            secondary: format!("{side} · {:?}", fact.symbol().key().kind()),
            trailing: fact.symbol().range().start_line().to_string(),
            tone: RowTone::Neutral,
            target: None,
        });
        stack.extend(fact.children().iter().rev().map(|child| (child, depth + 1)));
    }
    rows
}

fn structure_model(structure: &ReviewStructure, key: Option<&ReviewFileKey>) -> ReviewRenderModel {
    let coverage = structure.coverage();
    let selected = key.and_then(|key| structure.files().iter().find(|file| matches_key(file, key)));
    let mut summary = vec![
        ("Analyzed".into(), coverage.analyzed_items().to_string()),
        ("Pending".into(), coverage.pending_items().to_string()),
        ("Skipped".into(), coverage.skipped_items().to_string()),
        (
            "Unsupported".into(),
            coverage.unsupported_items().to_string(),
        ),
        ("Failed".into(), coverage.failed_items().to_string()),
    ];
    if let Some(file) = selected {
        summary.push((
            "Selected".into(),
            analysis_status_label(file.status()).into(),
        ));
        let parser = match (file.old_provenance(), file.new_provenance()) {
            (Some(old), Some(new)) if old != new => {
                format!("{} → {}", old.parser(), new.parser())
            }
            (Some(provenance), _) | (_, Some(provenance)) => provenance.parser().into(),
            (None, None) => "not parsed".into(),
        };
        summary.push(("Parser".into(), parser));
    }

    let mut sections = Vec::new();
    if let Some(file) = selected {
        sections.push(ReviewSection {
            title: "Symbol changes".into(),
            rows: file
                .symbol_changes()
                .iter()
                .map(symbol_change_row)
                .collect(),
        });
        sections.push(ReviewSection {
            title: "Hotspots".into(),
            rows: file.hotspots().iter().map(hotspot_row).collect(),
        });
        let mut outline = outline_rows("base", file.old_outline());
        outline.extend(outline_rows("head", file.new_outline()));
        sections.push(ReviewSection {
            title: "Outline".into(),
            rows: outline,
        });
        sections.push(ReviewSection {
            title: "File errors".into(),
            rows: file
                .errors()
                .iter()
                .map(|error| ReviewRow {
                    primary: error.message().into(),
                    secondary: format!("{:?}", error.stage()),
                    trailing: error.path().unwrap_or_default().into(),
                    tone: RowTone::Warning,
                    target: None,
                })
                .chain(file.truncation().into_iter().map(|truncation| ReviewRow {
                    primary: format!("{:?}", truncation.reason),
                    secondary: truncation.detail.clone().unwrap_or_default(),
                    trailing: format!(
                        "{} / {}",
                        truncation.observed.map_or_else(|| "-".into(), |n| n.to_string()),
                        truncation.limit.map_or_else(|| "-".into(), |n| n.to_string())
                    ),
                    tone: RowTone::Warning,
                    target: None,
                }))
                .collect(),
        });
    } else {
        sections.push(ReviewSection {
            title: "Selected file".into(),
            rows: vec![ReviewRow {
                primary: "No structured facts for the selected path pair".into(),
                secondary: key.map_or_else(
                    || "No file selected".into(),
                    |key| {
                        format!(
                            "{} → {}",
                            key.old_path.as_deref().unwrap_or("∅"),
                            key.new_path.as_deref().unwrap_or("∅")
                        )
                    },
                ),
                trailing: String::new(),
                tone: RowTone::Warning,
                target: key.cloned().map(ReviewRowTarget::File),
            }],
        });
    }
    sections.push(ReviewSection {
        title: "Language coverage".into(),
        rows: structure
            .language_coverage()
            .iter()
            .map(|entry| ReviewRow {
                primary: format!("{:?}", entry.language()),
                secondary: format!(
                    "{} analyzed · {} pending · {} skipped · {} unsupported · {} failed{}",
                    entry.coverage().analyzed_items(),
                    entry.coverage().pending_items(),
                    entry.coverage().skipped_items(),
                    entry.coverage().unsupported_items(),
                    entry.coverage().failed_items(),
                    entry
                        .coverage()
                        .truncation()
                        .map_or_else(String::new, |truncation| {
                            format!(
                                " · {:?} · {} / {} · {}",
                                truncation.reason,
                                truncation
                                    .observed
                                    .map_or_else(|| "-".into(), |n| n.to_string()),
                                truncation
                                    .limit
                                    .map_or_else(|| "-".into(), |n| n.to_string()),
                                truncation.detail.as_deref().unwrap_or("")
                            )
                        })
                ),
                trailing: entry.coverage().total_items().to_string(),
                tone: if entry.coverage().is_complete() {
                    RowTone::Neutral
                } else {
                    RowTone::Warning
                },
                target: None,
            })
            .collect(),
    });
    sections.push(ReviewSection {
        title: "Aggregate omissions".into(),
        rows: structure
            .omissions()
            .iter()
            .map(|omission| ReviewRow {
                primary: format!("{:?}", omission.reason()),
                secondary: format!(
                    "{} · {}{}",
                    omission.language().map_or_else(
                        || "not language-specific".into(),
                        |language| format!("{language:?}")
                    ),
                    analysis_status_label(omission.status()),
                    omission
                        .truncation()
                        .map_or_else(String::new, |truncation| format!(
                            " · {:?} · {} / {} · {}",
                            truncation.reason,
                            truncation
                                .observed
                                .map_or_else(|| "-".into(), |n| n.to_string()),
                            truncation
                                .limit
                                .map_or_else(|| "-".into(), |n| n.to_string()),
                            truncation.detail.as_deref().unwrap_or("")
                        ))
                ),
                trailing: omission.count().to_string(),
                tone: RowTone::Warning,
                target: None,
            })
            .collect(),
    });
    sections.push(ReviewSection {
        title: "Aggregate errors".into(),
        rows: structure
            .errors()
            .iter()
            .map(|error| ReviewRow {
                primary: error.message().into(),
                secondary: format!("{:?}", error.stage()),
                trailing: error.path().unwrap_or_default().into(),
                tone: RowTone::Warning,
                target: None,
            })
            .collect(),
    });
    ReviewRenderModel { summary, sections }
}

fn call_fact_label(change: &CallDiffChange) -> String {
    change
        .new_fact()
        .or_else(|| change.old())
        .map_or_else(|| "(unknown call)".into(), |call| call.callee_text().into())
}

fn call_change_row(change: &CallDiffChange) -> ReviewRow {
    let dimensions = match (change.arguments_changed(), change.control_context_changed()) {
        (true, true) => "arguments + control context",
        (true, false) => "arguments",
        (false, true) => "control context",
        (false, false) => match change.kind() {
            CallChangeKind::Added => "added",
            CallChangeKind::Removed => "removed",
            CallChangeKind::Modified => "modified",
        },
    };
    let argument_delta = match (change.old(), change.new_fact()) {
        (Some(old), Some(new)) => format!("{} → {}", old.argument_text(), new.argument_text()),
        (None, Some(new)) => new.argument_text().into(),
        (Some(old), None) => old.argument_text().into(),
        (None, None) => String::new(),
    };
    let mut evidence = change.pairing().map_or_else(
        || dimensions.to_string(),
        |pairing| {
            format!(
                "{dimensions} · {:?} · {}:{}",
                pairing.strategy(),
                pairing.old_candidate_count(),
                pairing.new_candidate_count()
            )
        },
    );
    if change.control_context_changed() {
        let old = change.old().map_or_else(
            || "∅".into(),
            |call| format!("{:?}", call.control_context()),
        );
        let new = change.new_fact().map_or_else(
            || "∅".into(),
            |call| format!("{:?}", call.control_context()),
        );
        evidence.push_str(&format!(" · {old} → {new}"));
    }
    if let Some(caller) = change
        .new_fact()
        .or_else(|| change.old())
        .and_then(|call| call.enclosing_symbol())
    {
        evidence.push_str(&format!(
            " · in {}",
            qualified_symbol_name(caller.qualified_path(), caller.name())
        ));
    }
    ReviewRow {
        primary: call_fact_label(change),
        secondary: if argument_delta.is_empty() {
            evidence
        } else {
            format!("{evidence} · {argument_delta}")
        },
        trailing: format!("{}:{}", change.navigation().path, change.navigation().line),
        tone: match change.kind() {
            CallChangeKind::Added => RowTone::Added,
            CallChangeKind::Removed => RowTone::Removed,
            CallChangeKind::Modified => RowTone::Neutral,
        },
        target: Some(ReviewRowTarget::Evidence(change.navigation().clone())),
    }
}

fn call_diff_model(structure: &ReviewStructure, key: Option<&ReviewFileKey>) -> ReviewRenderModel {
    let selected = key.and_then(|key| structure.files().iter().find(|file| matches_key(file, key)));
    let rows: Vec<ReviewRow> = selected
        .map(|file| file.call_diff().iter().map(call_change_row).collect())
        .unwrap_or_default();
    ReviewRenderModel {
        summary: vec![
            ("Changes".into(), rows.len().to_string()),
            (
                "File status".into(),
                selected.map_or_else(
                    || "not represented".into(),
                    |file| analysis_status_label(file.status()).into(),
                ),
            ),
        ],
        sections: vec![ReviewSection {
            title: "Direct call changes".into(),
            rows,
        }],
    }
}

impl DiffViewer {
    pub(super) fn render_review_lens_strip(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.smart_review.lens;
        div()
            .flex()
            .items_center()
            .h(px(38.0))
            .px(px(16.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .children(ReviewLens::ALL.into_iter().map(|lens| {
                let selected = lens == active;
                div()
                    .id(SharedString::from(format!("review-lens-{lens:?}")))
                    .h_full()
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .border_b_2()
                    .border_color(rgb(if selected {
                        t.border_active
                    } else {
                        t.bg_primary
                    }))
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(if selected {
                        t.text_primary
                    } else {
                        t.text_muted
                    }))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(t.bg_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.smart_review.set_lens(lens);
                        cx.notify();
                    }))
                    .child(match lens {
                        ReviewLens::Inventory => "Inventory",
                        ReviewLens::Structure => "Structure",
                        ReviewLens::CallDiff => "CallDiff",
                        ReviewLens::Diff => "Diff",
                    })
            }))
            .into_any_element()
    }

    pub(super) fn smart_diff_view_state(&self) -> SmartDiffViewState {
        let (phase, dataset) = match &self.smart_review.diff {
            LoadState::Idle => (DiffPhase::Idle, None),
            LoadState::Loading => (DiffPhase::Loading, None),
            LoadState::Failed(error) => (DiffPhase::Failed(error.clone()), None),
            LoadState::Ready(dataset) if dataset.files.is_empty() => (DiffPhase::Empty, None),
            LoadState::Ready(dataset) => (DiffPhase::Files, Some(dataset)),
        };
        let selected_in_diff = dataset.is_some_and(|dataset| {
            self.smart_review.selected_file.as_ref().is_some_and(|key| {
                dataset
                    .files
                    .iter()
                    .any(|file| file.old_path == key.old_path && file.new_path == key.new_path)
            })
        });
        let canonical = self.smart_review.selected_file.as_ref();
        let source = match &self.smart_review.file.source {
            LoadState::Idle => SourcePhase::Idle,
            LoadState::Loading => SourcePhase::Loading,
            LoadState::Failed(error) => SourcePhase::Failed(error.clone()),
            LoadState::Ready(source)
                if canonical.is_none_or(|key| {
                    self.smart_review.file.key.as_ref() != Some(key) || !key.matches_source(source)
                }) =>
            {
                SourcePhase::Failed("Exact source does not match canonical selection".into())
            }
            LoadState::Ready(_) => SourcePhase::Ready,
        };
        let source_matches = matches!(
            &self.smart_review.file.source,
            LoadState::Ready(source)
                if canonical.is_some_and(|key| key.matches_source(source))
        );
        let cache_ready =
            canonical.is_some_and(|key| self.smart_review.file.has_ready_cache(key, true));
        let display_ready = exact_display_ready(
            canonical,
            self.smart_review.file.key.as_ref(),
            source_matches,
            cache_ready,
            self.current_file.is_some(),
        );
        classify_smart_diff_state(phase, selected_in_diff, source, display_ready)
    }

    pub(super) fn render_smart_diff_state(
        &self,
        state: SmartDiffViewState,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, detail) = match state {
            SmartDiffViewState::Idle => ("Diff is not loaded", None),
            SmartDiffViewState::Loading => ("Loading exact diff…", None),
            SmartDiffViewState::Failed(error) => ("Exact diff failed", Some(error)),
            SmartDiffViewState::Empty => ("No changed files", None),
            SmartDiffViewState::NoSelection => ("No exact file selected", None),
            SmartDiffViewState::SourceIdle => ("Exact source is not loaded", None),
            SmartDiffViewState::SourceLoading => ("Loading exact source…", None),
            SmartDiffViewState::SourceFailed(error) => ("Exact source failed", Some(error)),
            SmartDiffViewState::DisplayLoading => ("Preparing diff display…", None),
            SmartDiffViewState::Ready => return div().into_any_element(),
        };
        self.render_review_state(title, detail.as_deref(), t, cx)
    }

    pub(super) fn render_review_selection(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .h(px(30.0))
            .px(px(16.0))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(t.border))
            .font_family("monospace")
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(
                self.smart_review
                    .selected_file
                    .as_ref()
                    .map_or_else(|| "No file selected".into(), ReviewFileKey::display),
            )
            .into_any_element()
    }

    pub(super) fn render_review_coverage(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match coverage_dataset_for_lens(self.smart_review.lens) {
            CoverageDataset::Inventory => {
                let label = if self.smart_review.lens == ReviewLens::Diff {
                    "Inventory (Diff file set)"
                } else {
                    "Inventory"
                };
                match &self.smart_review.inventory {
                    LoadState::Idle => self.render_coverage_state(label, "idle", None, t, cx),
                    LoadState::Loading => self.render_coverage_state(label, "loading", None, t, cx),
                    LoadState::Failed(error) => {
                        self.render_coverage_state(label, "failed", Some(error), t, cx)
                    }
                    LoadState::Ready(inventory) => {
                        self.render_coverage_values(label, &inventory.coverage, t, cx)
                    }
                }
            }
            CoverageDataset::Structure => match &self.smart_review.structure {
                LoadState::Idle => self.render_coverage_state("Structure", "idle", None, t, cx),
                LoadState::Loading => {
                    self.render_coverage_state("Structure", "loading", None, t, cx)
                }
                LoadState::Failed(error) => {
                    self.render_coverage_state("Structure", "failed", Some(error), t, cx)
                }
                LoadState::Ready(structure) => {
                    self.render_coverage_values("Structure", structure.coverage(), t, cx)
                }
            },
        }
    }

    fn render_coverage_state(
        &self,
        dataset: &str,
        state: &str,
        detail: Option<&str>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut values = vec![
            ("Coverage".into(), dataset.to_string()),
            ("State".into(), state.to_string()),
        ];
        if let Some(detail) = detail {
            values.push(("Detail".into(), detail.to_string()));
        }
        self.render_summary(&values, t, cx)
    }

    fn render_coverage_values(
        &self,
        dataset: &str,
        coverage: &ReviewCoverage,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut values = vec![
            ("Coverage".into(), dataset.to_string()),
            ("Total".into(), coverage.total_items().to_string()),
            ("Analyzed".into(), coverage.analyzed_items().to_string()),
            ("Pending".into(), coverage.pending_items().to_string()),
            ("Skipped".into(), coverage.skipped_items().to_string()),
            (
                "Unsupported".into(),
                coverage.unsupported_items().to_string(),
            ),
            ("Failed".into(), coverage.failed_items().to_string()),
        ];
        if let Some(truncation) = coverage.truncation() {
            values.push((
                "Truncation".into(),
                format!(
                    "{:?} · {} / {} · {}",
                    truncation.reason,
                    truncation
                        .observed
                        .map_or_else(|| "-".into(), |n| n.to_string()),
                    truncation
                        .limit
                        .map_or_else(|| "-".into(), |n| n.to_string()),
                    truncation.detail.as_deref().unwrap_or("")
                ),
            ));
        }
        self.render_summary(&values, t, cx)
    }

    pub(super) fn render_review_sidebar(
        &mut self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = self
            .ensure_review_cache(ReviewLens::Inventory)
            .map(|cache| cache.sidebar_rows)
            .unwrap_or_default();
        let count = rows.len();
        let colors = *t;
        let view = cx.entity();
        div()
            .w(px(280.0))
            .min_h_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .h(px(34.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .text_size(ui_text_ms(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .child(format!("Files  {count}")),
            )
            .child(
                uniform_list("review-sidebar-rows", count, move |range, _window, cx| {
                    let rows = rows.clone();
                    view.update(cx, |this, cx| {
                        range
                            .map(|index| {
                                this.render_review_row(
                                    rows[index].clone(),
                                    SharedString::from(format!("review-sidebar-row-{index}")),
                                    &colors,
                                    cx,
                                )
                            })
                            .collect()
                    })
                })
                .track_scroll(&self.review_sidebar_scroll_handle)
                .flex_1(),
            )
            .into_any_element()
    }

    pub(super) fn render_review_lens_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let lens = self.smart_review.lens;
        let state = match lens {
            ReviewLens::Inventory => match &self.smart_review.inventory {
                LoadState::Idle => Some(("Inventory is not loaded", None)),
                LoadState::Loading => Some(("Loading inventory…", None)),
                LoadState::Failed(error) => Some(("Inventory failed", Some(error.clone()))),
                LoadState::Ready(_) => None,
            },
            ReviewLens::Structure | ReviewLens::CallDiff => match &self.smart_review.structure {
                LoadState::Idle => Some(("Structure is not loaded", None)),
                LoadState::Loading => Some(("Loading structure…", None)),
                LoadState::Failed(error) => Some(("Structure failed", Some(error.clone()))),
                LoadState::Ready(_) => None,
            },
            ReviewLens::Diff => return div().into_any_element(),
        };
        if let Some((title, detail)) = state {
            return self.render_review_state(title, detail.as_deref(), &t, cx);
        }

        let cache = self.ensure_review_cache(lens);
        match cache {
            Some(cache) if lens == ReviewLens::Inventory => self.render_inventory(cache, &t, cx),
            Some(cache) => self.render_review_model(cache, &t, cx),
            None => self.render_review_state("Dataset is not ready", None, &t, cx),
        }
    }

    fn ensure_review_cache(&mut self, lens: ReviewLens) -> Option<ReviewRenderCache> {
        let content_revision = self.smart_review.content_revision();
        let selection_revision = match lens {
            ReviewLens::Inventory | ReviewLens::Diff => 0,
            ReviewLens::Structure | ReviewLens::CallDiff => self.smart_review.selection_revision(),
        };
        self.review_render_cache.retain(|cache| {
            cache.content_revision == content_revision
                && (cache.selection_revision == 0
                    || cache.selection_revision == self.smart_review.selection_revision())
        });
        if let Some(cache) = self
            .review_render_cache
            .iter()
            .find(|cache| cache.matches(content_revision, selection_revision, lens))
        {
            return Some(cache.clone());
        }
        let key = self.smart_review.selected_file.as_ref();
        let model = match lens {
            ReviewLens::Inventory => inventory_model(
                self.smart_review.inventory.ready()?,
                self.smart_review.role_filter,
            ),
            ReviewLens::Structure => structure_model(self.smart_review.structure.ready()?, key),
            ReviewLens::CallDiff => call_diff_model(self.smart_review.structure.ready()?, key),
            ReviewLens::Diff => return None,
        };
        let inventory_cache = self
            .review_render_cache
            .iter()
            .find(|cache| cache.lens == ReviewLens::Inventory);
        let (sidebar_rows, roles) = if let Some(cache) = inventory_cache {
            (cache.sidebar_rows.clone(), cache.roles.clone())
        } else {
            let inventory = self.smart_review.inventory.ready();
            let sidebar_rows = if lens == ReviewLens::Inventory {
                model
                    .sections
                    .iter()
                    .find(|section| section.title == "Files")
                    .map_or_else(Vec::new, |section| section.rows.clone())
            } else {
                inventory
                    .map(|inventory| {
                        inventory_model(inventory, self.smart_review.role_filter)
                            .sections
                            .into_iter()
                            .find(|section| section.title == "Files")
                            .map_or_else(Vec::new, |section| section.rows)
                    })
                    .unwrap_or_default()
            };
            let mut roles = inventory
                .into_iter()
                .flat_map(|inventory| inventory.files.iter())
                .map(|file| file.classification.role())
                .collect::<Vec<_>>();
            roles.sort_by_key(|role| role_label(*role));
            roles.dedup();
            (Arc::new(sidebar_rows), Arc::new(roles))
        };
        let flat_rows = flatten_sections(&model.sections);
        let cache = ReviewRenderCache {
            content_revision,
            selection_revision,
            lens,
            model: Arc::new(model),
            flat_rows: Arc::new(flat_rows),
            sidebar_rows,
            roles,
        };
        self.review_render_cache.push(cache.clone());
        Some(cache)
    }

    fn render_inventory(
        &self,
        cache: ReviewRenderCache,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter = self.smart_review.role_filter;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.render_summary(&cache.model.summary, t, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(16.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(self.render_role_filter("All", None, filter, t, cx))
                    .children(cache.roles.iter().copied().map(|role| {
                        self.render_role_filter(role_label(role), Some(role), filter, t, cx)
                    })),
            )
            .child(self.render_flat_rows(cache.flat_rows, t, cx))
            .into_any_element()
    }

    fn render_role_filter(
        &self,
        label: &'static str,
        role: Option<FileRole>,
        active: Option<FileRole>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = role == active;
        div()
            .id(SharedString::from(format!("review-role-{label}")))
            .px(px(7.0))
            .py(px(3.0))
            .border_1()
            .border_color(rgb(if selected { t.border_active } else { t.border }))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(if selected {
                t.text_primary
            } else {
                t.text_muted
            }))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.smart_review.set_role_filter(role);
                cx.notify();
            }))
            .child(label)
            .into_any_element()
    }

    fn render_summary(
        &self,
        summary: &[(String, String)],
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .border_b_1()
            .border_color(rgb(t.border))
            .children(summary.iter().map(|(label, value)| {
                div()
                    .flex_1()
                    .min_w(px(100.0))
                    .px(px(16.0))
                    .py(px(9.0))
                    .border_r_1()
                    .border_color(rgb(t.border))
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .mt(px(2.0))
                            .text_size(ui_text_sm(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .child(value.clone()),
                    )
            }))
            .into_any_element()
    }

    fn render_review_row(
        &self,
        row: ReviewRow,
        row_id: SharedString,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let file_target = match &row.target {
            Some(ReviewRowTarget::File(key)) => Some(key.clone()),
            Some(ReviewRowTarget::Evidence(_)) | None => None,
        };
        let selected = file_target
            .as_ref()
            .is_some_and(|key| self.smart_review.selected_file.as_ref() == Some(key));
        let tone = match row.tone {
            RowTone::Neutral => t.text_secondary,
            RowTone::Added => t.diff_added_fg,
            RowTone::Removed => t.diff_removed_fg,
            RowTone::Warning => t.term_yellow,
        };
        div()
            .id(row_id)
            .h(px(44.0))
            .border_l_2()
            .border_color(rgb(if selected {
                t.border_active
            } else {
                t.bg_primary
            }))
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(16.0))
            .py(px(7.0))
            .border_t_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_primary))
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(row.primary),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(row.secondary),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_family("monospace")
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(tone))
                    .child(row.trailing),
            )
            .when_some(file_target, |d, key| {
                d.cursor_pointer()
                    .hover(|style| style.bg(rgb(t.bg_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_smart_file(key.clone(), cx);
                    }))
            })
            .into_any_element()
    }

    fn render_flat_rows(
        &self,
        rows: Arc<Vec<FlatReviewRow>>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = rows.len();
        let colors = *t;
        let view = cx.entity();
        uniform_list("review-lens-rows", count, move |range, _window, cx| {
            let rows = rows.clone();
            view.update(cx, |this, cx| {
                range
                    .map(|index| match &rows[index] {
                        FlatReviewRow::Header { title, count } => div()
                            .h(px(44.0))
                            .px(px(16.0))
                            .flex()
                            .items_center()
                            .bg(rgb(colors.bg_secondary))
                            .border_b_1()
                            .border_color(rgb(colors.border))
                            .text_size(ui_text_ms(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(colors.text_secondary))
                            .child(format!("{title}  {count}"))
                            .into_any_element(),
                        FlatReviewRow::Empty => div()
                            .h(px(44.0))
                            .px(px(16.0))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(rgb(colors.border))
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(colors.text_muted))
                            .child("No entries")
                            .into_any_element(),
                        FlatReviewRow::Entry(row) => this.render_review_row(
                            row.clone(),
                            SharedString::from(format!("review-lens-row-{index}")),
                            &colors,
                            cx,
                        ),
                    })
                    .collect()
            })
        })
        .track_scroll(&self.review_scroll_handle)
        .flex_1()
        .into_any_element()
    }

    fn render_review_state(
        &self,
        title: &str,
        detail: Option<&str>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(title.to_string())
                    .when_some(detail, |d, detail| {
                        d.child(
                            div()
                                .mt(px(4.0))
                                .text_size(ui_text_ms(cx))
                                .child(detail.to_string()),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_review_model(
        &self,
        cache: ReviewRenderCache,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.render_summary(&cache.model.summary, t, cx))
            .child(self.render_flat_rows(cache.flat_rows, t, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CallDiffChange, CoverageDataset, DiffPhase, FileRole, FlatReviewRow, ReviewFileFact,
        ReviewFileKey, ReviewInventory, ReviewLens, ReviewRenderCache, ReviewRenderModel,
        ReviewRowTarget, ReviewStructure, RowTone, SmartDiffViewState, SourcePhase,
        call_change_row, classify_smart_diff_state, coverage_dataset_for_lens, exact_display_ready,
        file_path, inventory_model, qualified_symbol_name, role_label, structure_model,
    };
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn comparison_json() -> Value {
        let base = "1".repeat(40);
        let merge_base = "2".repeat(40);
        let head = "3".repeat(40);
        json!({
            "requested": { "branch_compare": { "base": "main", "head": "feature" } },
            "requested_base_oid": base,
            "requested_head_oid": head,
            "strategy": "merge_base_to_head",
            "base": { "kind": "commit", "oid": merge_base },
            "head": { "kind": "commit", "oid": head },
            "merge_base_oid": merge_base,
            "identity": format!("branch:merge-base:{base}:{head}:{merge_base}")
        })
    }

    fn coverage_json(total: u64, analyzed: u64, unsupported: u64) -> Value {
        json!({
            "total_items": total,
            "analyzed_items": analyzed,
            "pending_items": 0,
            "skipped_items": 0,
            "unsupported_items": unsupported,
            "failed_items": 0
        })
    }

    #[test]
    fn file_role_labels_are_stable() {
        assert_eq!(role_label(FileRole::Implementation), "Implementation");
        assert_eq!(role_label(FileRole::Unclassified), "Unclassified");
    }

    #[test]
    fn qualified_symbol_name_keeps_parents_and_own_name() {
        assert_eq!(
            qualified_symbol_name(&["module".into(), "Type".into()], "method"),
            "module::Type::method"
        );
        assert_eq!(
            qualified_symbol_name(&["module".into()], "module"),
            "module"
        );
    }

    #[test]
    fn smart_diff_local_states_cover_dataset_source_and_display() {
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Idle, false, SourcePhase::Idle, false),
            SmartDiffViewState::Idle
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Loading, false, SourcePhase::Idle, false),
            SmartDiffViewState::Loading
        );
        assert_eq!(
            classify_smart_diff_state(
                DiffPhase::Failed("diff".into()),
                false,
                SourcePhase::Idle,
                false
            ),
            SmartDiffViewState::Failed("diff".into())
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Empty, false, SourcePhase::Idle, false),
            SmartDiffViewState::Empty
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Files, false, SourcePhase::Idle, false),
            SmartDiffViewState::NoSelection
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Files, true, SourcePhase::Loading, false),
            SmartDiffViewState::SourceLoading
        );
        assert_eq!(
            classify_smart_diff_state(
                DiffPhase::Files,
                true,
                SourcePhase::Failed("source".into()),
                false
            ),
            SmartDiffViewState::SourceFailed("source".into())
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Files, true, SourcePhase::Ready, false),
            SmartDiffViewState::DisplayLoading
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Files, true, SourcePhase::Ready, true),
            SmartDiffViewState::Ready
        );
    }

    #[test]
    fn render_cache_scopes_selection_to_selected_fact_lenses() {
        let cache = ReviewRenderCache {
            content_revision: 7,
            selection_revision: 11,
            lens: ReviewLens::Structure,
            model: Arc::new(ReviewRenderModel {
                summary: Vec::new(),
                sections: Vec::new(),
            }),
            flat_rows: Arc::new(Vec::<FlatReviewRow>::new()),
            sidebar_rows: Arc::new(Vec::new()),
            roles: Arc::new(Vec::new()),
        };
        assert!(cache.matches(7, 11, ReviewLens::Structure));
        assert!(!cache.matches(7, 12, ReviewLens::Structure));
        assert!(!cache.matches(8, 11, ReviewLens::Structure));

        let inventory_cache = ReviewRenderCache {
            content_revision: 7,
            selection_revision: 0,
            lens: ReviewLens::Inventory,
            ..cache
        };
        assert!(inventory_cache.matches(7, 0, ReviewLens::Inventory));
        assert!(!inventory_cache.matches(7, 11, ReviewLens::Inventory));
    }

    #[test]
    fn coverage_dataset_is_explicit_for_every_lens() {
        assert_eq!(
            coverage_dataset_for_lens(ReviewLens::Inventory),
            CoverageDataset::Inventory
        );
        assert_eq!(
            coverage_dataset_for_lens(ReviewLens::Diff),
            CoverageDataset::Inventory
        );
        assert_eq!(
            coverage_dataset_for_lens(ReviewLens::Structure),
            CoverageDataset::Structure
        );
        assert_eq!(
            coverage_dataset_for_lens(ReviewLens::CallDiff),
            CoverageDataset::Structure
        );
    }

    #[test]
    fn exact_diff_ready_rejects_a_stale_file_key() {
        let canonical = ReviewFileKey {
            old_path: Some("a.rs".into()),
            new_path: Some("a.rs".into()),
        };
        let stale = ReviewFileKey {
            old_path: Some("b.rs".into()),
            new_path: Some("b.rs".into()),
        };
        assert!(!exact_display_ready(
            Some(&canonical),
            Some(&stale),
            true,
            true,
            true
        ));
        assert!(exact_display_ready(
            Some(&canonical),
            Some(&canonical),
            true,
            true,
            true
        ));
    }

    #[test]
    fn call_row_keeps_enclosing_caller_and_evidence_target() {
        let change: CallDiffChange = serde_json::from_value(json!({
            "kind": "added",
            "old": null,
            "new": {
                "provenance": { "language": "rust", "parser": "tree-sitter-rust" },
                "callee_text": "work",
                "argument_text": "value",
                "argument_range": {
                    "start_byte": 5, "end_byte": 10, "start_line": 3, "end_line": 3
                },
                "call_site_range": {
                    "start_byte": 0, "end_byte": 11, "start_line": 3, "end_line": 3
                },
                "enclosing_symbol": {
                    "qualified_path": ["module", "Type"], "kind": "method", "name": "caller"
                },
                "control_context": []
            },
            "arguments_changed": false,
            "control_context_changed": false,
            "pairing": null,
            "navigation": { "path": "src/lib.rs", "side": "head", "line": 3 }
        }))
        .unwrap();
        let row = call_change_row(&change);
        assert!(row.secondary.contains("module::Type::caller"));
        assert!(matches!(row.target, Some(ReviewRowTarget::Evidence(_))));
        assert_eq!(row.trailing, "src/lib.rs:3");
    }

    #[test]
    fn renamed_file_uses_exact_path_pair() {
        let file: ReviewFileFact = serde_json::from_value(serde_json::json!({
            "old_path": "old.rs",
            "new_path": "new.rs",
            "status": "renamed",
            "similarity": 100,
            "old_mode": "100644",
            "new_mode": "100644",
            "lines_added": 1,
            "lines_deleted": 1,
            "binary": false,
            "classification": { "role": "implementation", "rule_id": "extension.rs" },
            "provenance": { "source": "git" }
        }))
        .unwrap();
        assert_eq!(file_path(&file), "old.rs → new.rs");
    }

    #[test]
    fn inventory_model_filters_roles_without_reordering_the_ledger() {
        let inventory: ReviewInventory = serde_json::from_value(json!({
            "comparison": comparison_json(),
            "totals": {
                "commits": 2, "files": 2, "files_added": 1, "files_deleted": 0,
                "files_modified": 1, "files_renamed": 0, "files_copied": 0,
                "files_type_changed": 0, "files_mode_changed": 0,
                "submodule_changes": 0, "binary_files": 0,
                "lines_added": 11, "lines_deleted": 3,
                "provenance": { "source": "git" }
            },
            "commits": [
                { "oid": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "parent_oids": [],
                  "subject": "first", "author_name": "Ada", "timestamp": 1,
                  "provenance": { "source": "git" } },
                { "oid": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "parent_oids": [],
                  "subject": "second", "author_name": "Bob", "timestamp": 2,
                  "provenance": { "source": "git" } }
            ],
            "files": [
                { "new_path": "src/lib.rs", "status": "added", "lines_added": 10,
                  "lines_deleted": 0, "binary": false,
                  "classification": { "role": "implementation", "rule_id": "extension.rs" },
                  "provenance": { "source": "git" } },
                { "old_path": "tests/lib.rs", "new_path": "tests/lib.rs",
                  "status": "modified", "lines_added": 1, "lines_deleted": 3,
                  "binary": false,
                  "classification": { "role": "test", "rule_id": "path.tests" },
                  "provenance": { "source": "git" } }
            ],
            "coverage": coverage_json(2, 2, 0)
        }))
        .unwrap();

        let model = inventory_model(&inventory, Some(FileRole::Test));
        let commits = model
            .sections
            .iter()
            .find(|section| section.title == "Commit ledger")
            .unwrap();
        assert_eq!(commits.rows[0].primary, "first");
        assert_eq!(commits.rows[1].primary, "second");
        let files = model
            .sections
            .iter()
            .find(|section| section.title == "Files")
            .unwrap();
        assert_eq!(files.rows.len(), 1);
        assert_eq!(files.rows[0].primary, "tests/lib.rs");
        assert_eq!(files.rows[0].tone, RowTone::Neutral);
        assert!(matches!(
            files.rows[0].target,
            Some(ReviewRowTarget::File(_))
        ));
        assert!(files.rows[0].secondary.contains("path.tests"));
        assert!(files.rows[0].secondary.contains("Git"));
    }

    #[test]
    fn structure_model_uses_exact_pair_and_keeps_aggregate_omissions() {
        let structure: ReviewStructure = serde_json::from_value(json!({
            "comparison": comparison_json(),
            "files": [{
                "old_path": "old.rs", "new_path": "new.rs", "language": null,
                "old_provenance": null, "new_provenance": null,
                "status": "unsupported", "old_outline": [], "new_outline": [],
                "symbol_changes": [], "hotspots": [], "call_diff": [],
                "changed_hunks": [], "errors": [], "truncation": null
            }],
            "omissions": [
                {
                    "count": 2, "language": null, "reason": "binary",
                    "status": "unsupported", "truncation": null
                },
                {
                    "count": 1, "language": null, "reason": "source_byte_limit",
                    "status": "pending",
                    "truncation": {
                        "reason": "byte_limit", "limit": 100, "observed": 101,
                        "detail": "capture cap"
                    }
                }
            ],
            "coverage": {
                "total_items": 4, "analyzed_items": 0, "pending_items": 1,
                "skipped_items": 0, "unsupported_items": 3, "failed_items": 0,
                "truncation": {
                    "reason": "byte_limit", "limit": 100, "observed": 101,
                    "detail": "capture cap"
                }
            },
            "language_coverage": [], "errors": []
        }))
        .unwrap();
        let key = ReviewFileKey {
            old_path: Some("old.rs".into()),
            new_path: Some("new.rs".into()),
        };
        let model = structure_model(&structure, Some(&key));
        assert!(
            model
                .summary
                .contains(&("Selected".into(), "unsupported".into()))
        );
        let omissions = model
            .sections
            .iter()
            .find(|section| section.title == "Aggregate omissions")
            .unwrap();
        assert_eq!(omissions.rows.len(), 2);
        assert!(
            omissions
                .rows
                .iter()
                .all(|row| row.secondary.contains("not language-specific"))
        );
        let limited = omissions
            .rows
            .iter()
            .find(|row| row.primary == "SourceByteLimit")
            .unwrap();
        assert!(limited.secondary.contains("pending"));
        assert!(limited.secondary.contains("ByteLimit"));
        assert!(limited.secondary.contains("101 / 100"));
        assert!(limited.secondary.contains("capture cap"));

        let wrong_key = ReviewFileKey {
            old_path: Some("old.rs".into()),
            new_path: Some("other.rs".into()),
        };
        let wrong = structure_model(&structure, Some(&wrong_key));
        assert!(!wrong.summary.iter().any(|(label, _)| label == "Selected"));
    }
}
