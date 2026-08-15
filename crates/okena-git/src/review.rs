//! Exact Git comparison and deterministic review inventory.
//!
//! Friendly refs are resolved once. Every subsequent operation consumes the
//! effective immutable object IDs stored in the resolved comparison.

use std::path::Path;

use okena_core::process::{command, safe_output};
use okena_core::review::{
    ComparisonStrategy, FactProvenance, FileClassification, FileRole, GitObjectId,
    ImmutableResolvedComparison, ResolvedComparison, ReviewChangeTotals, ReviewCommitFact,
    ReviewComparisonId, ReviewCoverage, ReviewDiffRequest, ReviewFileFact, ReviewFileStatus,
    ReviewInventory, ReviewSnapshot, ReviewSourceRequest, ReviewSubmoduleChange,
};
use okena_core::types::DiffMode;
use serde::{Deserialize, Serialize};

use crate::diff::{DiffResult, parse_unified_diff};
use crate::error::{GitError, GitResult};

// Wave 1 records Git facts only; the later classifier replaces this explicit fallback.
const UNCLASSIFIED_RULE_ID: &str = "builtin.unclassified";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSourceContents {
    pub old_content: Option<String>,
    pub new_content: Option<String>,
}

/// Allocation limits for loading the two sides of an exact source request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSourceBudget {
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

impl ReviewSourceBudget {
    pub fn new(max_file_bytes: u64, max_total_bytes: u64) -> GitResult<Self> {
        if max_file_bytes == 0 || max_total_bytes == 0 {
            return Err(GitError::ParseError(
                "source byte limits must be greater than zero".to_string(),
            ));
        }
        Ok(Self {
            max_file_bytes,
            max_total_bytes,
        })
    }
}

/// Resolve an immutable review target to full object IDs exactly once.
pub fn resolve_review_comparison(path: &Path, mode: DiffMode) -> GitResult<ResolvedComparison> {
    match &mode {
        DiffMode::BranchCompare { base, head } => {
            crate::validate_git_ref(base)?;
            crate::validate_git_ref(head)?;
            let requested_base = resolve_commit_oid(path, base)?;
            let requested_head = resolve_commit_oid(path, head)?;
            let merge_base = resolve_merge_base(path, &requested_base, &requested_head)?;

            let (strategy, effective_base, merge_base_oid, identity) = match merge_base {
                Some(merge_base) => (
                    ComparisonStrategy::MergeBaseToHead,
                    merge_base.clone(),
                    Some(merge_base.clone()),
                    ReviewComparisonId(format!(
                        "branch:merge-base:{}:{}:{}",
                        requested_base, requested_head, merge_base
                    )),
                ),
                None => (
                    ComparisonStrategy::DirectBaseToHeadWithoutMergeBase,
                    requested_base.clone(),
                    None,
                    ReviewComparisonId(format!(
                        "branch:direct:{}:{}",
                        requested_base, requested_head
                    )),
                ),
            };

            resolved(
                mode,
                Some(requested_base),
                Some(requested_head.clone()),
                strategy,
                ReviewSnapshot::Commit {
                    oid: effective_base,
                },
                ReviewSnapshot::Commit {
                    oid: requested_head,
                },
                merge_base_oid,
                identity,
            )
        }
        DiffMode::Commit(reference) => {
            crate::validate_git_ref(reference)?;
            let commit = resolve_commit_oid(path, reference)?;
            let parents = commit_parent_oids(path, &commit)?;
            let first_parent = parents.first().cloned();
            match first_parent {
                Some(parent) => resolved(
                    mode,
                    Some(parent.clone()),
                    Some(commit.clone()),
                    ComparisonStrategy::ParentToCommit,
                    ReviewSnapshot::Commit { oid: parent },
                    ReviewSnapshot::Commit {
                        oid: commit.clone(),
                    },
                    None,
                    ReviewComparisonId(format!("commit:parent:{commit}")),
                ),
                None => {
                    let empty_tree = empty_tree_oid(path)?;
                    resolved(
                        mode,
                        None,
                        Some(commit.clone()),
                        ComparisonStrategy::EmptyTreeToCommit,
                        ReviewSnapshot::EmptyTree {
                            oid: empty_tree.clone(),
                        },
                        ReviewSnapshot::Commit {
                            oid: commit.clone(),
                        },
                        None,
                        ReviewComparisonId(format!("commit:root:{empty_tree}:{commit}")),
                    )
                }
            }
        }
        DiffMode::WorkingTree | DiffMode::Staged => Err(GitError::ParseError(
            "mutable review comparison resolution is not implemented".to_string(),
        )),
    }
}

/// Produce a line diff from the immutable effective snapshots.
pub fn get_exact_review_diff(path: &Path, request: &ReviewDiffRequest) -> GitResult<DiffResult> {
    let comparison = request.comparison.as_resolved();
    let (base, head) = immutable_endpoints(comparison)?;
    let mut args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames=50%",
        "--find-copies=50%",
        "--find-copies-harder",
        base.as_str(),
        head.as_str(),
    ];
    if request.ignore_whitespace {
        args.insert(7, "-w");
    }
    let output = run_git(path, &args)?;
    let stdout = String::from_utf8(output)
        .map_err(|error| GitError::ParseError(format!("diff output is not UTF-8: {error}")))?;
    Ok(parse_unified_diff(&stdout))
}

/// Load exact old/new source with distinct paths for additions, deletions, and renames.
pub fn get_exact_review_source(
    path: &Path,
    request: &ReviewSourceRequest,
    budget: ReviewSourceBudget,
) -> GitResult<ReviewSourceContents> {
    let comparison = request.comparison().as_resolved();
    let old = preflight_snapshot_file(path, comparison.base(), request.old_path())?;
    let new = preflight_snapshot_file(path, comparison.head(), request.new_path())?;
    enforce_source_budget(old.as_ref(), new.as_ref(), budget)?;
    let old_content = read_preflight_file(path, old)?;
    let new_content = read_preflight_file(path, new)?;
    Ok(ReviewSourceContents {
        old_content,
        new_content,
    })
}

