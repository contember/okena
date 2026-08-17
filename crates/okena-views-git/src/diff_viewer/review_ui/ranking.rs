//! Builds the review model from the loaded datasets — spec §5 / §6.

use super::super::review::ReviewFileKey;
use super::labels::{language_label, short_sha};
use super::model::{
    AnalysisStatus, AttentionItem, AttentionTarget, CommitRow, CoverageSummary, DirNode, Facts,
    FileAnalysis, FileEntry, KindGlyph, ReviewModel, Tier, VolumeRow,
};
use super::state::ALL_ROLES;
use okena_core::review::{FileRole, ReviewCoverage, ReviewInventory, ResolvedComparison};
use okena_git::DiffMode;
use okena_review::ReviewStructure;
use std::collections::BTreeMap;

/// Comparisons at or below either bound skip the Overview — spec §12.
const SMALL_CHANGE_FILES: usize = 10;
const SMALL_CHANGE_LINES: u64 = 500;

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
    // wave-0 stub: unit R replaces the body
    let Some(inventory) = inputs.inventory else {
        return empty_model(match inputs.inventory_error {
            Some(error) => AnalysisStatus::Unavailable {
                message: error.to_string(),
            },
            None => AnalysisStatus::LoadingInventory,
        });
    };

    let files: Vec<FileEntry> = inventory.files.iter().map(file_entry).collect();
    let total_changed_lines = files
        .iter()
        .fold(0u64, |total, entry| total.saturating_add(entry.changed_lines()));
    let root = directory_tree(&files);
    let volume = volume_rows(&files, total_changed_lines);
    let attention = attention_items(&files);
    // A single commit target has no commit ledger to show — spec §12.
    let commits = match inputs.diff_mode {
        DiffMode::Commit(_) => Vec::new(),
        _ => commit_rows(inventory),
    };
    let status = analysis_status(inventory, inputs.structure, &inputs.structure_state);
    let coverage = coverage_summary(inventory, inputs.structure, &files);
    let small_change =
        files.len() <= SMALL_CHANGE_FILES || total_changed_lines <= SMALL_CHANGE_LINES;

    ReviewModel {
        files,
        root,
        volume,
        total_changed_lines,
        facts: Facts::default(),
        attention,
        status,
        omissions: Vec::new(),
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

fn file_entry(file: &okena_core::review::ReviewFileFact) -> FileEntry {
    let key = ReviewFileKey::from_inventory(file);
    let role = file.classification.role();
    FileEntry {
        display_path: key.display(),
        key,
        old_path: file.old_path.clone(),
        new_path: file.new_path.clone(),
        status: file.status,
        role,
        rule_id: file.classification.rule_id().as_str().to_string(),
        similarity: file.similarity,
        lines_added: file.lines_added.unwrap_or(0),
        lines_deleted: file.lines_deleted.unwrap_or(0),
        binary: file.binary,
        analysis: FileAnalysis::NotInStructure,
        reasons: Vec::new(),
        tier: Tier::Rest,
        is_test: role == FileRole::Test,
        symbols: Vec::new(),
        structure_index: None,
    }
}

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

fn volume_rows(files: &[FileEntry], total_changed_lines: u64) -> Vec<VolumeRow> {
    ALL_ROLES
        .into_iter()
        .map(|role| {
            let matching = files.iter().filter(|entry| entry.role == role);
            let mut count = 0usize;
            let mut lines = 0u64;
            for entry in matching {
                count += 1;
                lines = lines.saturating_add(entry.changed_lines());
            }
            let percent = if total_changed_lines == 0 {
                0.0
            } else {
                ratio_percent(lines, total_changed_lines)
            };
            VolumeRow {
                role,
                files: count,
                lines,
                percent,
            }
        })
        .collect()
}

/// Share of the total, to one decimal place.
fn ratio_percent(part: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let permille = part.saturating_mul(1_000) / total;
    f32::from(u16::try_from(permille.min(1_000)).unwrap_or(1_000)) / 10.0
}

fn attention_items(files: &[FileEntry]) -> Vec<AttentionItem> {
    let mut order: Vec<&FileEntry> = files.iter().collect();
    order.sort_by(|left, right| {
        right
            .changed_lines()
            .cmp(&left.changed_lines())
            .then_with(|| left.display_path.cmp(&right.display_path))
    });
    order
        .into_iter()
        .map(|entry| AttentionItem {
            target: AttentionTarget::File(entry.key.clone()),
            tier: Tier::Rest,
            reasons: Vec::new(),
            name: basename(&entry.display_path).to_string(),
            path: entry.display_path.clone(),
            glyph: KindGlyph::File,
            lines_added: entry.lines_added,
            lines_deleted: entry.lines_deleted,
            dimmed: !entry.analysis.is_analyzed(),
            is_test: entry.is_test,
        })
        .collect()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn commit_rows(inventory: &ReviewInventory) -> Vec<CommitRow> {
    inventory
        .commits
        .iter()
        .map(|commit| CommitRow {
            sha: commit.oid.as_str().to_string(),
            short_sha: short_sha(commit.oid.as_str()),
            subject: commit.subject.clone(),
            author: commit.author_name.clone(),
            timestamp: commit.timestamp,
            is_merge: commit.parent_oids.len() > 1,
        })
        .collect()
}

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
            if coverage.analyzed_items() < coverage.total_items() {
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

fn coverage_summary(
    inventory: &ReviewInventory,
    structure: Option<&ReviewStructure>,
    files: &[FileEntry],
) -> CoverageSummary {
    let coverage: &ReviewCoverage = structure.map_or(&inventory.coverage, ReviewStructure::coverage);
    let impl_total = files
        .iter()
        .filter(|entry| entry.role == FileRole::Implementation)
        .count();
    let impl_analyzed = files
        .iter()
        .filter(|entry| entry.role == FileRole::Implementation && entry.analysis.is_analyzed())
        .count();
    CoverageSummary {
        analyzed_files: coverage.analyzed_items(),
        total_files: coverage.total_items(),
        impl_analyzed,
        impl_total,
        path_order_bias: false,
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
