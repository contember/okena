//! Builds the review model from the loaded datasets — spec §5 / §6.
//!
//! Everything here is deterministic and filter-independent: the same inventory
//! and structure always produce the same order, and no filter is ever consulted.

use super::super::review::ReviewFileKey;
use super::labels::reasons as words;
use super::labels::{control_context_word, language_from_path, language_label, symbol_glyph};
use super::model::{
    AlsoFact, AnalysisStatus, AttentionItem, AttentionTarget, CallRow, CommitRow, CommitsFact,
    CoverageSummary, DirNode, DirRef, Facts, FileAnalysis, FileEntry, KindGlyph, MovesFact,
    OmissionRow, PublicApiFact, Reason, ReasonKind, ReviewModel, SymbolEntry, SymbolMetrics,
    TestsFact, Tier, VolumeRow, is_under,
};
use super::state::{ALL_ROLES, MECHANICAL_RESIDUAL_LINES, is_likely_mechanical};
use okena_core::review::{
    ComparisonSide, FileRole, ResolvedComparison, ReviewCoverage, ReviewFileFact, ReviewFileStatus,
    ReviewInventory,
};
use okena_git::DiffMode;
use okena_review::{
    CallChangeKind, CallDiffChange, FileAnalysisStatus, OmittedFileGroup, OmittedFileReason,
    ReviewStructure, StructuralHotspot, StructuralMetric, StructuredFile, SymbolChange,
    SymbolChangeKind,
};
use okena_syntax::{ControlContext, SymbolKey, SymbolVisibility};
use std::collections::BTreeMap;

/// Comparisons at or below either bound skip the Overview — spec §12.
const SMALL_CHANGE_FILES: usize = 10;
const SMALL_CHANGE_LINES: u64 = 500;

/// Complexity worth a chip on a symbol that changed anyway — spec §6.
const COMPLEX_DEPTH: u32 = 5;
const COMPLEX_PARAMS: u32 = 6;

/// Churn share that counts as "one of the largest changes" — spec §6 tier 4.
const CHURN_DECILE: usize = 10;

/// Extensions named in the unsupported-language omission row.
const OMISSION_EXTENSIONS: usize = 4;

/// How far the structure request got, independent of the inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StructureLoad {
    NotStarted,
    Loading,
    Failed(String),
    Ready,
}

pub(crate) struct ModelInputs<'a> {
    pub inventory: Option<&'a ReviewInventory>,
    pub inventory_error: Option<&'a str>,
    pub structure: Option<&'a ReviewStructure>,
    pub structure_state: StructureLoad,
    pub diff_mode: &'a DiffMode,
}

/// Filter-independent. Rebuilt only when inventory / structure land or fail. Files
/// in inventory order, keyed by `ReviewFileKey` — NOT the diff pane's `file_stats`
/// order.
pub(crate) fn build_review_model(inputs: ModelInputs<'_>) -> ReviewModel {
    let Some(inventory) = inputs.inventory else {
        return empty_model(match inputs.inventory_error {
            Some(error) => AnalysisStatus::Unavailable {
                message: error.to_string(),
            },
            None => AnalysisStatus::LoadingInventory,
        });
    };

    let structure = inputs.structure;
    let mut files: Vec<FileEntry> = inventory
        .files
        .iter()
        .map(|fact| file_entry(fact, structure))
        .collect();
    apply_large_churn(&mut files);

    let total_changed_lines = files.iter().fold(0u64, |total, entry| {
        total.saturating_add(entry.changed_lines())
    });
    let root = directory_tree(&files);
    let volume = volume_rows(&files, total_changed_lines);
    let attention = attention_items(&files, &root);
    // A single commit target has no commit ledger to show — spec §12.
    let commits = match inputs.diff_mode {
        DiffMode::Commit(_) => Vec::new(),
        _ => commit_rows(inventory),
    };
    let status = analysis_status(inventory, structure, &inputs.structure_state);
    let coverage = coverage_summary(inventory, structure, &files);
    let facts = Facts {
        public_api: public_api_fact(&files, structure, &coverage),
        tests: tests_fact(&root),
        moves: moves_fact(&files),
        commits: commits_fact(&commits),
        also: also_fact(&files),
    };
    let omissions = omission_rows(structure, &files);
    let small_change =
        files.len() <= SMALL_CHANGE_FILES || total_changed_lines <= SMALL_CHANGE_LINES;

    ReviewModel {
        files,
        root,
        volume,
        total_changed_lines,
        facts,
        attention,
        status,
        omissions,
        commits,
        coverage,
        small_change,
    }
}

fn empty_model(status: AnalysisStatus) -> ReviewModel {
    ReviewModel {
        files: Vec::new(),
        root: DirNode::default(),
        volume: Vec::new(),
        total_changed_lines: 0,
        facts: Facts::default(),
        attention: Vec::new(),
        status,
        omissions: Vec::new(),
        commits: Vec::new(),
        coverage: CoverageSummary::default(),
        small_change: true,
    }
}

/// A reason plus the tier it argues for. Annotations (`Complex`, `NotAnalyzed`)
/// carry no tier — spec §6 keeps them off the ranking.
struct Scored {
    reason: Reason,
    tier: Option<Tier>,
}

fn scored(kind: ReasonKind, label: impl Into<String>, tier: Tier) -> Scored {
    Scored {
        reason: Reason {
            kind,
            label: label.into(),
        },
        tier: Some(tier),
    }
}

fn annotation(kind: ReasonKind, label: impl Into<String>) -> Scored {
    Scored {
        reason: Reason {
            kind,
            label: label.into(),
        },
        tier: None,
    }
}

fn tier_of(scored: &[Scored]) -> Tier {
    scored
        .iter()
        .filter_map(|entry| entry.tier)
        .min()
        .unwrap_or_default()
}

fn chips(scored: Vec<Scored>) -> Vec<Reason> {
    scored.into_iter().map(|entry| entry.reason).collect()
}

/// Unclassified files are reviewed like implementation files — spec §6.
fn is_implementation_like(role: FileRole) -> bool {
    matches!(role, FileRole::Implementation | FileRole::Unclassified)
}

fn is_function(glyph: KindGlyph) -> bool {
    matches!(glyph, KindGlyph::Function | KindGlyph::Method)
}

fn is_type(glyph: KindGlyph) -> bool {
    matches!(glyph, KindGlyph::Class | KindGlyph::Type)
}

// -- files -------------------------------------------------------------------

fn file_entry(fact: &ReviewFileFact, structure: Option<&ReviewStructure>) -> FileEntry {
    let key = ReviewFileKey::from_inventory(fact);
    let role = fact.classification.role();
    let structure_index = structure.and_then(|structure| {
        structure.files().iter().position(|file| {
            file.old_path() == fact.old_path.as_deref()
                && file.new_path() == fact.new_path.as_deref()
        })
    });
    let structured = structure
        .zip(structure_index)
        .and_then(|(structure, index)| structure.files().get(index));
    let path = fact
        .new_path
        .as_deref()
        .or(fact.old_path.as_deref())
        .unwrap_or_default();
    let analysis = file_analysis(structured, path);
    let symbols = structured.map(symbol_entries).unwrap_or_default();
    let reasons = file_reasons(fact, role, &analysis, structure.is_some(), path);

    FileEntry {
        display_path: key.display(),
        key,
        old_path: fact.old_path.clone(),
        new_path: fact.new_path.clone(),
        status: fact.status,
        role,
        rule_id: fact.classification.rule_id().as_str().to_string(),
        similarity: fact.similarity,
        lines_added: fact.lines_added.unwrap_or(0),
        lines_deleted: fact.lines_deleted.unwrap_or(0),
        binary: fact.binary,
        analysis,
        tier: tier_of(&reasons),
        reasons: chips(reasons),
        is_test: role == FileRole::Test,
        symbols,
        structure_index,
    }
}

