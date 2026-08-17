//! Pure review model — spec §5 / §6. Derived from inventory + structure only,
//! never from the filters; no GPUI types live here.
// Frozen surface: the wave-1 view units read these fields.
#![allow(dead_code)]

use super::super::review::ReviewFileKey;
use okena_core::review::{FileRole, ReviewFileStatus, ReviewNavigationTarget};
use okena_review::{CallChangeKind, SymbolChangeKind};

/// Everything the review screens read, in one immutable value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReviewModel {
    /// Inventory order — *not* the diff pane's `file_stats` order.
    pub files: Vec<FileEntry>,
    pub root: DirNode,
    pub volume: Vec<VolumeRow>,
    pub total_changed_lines: u64,
    pub facts: Facts,
    pub attention: Vec<AttentionItem>,
    pub status: AnalysisStatus,
    pub omissions: Vec<OmissionRow>,
    pub commits: Vec<CommitRow>,
    pub coverage: CoverageSummary,
    pub small_change: bool,
}

impl ReviewModel {
    pub(crate) fn file_index(&self, key: &ReviewFileKey) -> Option<usize> {
        self.files.iter().position(|entry| &entry.key == key)
    }

    pub(crate) fn attention_index(&self, target: &AttentionTarget) -> Option<usize> {
        self.attention
            .iter()
            .position(|item| &item.target == target)
    }

    pub(crate) fn first_attention_for_file(&self, key: &ReviewFileKey) -> Option<usize> {
        self.attention
            .iter()
            .position(|item| item.target.file() == Some(key))
    }

    /// First file under `dir_path`, in attention order; model order as fallback.
    pub(crate) fn first_file_under(&self, dir_path: &str) -> Option<usize> {
        let under = |index: usize| {
            self.files
                .get(index)
                .is_some_and(|entry| is_under(&entry.display_path, dir_path))
        };
        self.attention
            .iter()
            .filter_map(|item| item.target.file())
            .filter_map(|key| self.file_index(key))
            .find(|index| under(*index))
            .or_else(|| (0..self.files.len()).find(|index| under(*index)))
    }
}

/// Whether `path` sits inside `dir_path` (`dir_path` empty = repository root).
pub(crate) fn is_under(path: &str, dir_path: &str) -> bool {
    if dir_path.is_empty() {
        return true;
    }
    path.strip_prefix(dir_path)
        .is_some_and(|rest| rest.starts_with('/'))
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FileEntry {
    pub key: ReviewFileKey,
    /// `old → new` for renames, otherwise the single path.
    pub display_path: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: ReviewFileStatus,
    pub role: FileRole,
    pub rule_id: String,
    pub similarity: Option<u8>,
    pub lines_added: u64,
    pub lines_deleted: u64,
    pub binary: bool,
    pub analysis: FileAnalysis,
    pub reasons: Vec<Reason>,
    pub tier: Tier,
    /// The whole file is a test file, by its path.
    pub is_test: bool,
    /// Tests changed here: the file is one, or a test scope inside it changed —
    /// in Rust the tests usually live in the file they test.
    pub has_test_changes: bool,
    /// Changed lines that sit inside a test scope of an otherwise
    /// non-test file; counted once, on the outermost test scope.
    pub inline_test_lines: u64,
    pub symbols: Vec<SymbolEntry>,
    /// Index into `ReviewStructure::files` when structure reached this file.
    pub structure_index: Option<usize>,
}

impl FileEntry {
    pub(crate) fn changed_lines(&self) -> u64 {
        self.lines_added.saturating_add(self.lines_deleted)
    }
}

/// How far structure analysis got with one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileAnalysis {
    NotInStructure,
    Parsed { language: String },
    Partial { language: String },
    Pending,
    Unsupported,
    Failed,
    Skipped,
}

impl FileAnalysis {
    pub(crate) fn is_analyzed(&self) -> bool {
        matches!(self, Self::Parsed { .. } | Self::Partial { .. })
    }