/// Build deterministic facts over one immutable resolved comparison.
pub fn get_review_inventory(
    path: &Path,
    comparison: &ImmutableResolvedComparison,
) -> GitResult<ReviewInventory> {
    let (base, head) = immutable_endpoints(comparison.as_resolved())?;
    let raw = run_git(
        path,
        &[
            "diff",
            "--raw",
            "-z",
            "--abbrev=64",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames=50%",
            "--find-copies=50%",
            "--find-copies-harder",
            base.as_str(),
            head.as_str(),
        ],
    )?;
    let numstat = run_git(
        path,
        &[
            "diff",
            "--numstat",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames=50%",
            "--find-copies=50%",
            "--find-copies-harder",
            base.as_str(),
            head.as_str(),
        ],
    )?;
    let mut files = parse_raw_diff(&raw)?;
    apply_numstat(&mut files, &parse_numstat(&numstat)?)?;
    let commits = bounded_commit_ledger(path, comparison.as_resolved())?;
    let totals = calculate_totals(&files, commits.len() as u64);
    let coverage = ReviewCoverage::new(files.len() as u64, files.len() as u64, 0, 0, 0, 0, None)
        .map_err(model_error)?;

    Ok(ReviewInventory {
        comparison: comparison.as_resolved().clone(),
        totals,
        commits,
        files,
        coverage,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolved(
    requested: DiffMode,
    requested_base_oid: Option<GitObjectId>,
    requested_head_oid: Option<GitObjectId>,
    strategy: ComparisonStrategy,
    base: ReviewSnapshot,
    head: ReviewSnapshot,
    merge_base_oid: Option<GitObjectId>,
    identity: ReviewComparisonId,
) -> GitResult<ResolvedComparison> {
    ResolvedComparison::new(
        requested,
        requested_base_oid,
        requested_head_oid,
        strategy,
        base,
        head,
        merge_base_oid,
        identity,
    )
    .map_err(model_error)
}

fn model_error(error: impl std::fmt::Display) -> GitError {
    GitError::ParseError(error.to_string())
}

fn run_git(path: &Path, args: &[&str]) -> GitResult<Vec<u8>> {
    let output = safe_output(command("git").arg("-C").arg(path).args(args))?;
    if !output.status.success() {
        return Err(GitError::GitExitError {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output.stdout)
}

fn resolve_commit_oid(path: &Path, reference: &str) -> GitResult<GitObjectId> {
    let revision = format!("{reference}^{{commit}}");
    let output = run_git(path, &["rev-parse", "--verify", &revision])?;
    parse_oid(trim_ascii(&output), "resolved commit")
}

fn resolve_merge_base(
    path: &Path,
    base: &GitObjectId,
    head: &GitObjectId,
) -> GitResult<Option<GitObjectId>> {
    let output = safe_output(command("git").arg("-C").arg(path).args([
        "merge-base",
        base.as_str(),
        head.as_str(),
    ]))?;
    if output.status.success() {
        return parse_oid(trim_ascii(&output.stdout), "merge base").map(Some);
    }
    if output.status.code() == Some(1) && output.stdout.is_empty() {
        return Ok(None);
    }
    Err(GitError::GitExitError {
        status: output.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn commit_parent_oids(path: &Path, commit: &GitObjectId) -> GitResult<Vec<GitObjectId>> {
    let output = run_git(path, &["rev-list", "--parents", "-n", "1", commit.as_str()])?;
    let line = std::str::from_utf8(trim_ascii(&output))
        .map_err(|error| GitError::ParseError(format!("commit parents are not UTF-8: {error}")))?;
    let mut parts = line.split_ascii_whitespace();
    let Some(returned_commit) = parts.next() else {
        return Err(GitError::ParseError(
            "missing commit parent record".to_string(),
        ));
    };
    if returned_commit != commit.as_str() {
        return Err(GitError::ParseError(
            "commit parent record does not match requested commit".to_string(),
        ));
    }
    parts
        .map(|part| parse_oid(part.as_bytes(), "commit parent"))
        .collect()
}

fn empty_tree_oid(path: &Path) -> GitResult<GitObjectId> {
    // The command bus supplies an empty stdin, so this hashes an empty tree
    // without writing an object and works for both SHA-1 and SHA-256 repos.
    let output = run_git(path, &["hash-object", "-t", "tree", "--stdin"])?;
    parse_oid(trim_ascii(&output), "empty tree")
}

fn parse_oid(bytes: &[u8], context: &str) -> GitResult<GitObjectId> {
    let value = std::str::from_utf8(bytes)
        .map_err(|error| GitError::ParseError(format!("{context} is not UTF-8: {error}")))?;
    GitObjectId::new(value.to_string()).map_err(model_error)
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

fn immutable_endpoints(comparison: &ResolvedComparison) -> GitResult<(&GitObjectId, &GitObjectId)> {
    let base = comparison
        .base()
        .oid()
        .ok_or_else(|| GitError::ParseError("comparison base is mutable".to_string()))?;
    let head = comparison
        .head()
        .oid()
        .ok_or_else(|| GitError::ParseError("comparison head is mutable".to_string()))?;
    Ok((base, head))
}

#[derive(Debug)]
struct PreflightBlob {
    object: String,
    size: u64,
}

fn preflight_snapshot_file(
    path: &Path,
    snapshot: &ReviewSnapshot,
    file_path: Option<&str>,
) -> GitResult<Option<PreflightBlob>> {
    let Some(file_path) = file_path else {
        return Ok(None);
    };
    match snapshot {
        ReviewSnapshot::EmptyTree { .. } => Ok(None),
        ReviewSnapshot::Commit { oid } => {
            let object = format!("{}:{file_path}", oid.as_str());
            let output = run_git(path, &["cat-file", "-s", &object])?;
            let size = std::str::from_utf8(trim_ascii(&output))
                .map_err(|error| GitError::ParseError(format!("blob size is not UTF-8: {error}")))?
                .parse::<u64>()
                .map_err(|error| GitError::ParseError(format!("invalid blob size: {error}")))?;
            Ok(Some(PreflightBlob { object, size }))
        }
        ReviewSnapshot::Index { .. } | ReviewSnapshot::WorkingTree { .. } => Err(
            GitError::ParseError("exact source request contains a mutable snapshot".to_string()),
        ),
    }
}

fn enforce_source_budget(
    old: Option<&PreflightBlob>,
    new: Option<&PreflightBlob>,
    budget: ReviewSourceBudget,
) -> GitResult<()> {
    let budget = ReviewSourceBudget::new(budget.max_file_bytes, budget.max_total_bytes)?;
    for blob in [old, new].into_iter().flatten() {
        if blob.size > budget.max_file_bytes {
            return Err(GitError::ParseError(format!(
                "source blob is {} bytes, exceeding the per-file limit of {} bytes",
                blob.size, budget.max_file_bytes
            )));
        }
    }
    let total = old
        .map_or(0, |blob| blob.size)
        .checked_add(new.map_or(0, |blob| blob.size))
        .ok_or_else(|| GitError::ParseError("source byte total overflowed".to_string()))?;
    if total > budget.max_total_bytes {
        return Err(GitError::ParseError(format!(
            "source request is {total} bytes, exceeding the total limit of {} bytes",
            budget.max_total_bytes
        )));
    }
    Ok(())
}

fn read_preflight_file(path: &Path, blob: Option<PreflightBlob>) -> GitResult<Option<String>> {
    let Some(blob) = blob else {
        return Ok(None);
    };
    let bytes = run_git(path, &["cat-file", "blob", &blob.object])?;
    if bytes.len() as u64 != blob.size {
        return Err(GitError::ParseError(format!(
            "blob size changed between preflight and read: expected {}, received {}",
            blob.size,
            bytes.len()
        )));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| GitError::ParseError(format!("source is not UTF-8: {error}")))
}

fn parse_raw_diff(output: &[u8]) -> GitResult<Vec<ReviewFileFact>> {
    let chunks: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    let mut files = Vec::new();
    let mut index = 0;
    while index < chunks.len() && !chunks[index].is_empty() {
        let header = std::str::from_utf8(chunks[index])
            .map_err(|error| GitError::ParseError(format!("raw header is not UTF-8: {error}")))?;
        index += 1;
        let Some(header) = header.strip_prefix(':') else {
            return Err(GitError::ParseError(format!(
                "raw diff record does not start with ':': {header:?}"
            )));
        };
        let fields: Vec<&str> = header.split_ascii_whitespace().collect();
        if fields.len() != 5 {
            return Err(GitError::ParseError(format!(
                "raw diff header has {} fields instead of 5",
                fields.len()
            )));
        }
        let old_mode = fields[0].to_string();
        let new_mode = fields[1].to_string();
        let old_oid = fields[2];
        let new_oid = fields[3];
        let status_token = fields[4];
        let status_code = status_token
            .as_bytes()
            .first()
            .copied()
            .ok_or_else(|| GitError::ParseError("raw status is empty".to_string()))?;
        let first_path = take_path(&chunks, &mut index)?;
        let (old_path, new_path) = match status_code {
            b'A' => (None, Some(first_path)),
            b'D' => (Some(first_path), None),
            b'R' | b'C' => {
                let second_path = take_path(&chunks, &mut index)?;
                (Some(first_path), Some(second_path))
            }
            _ => (Some(first_path.clone()), Some(first_path)),
        };
        let is_submodule = old_mode == "160000" || new_mode == "160000";
        let similarity =
            match status_code {
                b'R' | b'C' => Some(status_token[1..].parse::<u8>().map_err(|error| {
                    GitError::ParseError(format!("invalid similarity: {error}"))
                })?),
                _ => None,
            };
        let status = match status_code {
            b'A' => ReviewFileStatus::Added,
            b'D' => ReviewFileStatus::Deleted,
            b'R' => ReviewFileStatus::Renamed,
            b'C' => ReviewFileStatus::Copied,
            b'T' => ReviewFileStatus::TypeChanged,
            b'U' => ReviewFileStatus::Unmerged,
            b'M' if is_submodule => ReviewFileStatus::SubmoduleChanged,
            b'M' if old_mode != new_mode && old_oid == new_oid => ReviewFileStatus::ModeChanged,
            b'M' => ReviewFileStatus::Modified,
            _ => ReviewFileStatus::Unknown,
        };
        let submodule = if is_submodule {
            Some(ReviewSubmoduleChange {
                old_oid: if old_mode == "160000" {
                    nonzero_oid(old_oid, "old submodule")?
                } else {
                    None
                },
                new_oid: if new_mode == "160000" {
                    nonzero_oid(new_oid, "new submodule")?
                } else {
                    None
                },
                worktree_dirty: false,
            })
        } else {
            None
        };
        files.push(ReviewFileFact {
            old_path,
            new_path,
            status,
            similarity,
            old_mode: mode_or_none(&old_mode),
            new_mode: mode_or_none(&new_mode),
            lines_added: None,
            lines_deleted: None,
            binary: false,
            submodule,
            classification: FileClassification::from_rule(
                FileRole::Unclassified,
                UNCLASSIFIED_RULE_ID,
            )
            .map_err(model_error)?,
            provenance: FactProvenance::Git,
        });
    }
    Ok(files)
}

fn take_path(chunks: &[&[u8]], index: &mut usize) -> GitResult<String> {
    let Some(path) = chunks.get(*index) else {
        return Err(GitError::ParseError("raw diff path is missing".to_string()));
    };
    *index += 1;
    String::from_utf8(path.to_vec())
        .map_err(|error| GitError::ParseError(format!("Git path is not UTF-8: {error}")))
}

fn nonzero_oid(value: &str, context: &str) -> GitResult<Option<GitObjectId>> {
    if value.bytes().all(|byte| byte == b'0') {
        Ok(None)
    } else {
        parse_oid(value.as_bytes(), context).map(Some)
    }
}

fn mode_or_none(mode: &str) -> Option<String> {
    (mode != "000000").then(|| mode.to_string())
}

#[derive(Debug)]
enum NumstatPaths {
    Single(String),
    Pair { old: String, new: String },
}

#[derive(Debug)]
struct NumstatEntry {
    paths: NumstatPaths,
    added: Option<u64>,
    deleted: Option<u64>,
}

fn parse_numstat(output: &[u8]) -> GitResult<Vec<NumstatEntry>> {
    let chunks: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < chunks.len() && !chunks[index].is_empty() {
        let record = chunks[index];
        index += 1;
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let added = fields
            .next()
            .ok_or_else(|| GitError::ParseError("numstat addition is missing".to_string()))?;
        let deleted = fields
            .next()
            .ok_or_else(|| GitError::ParseError("numstat deletion is missing".to_string()))?;
        let path = fields
            .next()
            .ok_or_else(|| GitError::ParseError("numstat path is missing".to_string()))?;
        let paths = if path.is_empty() {
            let old = take_path(&chunks, &mut index)?;
            let new = take_path(&chunks, &mut index)?;
            NumstatPaths::Pair { old, new }
        } else {
            NumstatPaths::Single(String::from_utf8(path.to_vec()).map_err(|error| {
                GitError::ParseError(format!("numstat path is not UTF-8: {error}"))
            })?)
        };
        let binary = added == b"-" && deleted == b"-";
        let (added, deleted) = if binary {
            (None, None)
        } else {
            (Some(parse_count(added)?), Some(parse_count(deleted)?))
        };
        entries.push(NumstatEntry {
            paths,
            added,
            deleted,
        });
    }
    Ok(entries)
}

fn parse_count(value: &[u8]) -> GitResult<u64> {
    let value = std::str::from_utf8(value)
        .map_err(|error| GitError::ParseError(format!("numstat count is not UTF-8: {error}")))?;
    value
        .parse()
        .map_err(|error| GitError::ParseError(format!("invalid numstat count: {error}")))
}

fn apply_numstat(files: &mut [ReviewFileFact], entries: &[NumstatEntry]) -> GitResult<()> {
    let mut matched = vec![false; files.len()];
    for entry in entries {
        let matching: Vec<usize> = files
            .iter()
            .enumerate()
            .filter_map(|(index, file)| numstat_matches(file, &entry.paths).then_some(index))
            .collect();
        if matching.len() != 1 {
            return Err(GitError::ParseError(format!(
                "numstat entry matched {} raw file facts",
                matching.len()
            )));
        }
        let index = matching[0];
        if matched[index] {
            return Err(GitError::ParseError(
                "multiple numstat entries matched one raw file fact".to_string(),
            ));
        }
        matched[index] = true;
        let file = &mut files[index];
        file.lines_added = entry.added;
        file.lines_deleted = entry.deleted;
        file.binary = entry.added.is_none() && entry.deleted.is_none();
    }
    for (index, file) in files.iter_mut().enumerate() {
        if matched[index] {
            continue;
        }
        if file.status == ReviewFileStatus::ModeChanged {
            file.lines_added = Some(0);
            file.lines_deleted = Some(0);
            continue;
        }
        return Err(GitError::ParseError(format!(
            "raw file fact for {:?} has no matching numstat entry",
            file.new_path.as_deref().or(file.old_path.as_deref())
        )));
    }
    Ok(())
}

fn numstat_matches(file: &ReviewFileFact, paths: &NumstatPaths) -> bool {
    match paths {
        NumstatPaths::Single(path) => {
            !matches!(
                file.status,
                ReviewFileStatus::Renamed | ReviewFileStatus::Copied
            ) && file.new_path.as_deref().or(file.old_path.as_deref()) == Some(path.as_str())
        }
        NumstatPaths::Pair { old, new } => {
            file.old_path.as_deref() == Some(old) && file.new_path.as_deref() == Some(new)
        }
    }
}

fn bounded_commit_ledger(
    path: &Path,
    comparison: &ResolvedComparison,
) -> GitResult<Vec<ReviewCommitFact>> {
    let (_, head) = immutable_endpoints(comparison)?;
    let range;
    let (revision, max_count) = match comparison.requested() {
        DiffMode::BranchCompare { .. } => {
            let base = comparison.base().oid().ok_or_else(|| {
                GitError::ParseError("branch comparison base is mutable".to_string())
            })?;
            range = format!("{}..{}", base.as_str(), head.as_str());
            (range.as_str(), None)
        }
        DiffMode::Commit(_) => (head.as_str(), Some("--max-count=1")),
        DiffMode::WorkingTree | DiffMode::Staged => {
            return Err(GitError::ParseError(
                "mutable comparison has no immutable commit ledger".to_string(),
            ));
        }
    };
    let mut args = vec![
        "log",
        "-z",
        "--reverse",
        "--topo-order",
        "--no-decorate",
        "--format=%H%x00%P%x00%s%x00%an%x00%ct",
    ];
    if let Some(max_count) = max_count {
        args.push(max_count);
    }
    args.push(revision);
    let output = run_git(path, &args)?;
    parse_commit_ledger(&output)
}

fn parse_commit_ledger(output: &[u8]) -> GitResult<Vec<ReviewCommitFact>> {
    if output.is_empty() {
        return Ok(Vec::new());
    }
    let mut chunks: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    if chunks.last().is_some_and(|chunk| chunk.is_empty()) {
        chunks.pop();
    }
    if chunks.len() % 5 != 0 {
        return Err(GitError::ParseError(format!(
            "commit ledger has {} fields, not a multiple of 5",
            chunks.len()
        )));
    }
    chunks
        .chunks_exact(5)
        .map(|record| {
            let oid = parse_oid(record[0], "ledger commit")?;
            let parents = std::str::from_utf8(record[1])
                .map_err(|error| {
                    GitError::ParseError(format!("ledger parents are not UTF-8: {error}"))
                })?
                .split_ascii_whitespace()
                .map(|parent| parse_oid(parent.as_bytes(), "ledger parent"))
                .collect::<GitResult<Vec<_>>>()?;
            let subject = String::from_utf8(record[2].to_vec()).map_err(|error| {
                GitError::ParseError(format!("commit subject is not UTF-8: {error}"))
            })?;
            let author_name = String::from_utf8(record[3].to_vec()).map_err(|error| {
                GitError::ParseError(format!("commit author is not UTF-8: {error}"))
            })?;
            let timestamp = std::str::from_utf8(record[4])
                .map_err(|error| {
                    GitError::ParseError(format!("commit timestamp is not UTF-8: {error}"))
                })?
                .parse::<i64>()
                .map_err(|error| {
                    GitError::ParseError(format!("invalid commit timestamp: {error}"))
                })?;
            Ok(ReviewCommitFact {
                oid,
                parent_oids: parents,
                subject,
                author_name,
                timestamp,
                provenance: FactProvenance::Git,
            })
        })
        .collect()
}

fn calculate_totals(files: &[ReviewFileFact], commits: u64) -> ReviewChangeTotals {
    let mut totals = ReviewChangeTotals {
        commits,
        files: files.len() as u64,
        files_added: 0,
        files_deleted: 0,
        files_modified: 0,
        files_renamed: 0,
        files_copied: 0,
        files_type_changed: 0,
        files_mode_changed: 0,
        submodule_changes: 0,
        binary_files: 0,
        lines_added: 0,
        lines_deleted: 0,
        provenance: FactProvenance::Git,
    };
    for file in files {
        match file.status {
            ReviewFileStatus::Added => totals.files_added += 1,
            ReviewFileStatus::Deleted => totals.files_deleted += 1,
            ReviewFileStatus::Modified => totals.files_modified += 1,
            ReviewFileStatus::Renamed => totals.files_renamed += 1,
            ReviewFileStatus::Copied => totals.files_copied += 1,
            ReviewFileStatus::TypeChanged => totals.files_type_changed += 1,
            ReviewFileStatus::ModeChanged => totals.files_mode_changed += 1,
            ReviewFileStatus::SubmoduleChanged
            | ReviewFileStatus::Unmerged
            | ReviewFileStatus::Unknown => {}
        }
        if file.submodule.is_some() {
            totals.submodule_changes += 1;
        }
        if file.binary {
            totals.binary_files += 1;
        }
        totals.lines_added += file.lines_added.unwrap_or(0);
        totals.lines_deleted += file.lines_deleted.unwrap_or(0);
    }
    totals
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::process::Command;

    use okena_core::review::{ComparisonStrategy, ImmutableResolvedComparison};

    use super::*;
    use crate::repository::test_support::{git_in, init_temp_repo};

    fn immutable(comparison: ResolvedComparison) -> ImmutableResolvedComparison {
        comparison.try_into().unwrap()
    }

    fn source_budget() -> ReviewSourceBudget {
        ReviewSourceBudget::new(1024 * 1024, 2 * 1024 * 1024).unwrap()
    }

    fn commit_at(repo: &Path, subject: &str, timestamp: &str) {
        let output = Command::new("git")
            .args(["-c", "commit.gpgsign=false", "commit", "-m", subject])
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_AUTHOR_DATE", timestamp)
            .env("GIT_COMMITTER_DATE", timestamp)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "dated commit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn file<'a>(inventory: &'a ReviewInventory, path: &str) -> &'a ReviewFileFact {
        inventory
            .files
            .iter()
            .find(|file| {
                file.new_path.as_deref() == Some(path) || file.old_path.as_deref() == Some(path)
            })
            .unwrap_or_else(|| panic!("missing file fact for {path:?}"))
    }

    #[test]
    fn branch_resolution_freezes_refs_and_bounds_commit_ledger() {
        let (_tmp, repo) = init_temp_repo();
        let fork = resolve_commit_oid(&repo, "main").unwrap();
        git_in(&repo, &["checkout", "-b", "feature"]);
        fs::write(repo.join("file.txt"), "feature one\n").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "feature one"],
        );
        fs::write(repo.join("file.txt"), "feature two\n").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "feature two"],
        );
        let frozen_head = resolve_commit_oid(&repo, "feature").unwrap();

        git_in(&repo, &["checkout", "main"]);
        fs::write(repo.join("main-only.txt"), "main\n").unwrap();
        git_in(&repo, &["add", "main-only.txt"]);
        git_in(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "main advanced",
            ],
        );
        let requested_base = resolve_commit_oid(&repo, "main").unwrap();

        let comparison = resolve_review_comparison(
            &repo,
            DiffMode::BranchCompare {
                base: "main".to_string(),
                head: "feature".to_string(),
            },
        )
        .unwrap();
        assert_eq!(comparison.strategy(), ComparisonStrategy::MergeBaseToHead);
        assert_eq!(comparison.requested_base_oid(), Some(&requested_base));
        assert_eq!(comparison.requested_head_oid(), Some(&frozen_head));
        assert_eq!(comparison.merge_base_oid(), Some(&fork));
        assert_eq!(comparison.base().oid(), Some(&fork));

        let request = ReviewDiffRequest::new(comparison.clone(), false).unwrap();
        let before = get_exact_review_diff(&repo, &request).unwrap();
        let source_request = ReviewSourceRequest::new(
            comparison.clone(),
            Some("file.txt".to_string()),
            Some("file.txt".to_string()),
        )
        .unwrap();
        let source_before =
            get_exact_review_source(&repo, &source_request, source_budget()).unwrap();

        git_in(&repo, &["branch", "-f", "feature", "main"]);
        assert_eq!(
            serde_json::to_value(get_exact_review_diff(&repo, &request).unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert_eq!(
            get_exact_review_source(&repo, &source_request, source_budget()).unwrap(),
            source_before
        );
        assert_eq!(source_before.old_content.as_deref(), Some("x"));
        assert_eq!(source_before.new_content.as_deref(), Some("feature two\n"));

        let inventory = get_review_inventory(&repo, &immutable(comparison)).unwrap();
        assert_eq!(inventory.commits.len(), 2);
        assert_eq!(inventory.commits[0].subject, "feature one");
        assert_eq!(inventory.commits[1].subject, "feature two");
        assert!(inventory.commits.iter().all(|commit| {
            matches!(commit.oid.as_str().len(), 40 | 64)
                && commit
                    .parent_oids
                    .iter()
                    .all(|parent| matches!(parent.as_str().len(), 40 | 64))
        }));
    }

    #[test]
    fn skewed_diamond_ledger_keeps_every_parent_before_its_child() {
        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["checkout", "-b", "left"]);
        fs::write(repo.join("left-one.txt"), "left one\n").unwrap();
        git_in(&repo, &["add", "left-one.txt"]);
        commit_at(&repo, "left one", "2040-01-01T00:00:00Z");

        git_in(&repo, &["checkout", "-b", "right", "main"]);
        fs::write(repo.join("right-one.txt"), "right one\n").unwrap();
        git_in(&repo, &["add", "right-one.txt"]);
        commit_at(&repo, "right one", "1990-01-01T00:00:00Z");
        fs::write(repo.join("right-two.txt"), "right two\n").unwrap();
        git_in(&repo, &["add", "right-two.txt"]);
        commit_at(&repo, "right two", "2060-01-01T00:00:00Z");

        git_in(&repo, &["checkout", "left"]);
        fs::write(repo.join("left-two.txt"), "left two\n").unwrap();
        git_in(&repo, &["add", "left-two.txt"]);
        commit_at(&repo, "left two", "1980-01-01T00:00:00Z");
        git_in(&repo, &["merge", "--no-ff", "--no-commit", "right"]);
        commit_at(&repo, "merge right", "1970-01-01T00:00:00Z");

        let comparison = resolve_review_comparison(
            &repo,
            DiffMode::BranchCompare {
                base: "main".to_string(),
                head: "left".to_string(),
            },
        )
        .unwrap();
        let inventory = get_review_inventory(&repo, &immutable(comparison)).unwrap();
        assert_eq!(inventory.commits.len(), 5);

        let positions: HashMap<_, _> = inventory
            .commits
            .iter()
            .enumerate()
            .map(|(index, commit)| (commit.oid.clone(), index))
            .collect();
        for (child_index, commit) in inventory.commits.iter().enumerate() {
            for parent in &commit.parent_oids {
                if let Some(parent_index) = positions.get(parent) {
                    assert!(
                        parent_index < &child_index,
                        "parent {parent} followed child {}",
                        commit.oid
                    );
                }
            }
        }
    }

    #[test]
    fn unrelated_histories_use_explicit_direct_strategy() {
        let (_tmp, repo) = init_temp_repo();
        let main = resolve_commit_oid(&repo, "main").unwrap();
        git_in(&repo, &["checkout", "--orphan", "other"]);
        git_in(&repo, &["rm", "-f", "file.txt"]);
        fs::write(repo.join("other.txt"), "other\n").unwrap();
        git_in(&repo, &["add", "other.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "other root"],
        );
        let other = resolve_commit_oid(&repo, "other").unwrap();

        let comparison = resolve_review_comparison(
            &repo,
            DiffMode::BranchCompare {
                base: "main".to_string(),
                head: "other".to_string(),
            },
        )
        .unwrap();
        assert_eq!(
            comparison.strategy(),
            ComparisonStrategy::DirectBaseToHeadWithoutMergeBase
        );
        assert_eq!(comparison.base().oid(), Some(&main));
        assert_eq!(comparison.head().oid(), Some(&other));
        assert_eq!(comparison.merge_base_oid(), None);
    }

    #[test]
    fn root_commit_uses_empty_tree_and_has_one_ledger_entry() {
        let (_tmp, repo) = init_temp_repo();
        let root = resolve_commit_oid(&repo, "main").unwrap();
        let comparison =
            resolve_review_comparison(&repo, DiffMode::Commit(root.to_string())).unwrap();
        assert_eq!(comparison.strategy(), ComparisonStrategy::EmptyTreeToCommit);
        assert!(matches!(
            comparison.base(),
            ReviewSnapshot::EmptyTree { .. }
        ));

        let diff = get_exact_review_diff(
            &repo,
            &ReviewDiffRequest::new(comparison.clone(), false).unwrap(),
        )
        .unwrap();
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].new_path.as_deref(), Some("file.txt"));

        let inventory = get_review_inventory(&repo, &immutable(comparison.clone())).unwrap();
        assert_eq!(inventory.commits.len(), 1);
        assert_eq!(inventory.commits[0].oid, root);
        assert!(inventory.commits[0].parent_oids.is_empty());

        let source = get_exact_review_source(
            &repo,
            &ReviewSourceRequest::new(comparison, None, Some("file.txt".to_string())).unwrap(),
            source_budget(),
        )
        .unwrap();
        assert_eq!(source.old_content, None);
        assert_eq!(source.new_content.as_deref(), Some("x"));
    }

    #[test]
    fn commit_mode_uses_first_parent_and_only_selected_commit_in_ledger() {
        let (_tmp, repo) = init_temp_repo();
        let parent = resolve_commit_oid(&repo, "HEAD").unwrap();
        fs::write(repo.join("file.txt"), "next\n").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        git_in(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "selected commit",
            ],
        );
        let head = resolve_commit_oid(&repo, "HEAD").unwrap();

        let comparison =
            resolve_review_comparison(&repo, DiffMode::Commit("HEAD".to_string())).unwrap();
        assert_eq!(comparison.strategy(), ComparisonStrategy::ParentToCommit);
        assert_eq!(comparison.base().oid(), Some(&parent));
        assert_eq!(comparison.head().oid(), Some(&head));

        let inventory = get_review_inventory(&repo, &immutable(comparison)).unwrap();
        assert_eq!(inventory.commits.len(), 1);
        assert_eq!(inventory.commits[0].oid, head);
        assert_eq!(inventory.commits[0].parent_oids, vec![parent]);
    }

    #[test]
    fn exact_source_preflights_per_file_and_total_byte_limits() {
        let (_tmp, repo) = init_temp_repo();
        fs::write(repo.join("file.txt"), "next").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "next"],
        );
        let comparison =
            resolve_review_comparison(&repo, DiffMode::Commit("HEAD".to_string())).unwrap();
        let request = ReviewSourceRequest::new(
            comparison,
            Some("file.txt".to_string()),
            Some("file.txt".to_string()),
        )
        .unwrap();

        let per_file =
            get_exact_review_source(&repo, &request, ReviewSourceBudget::new(3, 16).unwrap())
                .unwrap_err();
        assert!(per_file.to_string().contains("per-file limit"));

        let total =
            get_exact_review_source(&repo, &request, ReviewSourceBudget::new(4, 4).unwrap())
                .unwrap_err();
        assert!(total.to_string().contains("total limit"));
        assert!(ReviewSourceBudget::new(0, 1).is_err());
    }

    #[test]
    fn inventory_reports_file_shapes_and_exact_source_paths() {
        let (_tmp, repo) = init_temp_repo();
        let original = (1..=30)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        fs::write(repo.join("old.rs"), &original).unwrap();
        fs::write(repo.join("deleted.txt"), "deleted\n").unwrap();
        fs::write(repo.join("binary.bin"), [0, 1, 2, 3]).unwrap();
        fs::write(repo.join("mode.sh"), "echo ok\n").unwrap();
        fs::write(repo.join("copy-source.txt"), &original).unwrap();
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "seed review files",
            ],
        );
        git_in(&repo, &["checkout", "-b", "feature"]);

        git_in(&repo, &["mv", "old.rs", "new.rs"]);
        let changed = original.replace("line 15\n", "line fifteen changed\n");
        fs::write(repo.join("new.rs"), &changed).unwrap();
        fs::remove_file(repo.join("deleted.txt")).unwrap();
        fs::write(repo.join("added.txt"), "added\n").unwrap();
        fs::write(repo.join("binary.bin"), [0, 1, 2, 4]).unwrap();
        fs::copy(repo.join("copy-source.txt"), repo.join("copied.txt")).unwrap();
        fs::write(repo.join("odd name [é].txt"), "odd\n").unwrap();
        #[cfg(unix)]
        fs::write(repo.join("tab\tand\nnewline.txt"), "control path\n").unwrap();
        git_in(&repo, &["add", "-A"]);
        git_in(&repo, &["update-index", "--chmod=+x", "mode.sh"]);
        git_in(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "mixed file changes",
            ],
        );

        let comparison = resolve_review_comparison(
            &repo,
            DiffMode::BranchCompare {
                base: "main".to_string(),
                head: "feature".to_string(),
            },
        )
        .unwrap();
        let inventory = get_review_inventory(&repo, &immutable(comparison.clone())).unwrap();

        let renamed = file(&inventory, "new.rs");
        assert_eq!(renamed.status, ReviewFileStatus::Renamed);
        assert_eq!(renamed.old_path.as_deref(), Some("old.rs"));
        assert!(
            renamed
                .similarity
                .is_some_and(|similarity| similarity < 100)
        );
        assert_eq!(
            file(&inventory, "deleted.txt").status,
            ReviewFileStatus::Deleted
        );
        assert_eq!(
            file(&inventory, "added.txt").status,
            ReviewFileStatus::Added
        );
        assert_eq!(
            file(&inventory, "copied.txt").status,
            ReviewFileStatus::Copied
        );
        assert_eq!(
            file(&inventory, "mode.sh").status,
            ReviewFileStatus::ModeChanged
        );
        let binary = file(&inventory, "binary.bin");
        assert!(binary.binary);
        assert_eq!(binary.lines_added, None);
        assert_eq!(binary.lines_deleted, None);
        assert_eq!(
            file(&inventory, "odd name [é].txt").new_path.as_deref(),
            Some("odd name [é].txt")
        );
        #[cfg(unix)]
        assert_eq!(
            file(&inventory, "tab\tand\nnewline.txt")
                .new_path
                .as_deref(),
            Some("tab\tand\nnewline.txt")
        );
        assert!(
            inventory
                .files
                .iter()
                .all(|file| file.classification.role() == FileRole::Unclassified)
        );

        let renamed_source = get_exact_review_source(
            &repo,
            &ReviewSourceRequest::new(
                comparison.clone(),
                Some("old.rs".to_string()),
                Some("new.rs".to_string()),
            )
            .unwrap(),
            source_budget(),
        )
        .unwrap();
        assert_eq!(
            renamed_source.old_content.as_deref(),
            Some(original.as_str())
        );
        assert_eq!(
            renamed_source.new_content.as_deref(),
            Some(changed.as_str())
        );

        let addition = get_exact_review_source(
            &repo,
            &ReviewSourceRequest::new(comparison.clone(), None, Some("added.txt".to_string()))
                .unwrap(),
            source_budget(),
        )
        .unwrap();
        assert_eq!(addition.old_content, None);
        assert_eq!(addition.new_content.as_deref(), Some("added\n"));

        let deletion = get_exact_review_source(
            &repo,
            &ReviewSourceRequest::new(comparison, Some("deleted.txt".to_string()), None).unwrap(),
            source_budget(),
        )
        .unwrap();
        assert_eq!(deletion.old_content.as_deref(), Some("deleted\n"));
        assert_eq!(deletion.new_content, None);
    }

    #[cfg(unix)]
    #[test]
    fn exact_patch_and_inventory_agree_on_c_quoted_rename_paths() {
        let (_tmp, repo) = init_temp_repo();
        let old_path = "old é\t\"quoted.rs";
        let new_path = "new é\n\"quoted.rs";
        let original = (1..=40)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        fs::write(repo.join(old_path), &original).unwrap();
        git_in(&repo, &["add", old_path]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "odd path"],
        );
        git_in(&repo, &["checkout", "-b", "feature"]);
        git_in(&repo, &["mv", old_path, new_path]);
        let changed = original.replace("line 20\n", "line twenty changed\n");
        fs::write(repo.join(new_path), changed).unwrap();
        git_in(&repo, &["add", "-A"]);
        git_in(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "rename odd path",
            ],
        );

        let comparison = resolve_review_comparison(
            &repo,
            DiffMode::BranchCompare {
                base: "main".to_string(),
                head: "feature".to_string(),
            },
        )
        .unwrap();
        let inventory = get_review_inventory(&repo, &immutable(comparison.clone())).unwrap();
        let inventory_file = file(&inventory, new_path);
        assert_eq!(inventory_file.old_path.as_deref(), Some(old_path));
        assert_eq!(inventory_file.new_path.as_deref(), Some(new_path));

        let diff =
            get_exact_review_diff(&repo, &ReviewDiffRequest::new(comparison, false).unwrap())
                .unwrap();
        let patch_file = diff
            .files
            .iter()
            .find(|file| file.new_path.as_deref() == Some(new_path))
            .unwrap_or_else(|| {
                panic!(
                    "quoted rename exists in exact line diff: {:?}",
                    diff.files
                        .iter()
                        .map(|file| (&file.old_path, &file.new_path))
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(patch_file.old_path, inventory_file.old_path);
        assert_eq!(patch_file.new_path, inventory_file.new_path);
        assert!(patch_file.hunks.iter().any(|hunk| !hunk.lines.is_empty()));
    }

    #[test]
    fn raw_submodule_record_preserves_commit_oids() {
        let old = "1".repeat(40);
        let new = "2".repeat(40);
        let raw = format!(":160000 160000 {old} {new} M\0vendor/lib\0");
        let files = parse_raw_diff(raw.as_bytes()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, ReviewFileStatus::SubmoduleChanged);
        let submodule = files[0].submodule.as_ref().unwrap();
        assert_eq!(submodule.old_oid.as_ref().unwrap().as_str(), old);
        assert_eq!(submodule.new_oid.as_ref().unwrap().as_str(), new);
    }

    #[test]
    fn raw_submodule_transition_only_populates_the_submodule_side() {
        let regular = "1".repeat(40);
        let submodule = "2".repeat(40);
        let to_submodule = format!(":100644 160000 {regular} {submodule} T\0vendor/lib\0");
        let files = parse_raw_diff(to_submodule.as_bytes()).unwrap();
        let change = files[0].submodule.as_ref().unwrap();
        assert_eq!(change.old_oid, None);
        assert_eq!(change.new_oid.as_ref().unwrap().as_str(), submodule);

        let to_regular = format!(":160000 100644 {submodule} {regular} T\0vendor/lib\0");
        let files = parse_raw_diff(to_regular.as_bytes()).unwrap();
        let change = files[0].submodule.as_ref().unwrap();
        assert_eq!(change.old_oid.as_ref().unwrap().as_str(), submodule);
        assert_eq!(change.new_oid, None);
    }

    #[test]
    fn numstat_correspondence_rejects_missing_duplicate_and_unmatched_records() {
        let old = "1".repeat(40);
        let new = "2".repeat(40);
        let raw = format!(":100644 100644 {old} {new} M\0file.txt\0");

        let mut missing = parse_raw_diff(raw.as_bytes()).unwrap();
        assert!(apply_numstat(&mut missing, &[]).is_err());

        let entries = parse_numstat(b"1\t1\tfile.txt\x001\t1\tfile.txt\0").unwrap();
        let mut duplicate = parse_raw_diff(raw.as_bytes()).unwrap();
        assert!(apply_numstat(&mut duplicate, &entries).is_err());

        let entries = parse_numstat(b"1\t1\tother.txt\0").unwrap();
        let mut unmatched = parse_raw_diff(raw.as_bytes()).unwrap();
        assert!(apply_numstat(&mut unmatched, &entries).is_err());
    }
}