fn file_analysis(structured: Option<&StructuredFile>, path: &str) -> FileAnalysis {
    let Some(file) = structured else {
        return FileAnalysis::NotInStructure;
    };
    let language = file
        .language()
        .map(|language| language_label(&language).to_string())
        .or_else(|| language_from_path(path).map(str::to_string))
        .unwrap_or_default();
    match file.status() {
        FileAnalysisStatus::Parsed => FileAnalysis::Parsed { language },
        FileAnalysisStatus::Partial => FileAnalysis::Partial { language },
        FileAnalysisStatus::Pending => FileAnalysis::Pending,
        FileAnalysisStatus::Unsupported => FileAnalysis::Unsupported,
        FileAnalysisStatus::Failed => FileAnalysis::Failed,
        FileAnalysisStatus::Skipped => FileAnalysis::Skipped,
    }
}

fn file_reasons(
    fact: &ReviewFileFact,
    role: FileRole,
    analysis: &FileAnalysis,
    structure_present: bool,
    path: &str,
) -> Vec<Scored> {
    let mut out = Vec::new();
    let implementation = is_implementation_like(role);
    let residual = fact
        .lines_added
        .unwrap_or(0)
        .saturating_add(fact.lines_deleted.unwrap_or(0));

    if implementation && fact.status == ReviewFileStatus::Deleted {
        out.push(scored(
            ReasonKind::DeletedImpl,
            words::DELETED_IMPLEMENTATION_FILE,
            Tier::Contract,
        ));
    }
    if role == FileRole::Configuration {
        out.push(scored(
            ReasonKind::CiConfig,
            words::CI_CONFIG,
            Tier::GitFacts,
        ));
    }
    if role == FileRole::Lockfile {
        out.push(scored(
            ReasonKind::Lockfile,
            words::LOCKFILE,
            Tier::GitFacts,
        ));
    }
    if fact.submodule.is_some() || fact.status == ReviewFileStatus::SubmoduleChanged {
        out.push(scored(
            ReasonKind::Submodule,
            words::SUBMODULE,
            Tier::GitFacts,
        ));
    }
    if fact.binary && implementation {
        out.push(scored(ReasonKind::Binary, words::BINARY, Tier::GitFacts));
    }
    if implementation && !fact.binary && fact.status == ReviewFileStatus::Added {
        out.push(scored(
            ReasonKind::New,
            words::NEW_IMPLEMENTATION_FILE,
            Tier::GitFacts,
        ));
    }
    if fact.status == ReviewFileStatus::Renamed {
        // Renames are judged by residual lines, not by similarity — spec §6.
        let with_edits = residual > MECHANICAL_RESIDUAL_LINES;
        if let Some(similarity) = fact.similarity {
            out.push(Scored {
                reason: Reason {
                    kind: ReasonKind::Moved,
                    label: words::moved_label(similarity),
                },
                tier: with_edits.then_some(Tier::GitFacts),
            });
        }
        if with_edits {
            out.push(scored(
                ReasonKind::Moved,
                words::residual_label(residual),
                Tier::GitFacts,
            ));
        }
    }
    if structure_present && !analysis.is_analyzed() {
        out.push(annotation(
            ReasonKind::NotAnalyzed,
            words::not_analyzed_label(analysis.language().or_else(|| language_from_path(path))),
        ));
    }
    out
}

/// The biggest implementation changes nothing else already explains — spec §6.
fn apply_large_churn(files: &mut [FileEntry]) {
    let Some(threshold) = churn_threshold(files) else {
        return;
    };
    for entry in files.iter_mut() {
        let unexplained = entry.tier == Tier::Rest
            && entry
                .reasons
                .iter()
                .all(|reason| reason.kind == ReasonKind::NotAnalyzed);
        if is_implementation_like(entry.role) && unexplained && entry.changed_lines() >= threshold {
            entry.reasons.push(Reason {
                kind: ReasonKind::LargeChurn,
                label: words::LARGE_CHURN.to_string(),
            });
            entry.tier = Tier::GitFacts;
        }
    }
}

/// Smallest churn still inside the top decile of implementation files.
fn churn_threshold(files: &[FileEntry]) -> Option<u64> {
    let mut churn: Vec<u64> = files
        .iter()
        .filter(|entry| is_implementation_like(entry.role) && entry.changed_lines() > 0)
        .map(FileEntry::changed_lines)
        .collect();
    if churn.len() < 2 {
        return None;
    }
    churn.sort_unstable_by(|left, right| right.cmp(left));
    let decile = churn.len().div_ceil(CHURN_DECILE).max(1);
    churn.get(decile.saturating_sub(1)).copied()
}

// -- symbols -----------------------------------------------------------------

fn symbol_entries(file: &StructuredFile) -> Vec<SymbolEntry> {
    file.symbol_changes()
        .iter()
        .enumerate()
        .filter_map(|(change_index, change)| symbol_entry(file, change_index, change))
        .collect()
}

fn symbol_entry(
    file: &StructuredFile,
    change_index: usize,
    change: &SymbolChange,
) -> Option<SymbolEntry> {
    let fact = change.new_fact().or_else(|| change.old())?;
    let key = fact.key();
    let visibility = fact.visibility();
    let public = [change.old(), change.new_fact()]
        .into_iter()
        .flatten()
        .any(|fact| {
            matches!(
                fact.visibility(),
                SymbolVisibility::Public | SymbolVisibility::Exported
            )
        });
    let glyph = symbol_glyph(&key.kind());
    let calls = call_rows(file.call_diff(), key);
    let metrics = symbol_metrics(file.hotspots(), key);
    let reasons = symbol_reasons(SymbolFacts {
        change,
        glyph,
        public,
        visibility,
        calls: &calls,
        contexts: &call_contexts(file.call_diff(), key),
        metrics,
        edited_body: has_changed_lines(file.hotspots(), key),
    });
    let (old_hunks, new_hunks) = hunk_ranges(change);

    Some(SymbolEntry {
        change_index,
        name: key.name().to_string(),
        qualified: key.qualified_name(),
        glyph,
        change: change.kind(),
        public,
        signature: change.signature_change().map(|signature| {
            (
                signature.old_signature().to_string(),
                signature.new_signature().to_string(),
            )
        }),
        body_changed: change.body_changed(),
        lines_added: change.changed_new_lines(),
        lines_deleted: change.changed_old_lines(),
        calls,
        tier: tier_of(&reasons),
        reasons: chips(reasons),
        navigation: change.navigation().clone(),
        old_hunks,
        new_hunks,
        metrics,
    })
}

struct SymbolFacts<'a> {
    change: &'a SymbolChange,
    glyph: KindGlyph,
    public: bool,
    visibility: SymbolVisibility,
    calls: &'a [CallRow],
    contexts: &'a [&'a ControlContext],
    metrics: SymbolMetrics,
    /// The analysis measured how much of this symbol's body changed.
    edited_body: bool,
}