    pub(crate) fn language(&self) -> Option<&str> {
        match self {
            Self::Parsed { language } | Self::Partial { language } => Some(language),
            Self::NotInStructure
            | Self::Pending
            | Self::Unsupported
            | Self::Failed
            | Self::Skipped => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolEntry {
    /// Index into the file's `StructuredFile::symbol_changes`.
    pub change_index: usize,
    pub name: String,
    pub qualified: String,
    pub glyph: KindGlyph,
    pub change: SymbolChangeKind,
    pub public: bool,
    /// The symbol is a test scope or sits in one (`mod tests`, `describe`).
    pub in_test_scope: bool,
    /// Normalized `(old, new)` signature pair when the signature changed.
    pub signature: Option<(String, String)>,
    pub body_changed: bool,
    pub lines_added: u32,
    pub lines_deleted: u32,
    pub calls: Vec<CallRow>,
    pub reasons: Vec<Reason>,
    pub tier: Tier,
    pub navigation: ReviewNavigationTarget,
    /// 1-based inclusive line ranges, base side.
    pub old_hunks: Vec<(u32, u32)>,
    /// 1-based inclusive line ranges, head side.
    pub new_hunks: Vec<(u32, u32)>,
    pub metrics: SymbolMetrics,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SymbolMetrics {
    pub lines: Option<u32>,
    pub params: Option<u32>,
    pub depth: Option<u32>,
    pub members: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum KindGlyph {
    Function,
    Method,
    Class,
    Type,
    Module,
    File,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallRow {
    pub change: CallChangeKind,
    pub callee: String,
    pub old_args: Option<String>,
    pub new_args: Option<String>,
    /// Control-context stack, outermost first, already worded — head side,
    /// or base side for a removed call.
    pub context: Vec<String>,
    /// The base-side stack when a modified call moved between contexts.
    pub old_context: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DirNode {
    /// Display name; joined single-child chains keep the whole `a/b/c`.
    pub name: String,
    pub path: String,
    pub children: Vec<DirNode>,
    /// Indices into `ReviewModel::files`, this directory only.
    pub files: Vec<usize>,
    /// Files in the whole subtree.
    pub file_count: usize,
    pub lines_added: u64,
    pub lines_deleted: u64,
    pub is_implementation_dir: bool,
    pub no_test_changes: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VolumeRow {
    pub role: FileRole,
    pub files: usize,
    /// Files that gave this role lines without carrying it — inline tests in
    /// implementation files. They are counted in their own role's `files` too.
    pub inline_files: usize,
    pub lines: u64,
    pub percent: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Tier {
    Contract,
    Behaviour,
    Volume,
    GitFacts,
    #[default]
    Rest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ReasonKind {
    PublicRemoved,
    PublicSignature,
    ExportedSignature,
    Body,
    Calls,
    New,
    NewPublic,
    Removed,
    Moved,
    NoTestChanges,
    CiConfig,
    Lockfile,
    Submodule,
    Binary,
    Complex,
    NotAnalyzed,
    LargeChurn,
    DeletedImpl,
}

/// One measured reason plus its already-worded chip text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reason {
    pub kind: ReasonKind,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AttentionTarget {
    Symbol {
        file: ReviewFileKey,
        change_index: usize,
    },
    File(ReviewFileKey),
    Directory(String),
}

impl AttentionTarget {
    pub(crate) fn file(&self) -> Option<&ReviewFileKey> {
        match self {
            Self::Symbol { file, .. } | Self::File(file) => Some(file),
            Self::Directory(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AttentionItem {
    pub target: AttentionTarget,
    pub tier: Tier,
    pub reasons: Vec<Reason>,
    pub name: String,
    pub path: String,
    pub glyph: KindGlyph,
    pub lines_added: u64,
    pub lines_deleted: u64,
    /// Ranked from git facts only (structure never reached it).
    pub dimmed: bool,
    pub is_test: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Facts {
    pub public_api: Option<PublicApiFact>,
    pub tests: Option<TestsFact>,
    pub moves: Option<MovesFact>,
    pub commits: Option<CommitsFact>,
    pub also: Option<AlsoFact>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublicApiFact {
    pub removed: u64,
    pub signatures: u64,
    pub added: u64,
    /// Coverage is partial, so the counts are lower bounds (`≥`).
    pub lower_bound: bool,
    pub languages: Vec<String>,
    pub no_supported_language: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestsFact {
    pub impl_dirs: usize,
    pub with_tests: usize,
    pub without: Vec<DirRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirRef {
    pub path: String,
    pub files: usize,
    pub lines: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MovesFact {
    pub total: usize,
    pub likely_mechanical: usize,
    pub with_edits: usize,
    pub avg_similarity: u8,
    pub residual_lines: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommitsFact {
    pub count: usize,
    pub merges: usize,
    pub authors: Vec<String>,
    pub span_secs: i64,
    pub first_sha: String,
    pub last_sha: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AlsoFact {
    pub lockfiles: usize,
    pub submodules: usize,
    pub binaries: usize,
    pub deleted_impl: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommitRow {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author: String,
    pub timestamp: i64,
    pub is_merge: bool,
}

/// Header pill state — spec §10.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AnalysisStatus {
    LoadingInventory,
    AnalyzingStructure,
    Ready { files: u64, languages: Vec<String> },
    Limited { analyzed: u64, total: u64 },
    ReadyWithFailures { failed: u64 },
    Unavailable { message: String },
}

/// One omission group, already worded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OmissionRow {
    pub sentence: String,
    pub count: u64,
    pub detail: String,
    pub warn: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CoverageSummary {
    pub analyzed_files: u64,
    pub total_files: u64,
    pub impl_analyzed: usize,
    pub impl_total: usize,
    /// Analysis took files in path order, so the reached subset is biased.
    pub path_order_bias: bool,
    pub languages: Vec<String>,
    pub partial: bool,
    pub failed: u64,
    pub base_oid: String,
    pub head_oid: String,
    pub merge_base_oid: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::is_under;

    #[test]
    fn subtree_membership_requires_a_directory_boundary() {
        assert!(is_under("src/lib.rs", "src"));
        assert!(is_under("src/a/b.rs", "src/a"));
        assert!(!is_under("src2/lib.rs", "src"));
        assert!(!is_under("src", "src"));
        assert!(is_under("anything", ""));
    }
}