fn symbol_reasons(facts: SymbolFacts<'_>) -> Vec<Scored> {
    let mut out = Vec::new();
    let change = facts.change;
    let mut body_named = false;

    // Tier 1 — the contract other code depends on.
    match change.kind() {
        SymbolChangeKind::Removed if facts.public => {
            out.push(scored(
                ReasonKind::PublicRemoved,
                words::PUBLIC_REMOVED,
                Tier::Contract,
            ));
        }
        SymbolChangeKind::Removed => out.push(annotation(ReasonKind::Removed, words::REMOVED)),
        SymbolChangeKind::Added | SymbolChangeKind::Modified => {}
    }
    if change.signature_change().is_some() && facts.public {
        let (kind, label) = if facts.visibility == SymbolVisibility::Exported {
            (ReasonKind::ExportedSignature, words::EXPORTED_SIGNATURE)
        } else {
            (ReasonKind::PublicSignature, words::PUBLIC_SIGNATURE)
        };
        out.push(scored(kind, label, Tier::Contract));
        if change.body_changed() {
            // Signature *and* body outranks signature only inside the tier.
            out.push(scored(ReasonKind::Body, words::BODY, Tier::Contract));
            body_named = true;
        }
    }

    // Tier 2 — behaviour, read through the calls the function makes.
    if is_function(facts.glyph) && !facts.calls.is_empty() {
        let context = words::most_severe_context(facts.contexts.iter().copied());
        out.push(scored(
            ReasonKind::Calls,
            words::calls_label(facts.calls.len(), context.as_deref()),
            Tier::Behaviour,
        ));
    }

    // Tier 3 — volume, measured separately for edits and for new code.
    match change.kind() {
        SymbolChangeKind::Modified
            if is_function(facts.glyph) && facts.edited_body && !body_named =>
        {
            out.push(scored(ReasonKind::Body, words::BODY, Tier::Volume));
        }
        SymbolChangeKind::Added => {
            if let Some(lines) = facts.metrics.lines.filter(|_| is_function(facts.glyph)) {
                out.push(scored(
                    ReasonKind::New,
                    words::lines_label(lines),
                    Tier::Volume,
                ));
            }
            if let Some(members) = facts.metrics.members.filter(|_| is_type(facts.glyph)) {
                out.push(scored(
                    ReasonKind::New,
                    words::members_label(members),
                    Tier::Volume,
                ));
            }
            if facts.public {
                out.push(scored(
                    ReasonKind::NewPublic,
                    words::NEW_PUBLIC,
                    Tier::Volume,
                ));
            }
        }
        SymbolChangeKind::Modified | SymbolChangeKind::Removed => {}
    }

    // Complexity never ranks on its own; it explains code that changed anyway.
    if let Some(depth) = facts.metrics.depth.filter(|depth| *depth >= COMPLEX_DEPTH) {
        out.push(annotation(ReasonKind::Complex, words::nesting_label(depth)));
    }
    if let Some(params) = facts
        .metrics
        .params
        .filter(|params| *params >= COMPLEX_PARAMS)
    {
        out.push(annotation(ReasonKind::Complex, words::params_label(params)));
    }
    out
}

/// One-based inclusive line ranges on a single side.
type LineRanges = Vec<(u32, u32)>;

fn hunk_ranges(change: &SymbolChange) -> (LineRanges, LineRanges) {
    let mut old = Vec::new();
    let mut new = Vec::new();
    for hunk in change.hunks() {
        if let Some(range) = hunk.old() {
            old.push((range.start().get(), range.end().get()));
        }
        if let Some(range) = hunk.new_range() {
            new.push((range.start().get(), range.end().get()));
        }
    }
    (old, new)
}

fn encloses(change: &CallDiffChange, key: &SymbolKey) -> bool {
    [change.old(), change.new_fact()]
        .into_iter()
        .flatten()
        .any(|fact| fact.enclosing_symbol() == Some(key))
}

fn call_rows(call_diff: &[CallDiffChange], key: &SymbolKey) -> Vec<CallRow> {
    call_diff
        .iter()
        .filter(|change| encloses(change, key))
        .map(|change| {
            let fact = change.new_fact().or_else(|| change.old());
            CallRow {
                change: change.kind(),
                callee: fact
                    .map(|fact| fact.callee_text().to_string())
                    .unwrap_or_default(),
                old_args: change.old().map(|fact| fact.argument_text().to_string()),
                new_args: change
                    .new_fact()
                    .map(|fact| fact.argument_text().to_string()),
                context: fact
                    .map(|fact| {
                        fact.control_context()
                            .iter()
                            .map(control_context_word)
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

fn call_contexts<'a>(call_diff: &'a [CallDiffChange], key: &SymbolKey) -> Vec<&'a ControlContext> {
    call_diff
        .iter()
        .filter(|change| encloses(change, key))
        .flat_map(|change| {
            [change.old(), change.new_fact()]
                .into_iter()
                .flatten()
                .flat_map(|fact| fact.control_context())
        })
        .collect()
}

fn head_hotspots<'a>(
    hotspots: &'a [StructuralHotspot],
    key: &'a SymbolKey,
) -> impl Iterator<Item = &'a StructuralHotspot> {
    hotspots.iter().filter(move |hotspot| {
        hotspot.symbol().side() == ComparisonSide::Head && hotspot.symbol().key() == key
    })
}

/// Hotspots exist for every head-side symbol; only changed ones reach the model.
fn symbol_metrics(hotspots: &[StructuralHotspot], key: &SymbolKey) -> SymbolMetrics {
    let mut metrics = SymbolMetrics::default();
    for hotspot in head_hotspots(hotspots, key) {
        match hotspot.metric() {
            StructuralMetric::FunctionLineCount { lines } => metrics.lines = Some(*lines),
            StructuralMetric::ParameterCount { parameters } => metrics.params = Some(*parameters),
            StructuralMetric::SyntacticNestingDepth { depth } => metrics.depth = Some(*depth),
            StructuralMetric::TypeMemberCount { members } => metrics.members = Some(*members),
            StructuralMetric::ChangedLines { .. } => {}
        }
    }
    metrics
}

fn has_changed_lines(hotspots: &[StructuralHotspot], key: &SymbolKey) -> bool {
    head_hotspots(hotspots, key)
        .any(|hotspot| matches!(hotspot.metric(), StructuralMetric::ChangedLines { .. }))
}

// -- directories -------------------------------------------------------------

/// The path a file occupies in the tree — head side when it has one.
fn tree_path(entry: &FileEntry) -> &str {
    entry
        .new_path
        .as_deref()
        .or(entry.old_path.as_deref())
        .unwrap_or_default()
}

struct DirBuilder {
    name: String,
    path: String,
    children: BTreeMap<String, DirBuilder>,
    files: Vec<usize>,
}

impl DirBuilder {
    fn new(name: String, path: String) -> Self {
        Self {
            name,
            path,
            children: BTreeMap::new(),
            files: Vec::new(),
        }
    }
}

fn directory_tree(files: &[FileEntry]) -> DirNode {
    let mut root = DirBuilder::new(String::new(), String::new());
    for (index, entry) in files.iter().enumerate() {
        let path = tree_path(entry);
        let segments: Vec<&str> = path.split('/').collect();
        let mut node = &mut root;
        for segment in segments.iter().take(segments.len().saturating_sub(1)) {
            let child_path = if node.path.is_empty() {
                (*segment).to_string()
            } else {
                format!("{}/{segment}", node.path)
            };
            node = node
                .children
                .entry((*segment).to_string())
                .or_insert_with(|| DirBuilder::new((*segment).to_string(), child_path));
        }
        node.files.push(index);
    }
    let mut root = finish_dir(root, files);
    mark_test_coverage(&mut root, files);
    root
}

fn finish_dir(builder: DirBuilder, files: &[FileEntry]) -> DirNode {
    let children: Vec<DirNode> = builder
        .children
        .into_values()
        .map(|child| join_chain(finish_dir(child, files)))
        .collect();
    let mut lines_added = 0u64;
    let mut lines_deleted = 0u64;
    let mut file_count = builder.files.len();
    let mut is_implementation_dir = false;
    for index in &builder.files {
        let Some(entry) = files.get(*index) else {
            continue;
        };
        lines_added = lines_added.saturating_add(entry.lines_added);
        lines_deleted = lines_deleted.saturating_add(entry.lines_deleted);
        is_implementation_dir |= entry.role == FileRole::Implementation;
    }
    for child in &children {
        file_count = file_count.saturating_add(child.file_count);
        lines_added = lines_added.saturating_add(child.lines_added);
        lines_deleted = lines_deleted.saturating_add(child.lines_deleted);
    }
    DirNode {
        name: builder.name,
        path: builder.path,
        children,
        files: builder.files,
        file_count,
        lines_added,
        lines_deleted,
        is_implementation_dir,
        no_test_changes: false,
    }
}

/// Collapse `a` → `b` → `c` into one `a/b/c` row.
fn join_chain(mut node: DirNode) -> DirNode {
    while node.files.is_empty() && node.children.len() == 1 {
        let child = node.children.remove(0);
        node.name = format!("{}/{}", node.name, child.name);
        node.path = child.path;
        node.children = child.children;
        node.files = child.files;
        node.is_implementation_dir = child.is_implementation_dir;
    }
    node
}

/// Returns whether the subtree contains a test file.
fn mark_test_coverage(node: &mut DirNode, files: &[FileEntry]) -> bool {
    let mut has_test = node
        .files
        .iter()
        .any(|index| files.get(*index).is_some_and(|entry| entry.is_test));
    for child in &mut node.children {
        has_test |= mark_test_coverage(child, files);
    }
    node.no_test_changes = node.is_implementation_dir && !has_test;
    has_test
}

/// Implementation directories with no test change, top-most first and never
/// repeated on a child — spec §6 tier 4.
fn topmost_untested<'a>(node: &'a DirNode, out: &mut Vec<&'a DirNode>) {
    if node.no_test_changes {
        out.push(node);
        return;
    }
    for child in &node.children {
        topmost_untested(child, out);
    }
}

fn implementation_dirs<'a>(node: &'a DirNode, out: &mut Vec<&'a DirNode>) {
    if node.is_implementation_dir {
        out.push(node);
    }
    for child in &node.children {
        implementation_dirs(child, out);
    }
}

fn implementation_files_under(files: &[FileEntry], dir: &str) -> usize {
    files
        .iter()
        .filter(|entry| entry.role == FileRole::Implementation && is_under(tree_path(entry), dir))
        .count()
}

// -- volume ------------------------------------------------------------------

fn volume_rows(files: &[FileEntry], total_changed_lines: u64) -> Vec<VolumeRow> {
    let mut counts = Vec::with_capacity(ALL_ROLES.len());
    for role in ALL_ROLES {
        let mut role_files = 0usize;
        let mut lines = 0u64;
        for entry in files.iter().filter(|entry| entry.role == role) {
            role_files += 1;
            lines = lines.saturating_add(entry.changed_lines());
        }
        counts.push((role, role_files, lines));
    }
    // Binary-only comparisons have no lines to share out, so files carry the bar.
    let parts: Vec<u64> = if total_changed_lines > 0 {
        counts.iter().map(|(_, _, lines)| *lines).collect()
    } else {
        counts
            .iter()
            .map(|(_, role_files, _)| u64::try_from(*role_files).unwrap_or(u64::MAX))
            .collect()
    };
    let total = parts
        .iter()
        .fold(0u64, |total, part| total.saturating_add(*part));
    let permille = apportion(&parts, total);
    counts
        .into_iter()
        .zip(permille)
        .map(|((role, role_files, lines), permille)| VolumeRow {
            role,
            files: role_files,
            lines,
            percent: f32::from(permille) / 10.0,
        })
        .collect()
}

/// Largest-remainder split into permille, so the shares always add up to 100 %.
fn apportion(parts: &[u64], total: u64) -> Vec<u16> {
    if total == 0 {
        return vec![0; parts.len()];
    }
    let mut shares: Vec<u64> = parts
        .iter()
        .map(|part| part.saturating_mul(1_000) / total)
        .collect();
    let assigned = shares
        .iter()
        .fold(0u64, |sum, share| sum.saturating_add(*share));
    let mut spare = 1_000u64.saturating_sub(assigned);
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by(|left, right| {
        let left_rest = parts[*left].saturating_mul(1_000) % total;
        let right_rest = parts[*right].saturating_mul(1_000) % total;
        right_rest.cmp(&left_rest).then(left.cmp(right))
    });
    for index in order {
        if spare == 0 {
            break;
        }
        if parts[index] == 0 {
            continue;
        }
        shares[index] = shares[index].saturating_add(1);
        spare -= 1;
    }
    shares
        .into_iter()
        .map(|share| u16::try_from(share.min(1_000)).unwrap_or(1_000))
        .collect()
}

// -- attention ---------------------------------------------------------------

/// One row plus the keys that place it. `sub_rank` carries the two orderings
/// spec §6 states inside a tier (signature+body first, mechanical moves last).
struct Ranked {
    item: AttentionItem,
    sub_rank: u8,
    implementation: bool,
    sort_path: String,
    source: usize,
}

fn attention_items(files: &[FileEntry], root: &DirNode) -> Vec<AttentionItem> {
    let mut ranked: Vec<Ranked> = Vec::new();
    let mut source = 0usize;
    for entry in files {
        if entry.symbols.is_empty() {
            ranked.push(file_row(entry, source));
        } else {
            for symbol in &entry.symbols {
                ranked.push(symbol_row(entry, symbol, source));
                source += 1;
            }
        }
        source += 1;
    }
    let mut untested = Vec::new();
    topmost_untested(root, &mut untested);
    for node in untested {
        ranked.push(directory_row(node, files, source));
        source += 1;
    }

    ranked.sort_by(|left, right| {
        left.item
            .tier
            .cmp(&right.item.tier)
            .then_with(|| left.sub_rank.cmp(&right.sub_rank))
            .then_with(|| right.implementation.cmp(&left.implementation))
            .then_with(|| right.item.reasons.len().cmp(&left.item.reasons.len()))
            .then_with(|| churn(&right.item).cmp(&churn(&left.item)))
            .then_with(|| left.sort_path.cmp(&right.sort_path))
            .then_with(|| left.source.cmp(&right.source))
    });
    ranked.into_iter().map(|ranked| ranked.item).collect()
}

fn churn(item: &AttentionItem) -> u64 {
    item.lines_added.saturating_add(item.lines_deleted)
}

fn symbol_row(entry: &FileEntry, symbol: &SymbolEntry, source: usize) -> Ranked {
    Ranked {
        item: AttentionItem {
            target: AttentionTarget::Symbol {
                file: entry.key.clone(),
                change_index: symbol.change_index,
            },
            tier: symbol.tier,
            reasons: symbol.reasons.clone(),
            name: symbol.qualified.clone(),
            path: entry.display_path.clone(),
            glyph: symbol.glyph,
            lines_added: u64::from(symbol.lines_added),
            lines_deleted: u64::from(symbol.lines_deleted),
            dimmed: false,
            is_test: entry.is_test,
        },
        sub_rank: symbol_sub_rank(symbol),
        implementation: is_implementation_like(entry.role),
        sort_path: entry.display_path.clone(),
        source,
    }
}

fn symbol_sub_rank(symbol: &SymbolEntry) -> u8 {
    match symbol.tier {
        Tier::Contract => {
            let signature = symbol.reasons.iter().any(|reason| {
                matches!(
                    reason.kind,
                    ReasonKind::PublicSignature | ReasonKind::ExportedSignature
                )
            });
            let body = symbol
                .reasons
                .iter()
                .any(|reason| reason.kind == ReasonKind::Body);
            u8::from(signature && !body)
        }
        // Calls that vanished or changed shape come before calls merely added.
        Tier::Behaviour => u8::from(!symbol.calls.iter().any(|call| {
            matches!(
                call.change,
                CallChangeKind::Removed | CallChangeKind::Modified
            )
        })),
        Tier::Volume | Tier::GitFacts | Tier::Rest => 0,
    }
}

fn file_row(entry: &FileEntry, source: usize) -> Ranked {
    // Inside the rest tier the leftover files follow the leftover symbols, and a
    // move that only moved comes last of all — spec §6 tier 5.
    let sub_rank = match (entry.tier, is_likely_mechanical(entry)) {
        (Tier::Rest, true) => 2,
        (Tier::Rest, false) => 1,
        _ => 0,
    };
    Ranked {
        item: AttentionItem {
            target: AttentionTarget::File(entry.key.clone()),
            tier: entry.tier,
            reasons: entry.reasons.clone(),
            name: basename(&entry.display_path).to_string(),
            path: entry.display_path.clone(),
            glyph: KindGlyph::File,
            lines_added: entry.lines_added,
            lines_deleted: entry.lines_deleted,
            dimmed: entry
                .reasons
                .iter()
                .any(|reason| reason.kind == ReasonKind::NotAnalyzed),
            is_test: entry.is_test,
        },
        sub_rank,
        implementation: is_implementation_like(entry.role),
        sort_path: entry.display_path.clone(),
        source,
    }
}

fn directory_row(node: &DirNode, files: &[FileEntry], source: usize) -> Ranked {
    let name = if node.path.is_empty() {
        "repository root".to_string()
    } else {
        node.path.clone()
    };
    Ranked {
        item: AttentionItem {
            target: AttentionTarget::Directory(node.path.clone()),
            tier: Tier::GitFacts,
            reasons: vec![Reason {
                kind: ReasonKind::NoTestChanges,
                label: words::NO_TEST_CHANGES.to_string(),
            }],
            name,
            path: words::implementation_files(implementation_files_under(files, &node.path)),
            glyph: KindGlyph::Directory,
            lines_added: node.lines_added,
            lines_deleted: node.lines_deleted,
            dimmed: false,
            is_test: false,
        },
        sub_rank: 0,
        implementation: true,
        sort_path: node.path.clone(),
        source,
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// -- facts -------------------------------------------------------------------

fn public_api_fact(
    files: &[FileEntry],
    structure: Option<&ReviewStructure>,
    coverage: &CoverageSummary,
) -> Option<PublicApiFact> {
    let structure = structure?;
    let mut removed = 0u64;
    let mut signatures = 0u64;
    let mut added = 0u64;
    for symbol in files
        .iter()
        .flat_map(|entry| entry.symbols.iter())
        .filter(|symbol| symbol.public)
    {
        match symbol.change {
            SymbolChangeKind::Removed => removed += 1,
            SymbolChangeKind::Added => added += 1,
            SymbolChangeKind::Modified => {}
        }
        if symbol.signature.is_some() {
            signatures += 1;
        }
    }
    let no_supported_language = structure.language_coverage().is_empty();
    if removed == 0 && signatures == 0 && added == 0 && !no_supported_language {
        return None;
    }
    Some(PublicApiFact {
        removed,
        signatures,
        added,
        lower_bound: coverage.partial,
        languages: coverage.languages.clone(),
        no_supported_language,
    })
}

fn tests_fact(root: &DirNode) -> Option<TestsFact> {
    let mut dirs = Vec::new();
    implementation_dirs(root, &mut dirs);
    if dirs.is_empty() {
        return None;
    }
    let mut without: Vec<DirRef> = dirs
        .iter()
        .filter(|node| node.no_test_changes)
        .map(|node| DirRef {
            path: node.path.clone(),
            files: node.file_count,
            lines: node.lines_added.saturating_add(node.lines_deleted),
        })
        .collect();
    without.sort_by(|left, right| {
        right
            .lines
            .cmp(&left.lines)
            .then_with(|| left.path.cmp(&right.path))
    });
    Some(TestsFact {
        impl_dirs: dirs.len(),
        with_tests: dirs.len().saturating_sub(without.len()),
        without,
    })
}

fn moves_fact(files: &[FileEntry]) -> Option<MovesFact> {
    let renames: Vec<&FileEntry> = files
        .iter()
        .filter(|entry| entry.status == ReviewFileStatus::Renamed)
        .collect();
    if renames.is_empty() {
        return None;
    }
    let likely_mechanical = renames
        .iter()
        .filter(|entry| is_likely_mechanical(entry))
        .count();
    let similarities: Vec<u64> = renames
        .iter()
        .filter_map(|entry| entry.similarity.map(u64::from))
        .collect();
    let average = if similarities.is_empty() {
        0
    } else {
        let total = similarities
            .iter()
            .fold(0u64, |sum, value| sum.saturating_add(*value));
        let count = u64::try_from(similarities.len()).unwrap_or(1);
        u8::try_from(total / count.max(1)).unwrap_or(u8::MAX)
    };
    Some(MovesFact {
        total: renames.len(),
        likely_mechanical,
        with_edits: renames.len().saturating_sub(likely_mechanical),
        avg_similarity: average,
        // Residual lines across every rename; the split above says how they land.
        residual_lines: renames.iter().fold(0u64, |total, entry| {
            total.saturating_add(entry.changed_lines())
        }),
    })
}

fn commits_fact(commits: &[CommitRow]) -> Option<CommitsFact> {
    let first = commits.first()?;
    let last = commits.last()?;
    let mut authors: Vec<String> = Vec::new();
    for commit in commits {
        if !authors.contains(&commit.author) {
            authors.push(commit.author.clone());
        }
    }
    let oldest = commits.iter().map(|commit| commit.timestamp).min()?;
    let newest = commits.iter().map(|commit| commit.timestamp).max()?;
    Some(CommitsFact {
        count: commits.len(),
        merges: commits.iter().filter(|commit| commit.is_merge).count(),
        authors,
        span_secs: newest.saturating_sub(oldest),
        first_sha: first.short_sha.clone(),
        last_sha: last.short_sha.clone(),
    })
}

fn also_fact(files: &[FileEntry]) -> Option<AlsoFact> {
    let count = |kind: ReasonKind| {
        files
            .iter()
            .filter(|entry| entry.reasons.iter().any(|reason| reason.kind == kind))
            .count()
    };
    let fact = AlsoFact {
        lockfiles: count(ReasonKind::Lockfile),
        submodules: count(ReasonKind::Submodule),
        binaries: files.iter().filter(|entry| entry.binary).count(),
        deleted_impl: count(ReasonKind::DeletedImpl),
    };
    let empty =
        fact.lockfiles == 0 && fact.submodules == 0 && fact.binaries == 0 && fact.deleted_impl == 0;
    (!empty).then_some(fact)
}

fn commit_rows(inventory: &ReviewInventory) -> Vec<CommitRow> {
    inventory
        .commits
        .iter()
        .map(|commit| CommitRow {
            sha: commit.oid.as_str().to_string(),
            short_sha: super::labels::short_sha(commit.oid.as_str()),
            subject: commit.subject.clone(),
            author: commit.author_name.clone(),
            timestamp: commit.timestamp,
            is_merge: commit.parent_oids.len() > 1,
        })
        .collect()
}

// -- status, omissions, coverage ---------------------------------------------

fn analysis_status(
    inventory: &ReviewInventory,
    structure: Option<&ReviewStructure>,
    state: &StructureLoad,
) -> AnalysisStatus {
    match state {
        StructureLoad::NotStarted | StructureLoad::Loading => AnalysisStatus::AnalyzingStructure,
        StructureLoad::Failed(error) => AnalysisStatus::Unavailable {
            message: error.clone(),
        },
        StructureLoad::Ready => {
            let coverage = structure.map_or(&inventory.coverage, ReviewStructure::coverage);
            // A capped run is limited even when the rest parsed cleanly.
            if coverage.pending_items() > 0 || coverage.truncation().is_some() {
                AnalysisStatus::Limited {
                    analyzed: coverage.analyzed_items(),
                    total: coverage.total_items(),
                }
            } else if coverage.failed_items() > 0 {
                AnalysisStatus::ReadyWithFailures {
                    failed: coverage.failed_items(),
                }
            } else {
                AnalysisStatus::Ready {
                    files: coverage.analyzed_items(),
                    languages: structure_languages(structure),
                }
            }
        }
    }
}

fn structure_languages(structure: Option<&ReviewStructure>) -> Vec<String> {
    structure
        .map(|structure| {
            structure
                .language_coverage()
                .iter()
                .map(|coverage| language_label(&coverage.language()).to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One merged row per omission reason, plus the parse failures — spec §10.
fn omission_rows(structure: Option<&ReviewStructure>, files: &[FileEntry]) -> Vec<OmissionRow> {
    let Some(structure) = structure else {
        return Vec::new();
    };
    let mut merged: Vec<(OmittedFileReason, MergedOmission)> = Vec::new();
    for group in structure.omissions() {
        let slot = match merged
            .iter_mut()
            .find(|(reason, _)| *reason == group.reason())
        {
            Some((_, slot)) => slot,
            None => {
                merged.push((group.reason(), MergedOmission::default()));
                match merged.last_mut() {
                    Some((_, slot)) => slot,
                    None => continue,
                }
            }
        };
        slot.absorb(group);
    }
    let mut rows: Vec<OmissionRow> = merged
        .into_iter()
        .map(|(reason, merged)| OmissionRow {
            sentence: words::omission_sentence(reason, merged.limit),
            count: merged.count,
            detail: merged.detail(reason, files),
            warn: words::omission_warns(reason),
        })
        .collect();
    if let Some(row) = failure_row(structure, files) {
        rows.push(row);
    }
    rows
}

#[derive(Default)]
struct MergedOmission {
    count: u64,
    limit: Option<u64>,
    detail: Option<String>,
    languages: Vec<(String, u64)>,
}

impl MergedOmission {
    fn absorb(&mut self, group: &OmittedFileGroup) {
        self.count = self.count.saturating_add(group.count());
        if let Some(truncation) = group.truncation() {
            self.limit = self.limit.or(truncation.limit);
            self.detail = self.detail.take().or_else(|| truncation.detail.clone());
        }
        if let Some(language) = group.language() {
            let name = language_label(&language).to_string();
            match self
                .languages
                .iter_mut()
                .find(|(existing, _)| *existing == name)
            {
                Some((_, count)) => *count = count.saturating_add(group.count()),
                None => self.languages.push((name, group.count())),
            }
        }
    }

    fn detail(&self, reason: OmittedFileReason, files: &[FileEntry]) -> String {
        if !self.languages.is_empty() {
            return words::extension_summary(&self.languages);
        }
        if reason == OmittedFileReason::UnsupportedLanguage {
            return words::extension_summary(&unsupported_extensions(files));
        }
        self.detail.clone().unwrap_or_default()
    }
}

/// What the unsupported group actually held, read back from the file list.
fn unsupported_extensions(files: &[FileEntry]) -> Vec<(String, u64)> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for entry in files
        .iter()
        .filter(|entry| entry.analysis == FileAnalysis::Unsupported)
    {
        let Some(extension) = extension_of(tree_path(entry)) else {
            continue;
        };
        *counts.entry(format!(".{extension}")).or_default() += 1;
    }
    let mut ordered: Vec<(String, u64)> = counts.into_iter().collect();
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ordered.truncate(OMISSION_EXTENSIONS);
    ordered
}

fn extension_of(path: &str) -> Option<String> {
    Some(path.rsplit('/').next()?.rsplit_once('.')?.1.to_lowercase())
}

fn failure_row(structure: &ReviewStructure, files: &[FileEntry]) -> Option<OmissionRow> {
    let failed = files
        .iter()
        .filter(|entry| entry.analysis == FileAnalysis::Failed)
        .count();
    let error = structure
        .files()
        .iter()
        .flat_map(StructuredFile::errors)
        .chain(structure.errors())
        .next();
    if failed == 0 && error.is_none() {
        return None;
    }
    Some(OmissionRow {
        sentence: words::FAILED_TO_PARSE.to_string(),
        count: u64::try_from(failed).unwrap_or(0),
        detail: error
            .map(|error| words::failure_detail(error.stage(), error.message()))
            .unwrap_or_default(),
        warn: true,
    })
}

fn coverage_summary(
    inventory: &ReviewInventory,
    structure: Option<&ReviewStructure>,
    files: &[FileEntry],
) -> CoverageSummary {
    let coverage: &ReviewCoverage =
        structure.map_or(&inventory.coverage, ReviewStructure::coverage);
    let implementation = files
        .iter()
        .filter(|entry| entry.role == FileRole::Implementation);
    let impl_total = implementation.clone().count();
    let impl_analyzed = implementation
        .filter(|entry| entry.analysis.is_analyzed())
        .count();
    CoverageSummary {
        analyzed_files: coverage.analyzed_items(),
        total_files: coverage.total_items(),
        impl_analyzed,
        impl_total,
        path_order_bias: structure.is_some_and(|structure| {
            structure
                .omissions()
                .iter()
                .any(|group| group.reason() == OmittedFileReason::FileLimit)
        }),
        languages: structure_languages(structure),
        partial: !coverage.is_complete(),
        failed: coverage.failed_items(),
        base_oid: snapshot_oid(&inventory.comparison, true),
        head_oid: snapshot_oid(&inventory.comparison, false),
        merge_base_oid: inventory
            .comparison
            .merge_base_oid()
            .map(|oid| oid.as_str().to_string()),
    }
}

fn snapshot_oid(comparison: &ResolvedComparison, base: bool) -> String {
    let snapshot = if base {
        comparison.base()
    } else {
        comparison.head()
    };
    snapshot
        .oid()
        .map(|oid| oid.as_str().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::super::model::{
        AnalysisStatus, AttentionTarget, KindGlyph, ReasonKind, ReviewModel, Tier,
    };
    use super::{ModelInputs, StructureLoad, apportion, build_review_model};
    use okena_core::review::ReviewInventory;
    use okena_git::DiffMode;
    use okena_review::ReviewStructure;
    use serde_json::{Value, json};

    fn branch() -> DiffMode {
        DiffMode::BranchCompare {
            base: "main".into(),
            head: "feature".into(),
        }
    }

    fn model_of(
        inventory: &ReviewInventory,
        structure: Option<&ReviewStructure>,
        state: StructureLoad,
    ) -> ReviewModel {
        let mode = branch();
        build_review_model(ModelInputs {
            inventory: Some(inventory),
            inventory_error: None,
            structure,
            structure_state: state,
            diff_mode: &mode,
        })
    }

    /// Every attention row as `name | tier | reason kinds`.
    fn rows(model: &ReviewModel) -> Vec<String> {
        model
            .attention
            .iter()
            .map(|item| {
                let kinds: Vec<String> = item
                    .reasons
                    .iter()
                    .map(|reason| format!("{:?}", reason.kind))
                    .collect();
                format!("{} | {:?} | {}", item.name, item.tier, kinds.join("+"))
            })
            .collect()
    }

    fn find(model: &ReviewModel, name: &str) -> usize {
        model
            .attention
            .iter()
            .position(|item| item.name == name)
            .unwrap_or_else(|| panic!("{name} is missing from the attention list"))
    }

    #[test]
    fn the_attention_list_is_the_whole_ranking_in_order() {
        let model = fixtures::model();
        assert_eq!(
            rows(&model),
            [
                "Engine::run | Contract | PublicSignature+Body+Calls",
                "legacy.rs | Contract | DeletedImpl+NotAnalyzed",
                "Engine::legacy_run | Contract | PublicRemoved",
                "Engine::configure | Contract | PublicSignature",
                "Engine::dispatch | Behaviour | Calls",
                "normalize | Behaviour | Calls+Body",
                "orchestrate | Volume | New+NewPublic+Complex+Complex",
                "motion_new.rs | GitFacts | Moved+Moved+NotAnalyzed",
                "handler.rs | GitFacts | NotAnalyzed+LargeChurn",
                "lib.rs | GitFacts | New+NotAnalyzed",
                "logo.png | GitFacts | Binary+NotAnalyzed",
                "src | GitFacts | NoTestChanges",
                "pnpm-lock.yaml | GitFacts | Lockfile+NotAnalyzed",
                "Cargo.toml | GitFacts | CiConfig+NotAnalyzed",
                "app.js | Rest | NotAnalyzed",
                "handler_test.rs | Rest | NotAnalyzed",
                "README.md | Rest | NotAnalyzed",
                "lib.rs | Rest | NotAnalyzed",
                "new.rs | Rest | Moved+NotAnalyzed",
            ]
        );
    }

    #[test]
    fn signature_and_body_outranks_signature_only_inside_the_contract_tier() {
        let model = fixtures::model();
        assert!(find(&model, "Engine::run") < find(&model, "Engine::configure"));
        let contract: Vec<&str> = model
            .attention
            .iter()
            .filter(|item| item.tier == Tier::Contract)
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(
            contract.last(),
            Some(&"Engine::configure"),
            "the signature-only change closes the tier"
        );
    }

    #[test]
    fn calls_that_vanished_outrank_calls_merely_added() {
        let model = fixtures::model();
        // `normalize` carries more reasons, so only the sub-rank can order these.
        assert!(find(&model, "Engine::dispatch") < find(&model, "normalize"));
    }

    #[test]
    fn every_symbol_and_file_appears_exactly_once() {
        let model = fixtures::model();
        let mut targets: Vec<&AttentionTarget> =
            model.attention.iter().map(|item| &item.target).collect();
        let before = targets.len();
        targets.sort_by_key(|target| format!("{target:?}"));
        targets.dedup_by_key(|target| format!("{target:?}"));
        assert_eq!(targets.len(), before, "no target is listed twice");

        let with_symbols = model
            .files
            .iter()
            .filter(|entry| !entry.symbols.is_empty())
            .count();
        assert_eq!(
            model.attention.len(),
            model.files.len() - with_symbols
                + model
                    .files
                    .iter()
                    .map(|entry| entry.symbols.len())
                    .sum::<usize>()
                + 1,
            "every file is represented by its symbols or by itself, plus one directory"
        );
    }

    #[test]
    fn complexity_and_not_analyzed_never_lift_an_item_out_of_the_rest_tier() {
        let model = fixtures::model();
        for item in &model.attention {
            let ranking = item
                .reasons
                .iter()
                .filter(|reason| {
                    !matches!(reason.kind, ReasonKind::Complex | ReasonKind::NotAnalyzed)
                })
                .count();
            assert!(
                item.tier == Tier::Rest || ranking > 0,
                "{} was ranked by an annotation alone",
                item.name
            );
        }
        let orchestrate = &model.attention[find(&model, "orchestrate")];
        assert_eq!(orchestrate.tier, Tier::Volume);
        assert!(
            orchestrate
                .reasons
                .iter()
                .any(|reason| reason.label == "nesting 6")
        );
        assert!(
            orchestrate
                .reasons
                .iter()
                .any(|reason| reason.label == "8 params")
        );
    }

    #[test]
    fn hotspots_for_symbols_that_did_not_change_are_ignored() {
        let model = fixtures::model();
        assert!(
            !model
                .attention
                .iter()
                .any(|item| item.name.contains("helper")),
            "an untouched hotspot must not become a row"
        );
        let engine = model
            .files
            .iter()
            .find(|entry| entry.display_path == "src/engine.rs")
            .expect("the fixture analyses src/engine.rs");
        assert_eq!(engine.symbols.len(), 6);
        assert!(!engine.symbols.iter().any(|symbol| symbol.name == "helper"));
    }

    #[test]
    fn the_untested_directory_marker_lands_on_the_top_most_directory_only() {
        let model = fixtures::model();
        let dirs: Vec<&str> = model
            .attention
            .iter()
            .filter(|item| item.glyph == KindGlyph::Directory)
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(dirs, ["src"], "worker changed tests, so only src is bare");
        let row = &model.attention[find(&model, "src")];
        assert_eq!(row.path, "6 implementation files");
        assert_eq!(row.reasons[0].label, "no test files changed next to it");
    }

    #[test]
    fn renames_split_at_twenty_residual_lines() {
        let inventory = renames_inventory();
        let model = model_of(&inventory, None, StructureLoad::Loading);
        let mechanical = &model.files[0];
        let edited = &model.files[1];
        assert_eq!(mechanical.changed_lines(), 20);
        assert_eq!(mechanical.tier, Tier::Rest);
        assert_eq!(
            mechanical
                .reasons
                .iter()
                .map(|reason| reason.label.as_str())
                .collect::<Vec<_>>(),
            ["moved 99 %"]
        );
        assert_eq!(edited.changed_lines(), 21);
        assert_eq!(edited.tier, Tier::GitFacts);
        assert_eq!(
            edited
                .reasons
                .iter()
                .map(|reason| reason.label.as_str())
                .collect::<Vec<_>>(),
            ["moved 91 %", "21 residual lines"]
        );

        let moves = model.facts.moves.expect("two renames make a moves fact");
        assert_eq!(moves.total, 2);
        assert_eq!(moves.likely_mechanical, 1);
        assert_eq!(moves.with_edits, 1);
        assert_eq!(moves.avg_similarity, 95);
        assert_eq!(moves.residual_lines, 41);
    }

    #[test]
    fn public_api_counts_are_lower_bounds_exactly_when_coverage_is_partial() {
        let model = fixtures::model();
        let fact = model
            .facts
            .public_api
            .clone()
            .expect("the fixture changes public symbols");
        assert_eq!((fact.removed, fact.signatures, fact.added), (1, 2, 1));
        assert!(model.coverage.partial);
        assert!(fact.lower_bound);
        assert!(!fact.no_supported_language);
        assert_eq!(fact.languages, ["Rust"]);

        let unsupported = model_of(
            &fixtures::inventory_all_unsupported(),
            Some(&fixtures::structure_empty()),
            StructureLoad::Ready,
        );
        let fact = unsupported
            .facts
            .public_api
            .expect("an all-unsupported comparison still says so");
        assert!(!unsupported.coverage.partial);
        assert!(!fact.lower_bound);
        assert!(fact.no_supported_language);
        assert_eq!(
            unsupported
                .attention
                .iter()
                .filter(|item| item.glyph == KindGlyph::File)
                .count(),
            3,
            "nothing is empty; every file is ranked from git facts"
        );
    }

    #[test]
    fn the_status_matrix_follows_coverage_and_the_load_state() {
        let inventory = fixtures::inventory();
        assert_eq!(
            build_review_model(ModelInputs {
                inventory: None,
                inventory_error: None,
                structure: None,
                structure_state: StructureLoad::NotStarted,
                diff_mode: &branch(),
            })
            .status,
            AnalysisStatus::LoadingInventory
        );
        assert_eq!(
            build_review_model(ModelInputs {
                inventory: None,
                inventory_error: Some("git exited 128"),
                structure: None,
                structure_state: StructureLoad::NotStarted,
                diff_mode: &branch(),
            })
            .status,
            AnalysisStatus::Unavailable {
                message: "git exited 128".into()
            }
        );
        assert_eq!(
            model_of(&inventory, None, StructureLoad::Loading).status,
            AnalysisStatus::AnalyzingStructure
        );
        assert_eq!(
            model_of(
                &inventory,
                None,
                StructureLoad::Failed("worker exited".into())
            )
            .status,
            AnalysisStatus::Unavailable {
                message: "worker exited".into()
            }
        );
        assert_eq!(
            model_of(
                &inventory,
                Some(&fixtures::structure()),
                StructureLoad::Ready
            )
            .status,
            AnalysisStatus::Limited {
                analyzed: 1,
                total: 5
            },
            "a pending group outranks the parse failure next to it"
        );
        assert_eq!(
            model_of(
                &inventory,
                Some(&fixtures::structure_with_failure()),
                StructureLoad::Ready
            )
            .status,
            AnalysisStatus::ReadyWithFailures { failed: 1 }
        );
        assert_eq!(
            model_of(
                &inventory,
                Some(&fixtures::structure_empty()),
                StructureLoad::Ready
            )
            .status,
            AnalysisStatus::Ready {
                files: 0,
                languages: Vec::new()
            }
        );
    }

    #[test]
    fn omission_rows_are_sentences_and_never_debug_names() {
        let model = fixtures::model();
        let sentences: Vec<&str> = model
            .omissions
            .iter()
            .map(|row| row.sentence.as_str())
            .collect();
        assert_eq!(
            sentences,
            [
                "Not analyzed \u{2014} file limit (200), taken in path order",
                "Failed to parse"
            ]
        );
        assert_eq!(model.omissions[0].count, 2);
        assert!(model.omissions[0].warn);
        assert_eq!(model.omissions[1].detail, "parsing: unexpected token");

        for row in &model.omissions {
            for name in [
                "SourceByteLimit",
                "ByteLimit",
                "ItemLimit",
                "FileLimit",
                "UnsupportedLanguage",
                "Parsing",
            ] {
                assert!(
                    !row.sentence.contains(name),
                    "{} leaks {name}",
                    row.sentence
                );
                assert!(!row.detail.contains(name), "{} leaks {name}", row.detail);
            }
        }
    }

    #[test]
    fn volume_shares_add_up_to_one_hundred_percent() {
        for model in [
            fixtures::model(),
            model_of(&fixtures::inventory_small(), None, StructureLoad::Loading),
            model_of(
                &fixtures::inventory_binary_only(),
                None,
                StructureLoad::Loading,
            ),
        ] {
            let total: f32 = model.volume.iter().map(|row| row.percent).sum();
            assert!(
                (total - 100.0).abs() < 0.1,
                "shares add up to {total}, not 100"
            );
            assert_eq!(model.volume.len(), 11, "every role stays in the model");
        }
        // Binary-only comparisons have no lines, so the bar is built from files.
        let binary = model_of(
            &fixtures::inventory_binary_only(),
            None,
            StructureLoad::Loading,
        );
        assert_eq!(binary.total_changed_lines, 0);
        let unclassified = binary
            .volume
            .iter()
            .find(|row| row.files == 2)
            .expect("both binaries are unclassified");
        assert!((unclassified.percent - 100.0).abs() < 0.1);
    }

    #[test]
    fn apportioned_shares_always_spend_the_whole_thousand() {
        assert_eq!(apportion(&[1, 1, 1], 3), [334, 333, 333]);
        assert_eq!(apportion(&[0, 0], 0), [0, 0]);
        assert_eq!(apportion(&[7], 7), [1_000]);
        let shares = apportion(&[585, 12, 6, 3, 160, 0], 766);
        assert_eq!(
            shares.iter().map(|share| u64::from(*share)).sum::<u64>(),
            1_000
        );
        assert_eq!(shares[5], 0, "a role with nothing changed stays at zero");
    }

    #[test]
    fn small_comparisons_are_bounded_by_files_or_by_lines() {
        assert!(model_of(&fixtures::inventory_small(), None, StructureLoad::Loading).small_change);
        assert!(!fixtures::model().small_change);
        // Ten files or five hundred lines is still small; both bounds must fail.
        let ten = model_of(&sized_inventory(10, 60), None, StructureLoad::Loading);
        assert!(ten.small_change, "ten files stay small however large");
        let quiet = model_of(&sized_inventory(11, 45), None, StructureLoad::Loading);
        assert_eq!(quiet.total_changed_lines, 495);
        assert!(quiet.small_change, "under five hundred lines stays small");
        let big = model_of(&sized_inventory(11, 46), None, StructureLoad::Loading);
        assert_eq!(big.total_changed_lines, 506);
        assert!(!big.small_change);
    }

    #[test]
    fn files_structure_never_reached_are_dimmed_and_say_why() {
        let model = fixtures::model();
        let javascript = &model.attention[find(&model, "app.js")];
        assert!(javascript.dimmed);
        assert_eq!(
            javascript
                .reasons
                .iter()
                .map(|reason| reason.label.as_str())
                .collect::<Vec<_>>(),
            ["not analyzed \u{00B7} JS"]
        );
        let engine = &model.attention[find(&model, "Engine::run")];
        assert!(!engine.dimmed, "analysed symbols are never dimmed");

        // While structure is still loading nothing is claimed about analysis.
        let loading = model_of(&fixtures::inventory(), None, StructureLoad::Loading);
        assert!(loading.attention.iter().all(|item| !item.dimmed));
        assert!(loading.files.iter().all(|entry| {
            entry
                .reasons
                .iter()
                .all(|reason| reason.kind != ReasonKind::NotAnalyzed)
        }));
    }

    #[test]
    fn the_supporting_facts_read_off_the_same_files() {
        let model = fixtures::model();
        let tests = model.facts.tests.clone().expect("two implementation dirs");
        assert_eq!(tests.impl_dirs, 2);
        assert_eq!(tests.with_tests, 1);
        assert_eq!(
            tests
                .without
                .iter()
                .map(|dir| dir.path.as_str())
                .collect::<Vec<_>>(),
            ["src"]
        );
        assert_eq!(tests.without[0].files, 6);

        let also = model
            .facts
            .also
            .expect("the fixture touches supporting files");
        assert_eq!(
            (
                also.lockfiles,
                also.submodules,
                also.binaries,
                also.deleted_impl
            ),
            (1, 0, 1, 1)
        );

        let commits = model.facts.commits.clone().expect("the ledger has commits");
        assert_eq!(commits.count, 2);
        assert_eq!(commits.merges, 1);
        assert_eq!(commits.authors, ["Ada", "Bob"]);
        assert_eq!(commits.span_secs, 1);
        assert_eq!(commits.first_sha, "aaaaaaa");
        assert_eq!(commits.last_sha, "bbbbbbb");

        let mode = DiffMode::Commit("abc".into());
        let inventory = fixtures::inventory();
        let single = build_review_model(ModelInputs {
            inventory: Some(&inventory),
            inventory_error: None,
            structure: None,
            structure_state: StructureLoad::Loading,
            diff_mode: &mode,
        });
        assert!(single.commits.is_empty());
        assert!(single.facts.commits.is_none(), "spec §12 hides the fact");
    }

    #[test]
    fn coverage_reports_the_implementation_subset_and_the_path_order_bias() {
        let model = fixtures::model();
        assert_eq!(model.coverage.analyzed_files, 1);
        assert_eq!(model.coverage.total_files, 5);
        assert_eq!(model.coverage.impl_total, 7);
        assert_eq!(model.coverage.impl_analyzed, 1);
        assert!(model.coverage.path_order_bias);
        assert_eq!(model.coverage.failed, 1);
        assert_eq!(model.coverage.merge_base_oid, Some("2".repeat(40)));
    }

    fn file_json(path: &str, added: u64, deleted: u64) -> Value {
        json!({
            "old_path": path, "new_path": path, "status": "modified",
            "lines_added": added, "lines_deleted": deleted, "binary": false,
            "classification": { "role": "implementation",
                                "rule_id": "builtin.path.implementation.v1" },
            "provenance": { "source": "git" }
        })
    }

    fn wrap(files: Vec<Value>) -> ReviewInventory {
        let count = files.len();
        let totals = json!({
            "commits": 0, "files": count, "files_added": 0, "files_deleted": 0,
            "files_modified": count, "files_renamed": 0, "files_copied": 0,
            "files_type_changed": 0, "files_mode_changed": 0, "submodule_changes": 0,
            "binary_files": 0, "lines_added": 0, "lines_deleted": 0,
            "provenance": { "source": "git" }
        });
        serde_json::from_value(json!({
            "comparison": fixtures::comparison_json(),
            "totals": totals,
            "commits": [],
            "files": files,
            "coverage": fixtures::coverage_json(
                u64::try_from(count).unwrap_or(0),
                u64::try_from(count).unwrap_or(0),
                0,
            )
        }))
        .expect("inline inventory")
    }

    fn sized_inventory(files: usize, churn: u64) -> ReviewInventory {
        wrap(
            (0..files)
                .map(|index| file_json(&format!("src/f{index}.rs"), churn, 0))
                .collect(),
        )
    }

    fn renames_inventory() -> ReviewInventory {
        wrap(vec![
            json!({
                "old_path": "src/a_old.rs", "new_path": "src/a_new.rs", "status": "renamed",
                "similarity": 99, "lines_added": 12, "lines_deleted": 8, "binary": false,
                "classification": { "role": "implementation",
                                    "rule_id": "builtin.path.implementation.v1" },
                "provenance": { "source": "git" }
            }),
            json!({
                "old_path": "src/b_old.rs", "new_path": "src/b_new.rs", "status": "renamed",
                "similarity": 91, "lines_added": 13, "lines_deleted": 8, "binary": false,
                "classification": { "role": "implementation",
                                    "rule_id": "builtin.path.implementation.v1" },
                "provenance": { "source": "git" }
            }),
        ])
    }
}
