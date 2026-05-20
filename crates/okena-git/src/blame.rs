//! Per-line git blame for a single file.
//!
//! Uses `gix-blame` (the gitoxide blame implementation) to attribute each line
//! to the commit that introduced it. Working-tree modifications are detected
//! by diffing the live file against the HEAD blob: lines that don't match
//! anything in HEAD are reported as [`BlameKind::Uncommitted`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use gix::bstr::BStr;
use gix::ObjectId;

/// Whether a line is attributable to a committed change or is a working-tree
/// modification that has no commit yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlameKind {
    Committed,
    Uncommitted,
}

/// Commit metadata attached to a blame line. Shared via `Arc` because many
/// consecutive lines typically reference the same commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameCommit {
    /// Full SHA-1 (40 hex chars). `"0".repeat(40)` for the uncommitted sentinel.
    pub hash: String,
    /// First 7 chars of `hash`.
    pub short_hash: String,
    pub author: String,
    pub author_email: String,
    /// Unix epoch seconds (author time).
    pub timestamp: i64,
    /// First line of the commit message.
    pub summary: String,
}

impl BlameCommit {
    /// Sentinel commit used for [`BlameKind::Uncommitted`] lines.
    fn uncommitted() -> Self {
        Self {
            hash: "0".repeat(40),
            short_hash: "0000000".to_string(),
            author: String::new(),
            author_email: String::new(),
            timestamp: 0,
            summary: String::new(),
        }
    }
}

/// Per-line blame attribution.
#[derive(Clone, Debug)]
pub struct BlameLine {
    /// 1-based line number in the working-tree file.
    pub line_number: usize,
    pub commit: Arc<BlameCommit>,
    pub kind: BlameKind,
}

#[derive(Debug, thiserror::Error)]
pub enum BlameError {
    #[error("path is not inside a git repository")]
    NotGitRepo,
    #[error("file is not tracked in HEAD")]
    NotTracked,
    #[error("repository has no commits")]
    NoCommits,
    #[error("blame failed: {0}")]
    Backend(String),
    #[error("io error: {0}")]
    Io(String),
}

/// Blame `relative_path` (project-relative) inside the repo at `repo_path`.
/// Returns one `BlameLine` per working-tree line, ordered top-to-bottom.
///
/// Uncommitted (working-tree-only) lines get a sentinel `BlameCommit` and
/// [`BlameKind::Uncommitted`]. Lines that survive from HEAD point to the
/// commit that introduced them via gix-blame's rename-aware history walk.
pub fn get_blame(repo_path: &Path, relative_path: &str) -> Result<Vec<BlameLine>, BlameError> {
    let repo = crate::gix_helpers::open(repo_path).ok_or(BlameError::NotGitRepo)?;
    let workdir = repo.workdir().ok_or(BlameError::NotGitRepo)?.to_path_buf();

    let head_id = repo
        .head_id()
        .map_err(|_| BlameError::NoCommits)?
        .detach();
    let head_commit = repo
        .head_commit()
        .map_err(|e| BlameError::Backend(e.to_string()))?;

    // Confirm the path exists in HEAD's tree — otherwise blame will surface
    // a confusing error and the caller can show "not tracked" up front.
    let tree = head_commit
        .tree()
        .map_err(|e| BlameError::Backend(e.to_string()))?;
    let entry = tree
        .lookup_entry_by_path(relative_path)
        .map_err(|e| BlameError::Backend(e.to_string()))?;
    let in_head = entry.is_some();

    if !in_head {
        // File is untracked or freshly added — every working-tree line is
        // uncommitted. Read the WT contents and mark them all.
        return Ok(all_uncommitted_from_wt(&workdir.join(relative_path)));
    }

    let path_bstr: &BStr = BStr::new(relative_path.as_bytes());
    let outcome = repo
        .blame_file(
            path_bstr,
            head_id,
            gix::repository::blame_file::Options {
                rewrites: Some(gix::diff::Rewrites {
                    copies: None,
                    percentage: Some(0.5),
                    limit: 1000,
                    track_empty: false,
                }),
                ..Default::default()
            },
        )
        .map_err(|e| BlameError::Backend(e.to_string()))?;

    // Cache one `Arc<BlameCommit>` per unique commit referenced by the blame.
    let mut commit_cache: HashMap<ObjectId, Arc<BlameCommit>> = HashMap::new();
    for entry in &outcome.entries {
        if let std::collections::hash_map::Entry::Vacant(e) = commit_cache.entry(entry.commit_id) {
            let meta = load_commit_meta(&repo, entry.commit_id)?;
            e.insert(Arc::new(meta));
        }
    }

    // Expand the per-hunk entries into a per-line blame array for HEAD.
    let head_line_count = count_lines(&outcome.blob);
    let mut head_blame: Vec<Arc<BlameCommit>> = Vec::with_capacity(head_line_count);
    for entry in &outcome.entries {
        // Every commit_id was inserted into commit_cache by the loop above.
        #[allow(clippy::expect_used)]
        let commit = commit_cache
            .get(&entry.commit_id)
            .expect("populated above")
            .clone();
        for _ in 0..entry.len.get() {
            head_blame.push(commit.clone());
        }
    }

    // Compare against working-tree contents. If they match, every line maps
    // 1-to-1 to head_blame. Otherwise diff to figure out which WT lines are
    // unchanged (use blame) versus newly inserted (mark uncommitted).
    let wt_path = workdir.join(relative_path);
    let wt_content = std::fs::read(&wt_path).ok();

    let uncommitted = Arc::new(BlameCommit::uncommitted());

    let lines = match wt_content {
        Some(wt) if wt == outcome.blob => head_blame
            .into_iter()
            .enumerate()
            .map(|(i, commit)| BlameLine {
                line_number: i + 1,
                commit,
                kind: BlameKind::Committed,
            })
            .collect(),
        Some(wt) => {
            let mapping = map_wt_lines_to_head(&outcome.blob, &wt);
            mapping
                .into_iter()
                .enumerate()
                .map(|(i, head_idx)| match head_idx {
                    Some(h) if (h as usize) < head_blame.len() => BlameLine {
                        line_number: i + 1,
                        commit: head_blame[h as usize].clone(),
                        kind: BlameKind::Committed,
                    },
                    _ => BlameLine {
                        line_number: i + 1,
                        commit: uncommitted.clone(),
                        kind: BlameKind::Uncommitted,
                    },
                })
                .collect()
        }
        None => head_blame
            .into_iter()
            .enumerate()
            .map(|(i, commit)| BlameLine {
                line_number: i + 1,
                commit,
                kind: BlameKind::Committed,
            })
            .collect(),
    };

    Ok(lines)
}

fn load_commit_meta(repo: &gix::Repository, id: ObjectId) -> Result<BlameCommit, BlameError> {
    let commit = repo
        .find_commit(id)
        .map_err(|e| BlameError::Backend(e.to_string()))?;
    let author = commit
        .author()
        .map_err(|e| BlameError::Backend(e.to_string()))?;
    let message = commit
        .message()
        .map_err(|e| BlameError::Backend(e.to_string()))?;

    let hash = id.to_hex().to_string();
    let short_hash = hash.chars().take(7).collect();
    let summary = message.summary().to_string();

    Ok(BlameCommit {
        hash,
        short_hash,
        author: author.name.to_string(),
        author_email: author.email.to_string(),
        timestamp: author.seconds(),
        summary,
    })
}

fn all_uncommitted_from_wt(wt_path: &Path) -> Vec<BlameLine> {
    let content = std::fs::read(wt_path).unwrap_or_default();
    let count = count_lines(&content);
    let uncommitted = Arc::new(BlameCommit::uncommitted());
    (0..count)
        .map(|i| BlameLine {
            line_number: i + 1,
            commit: uncommitted.clone(),
            kind: BlameKind::Uncommitted,
        })
        .collect()
}

/// Count lines the same way the diff tokenizer does: one per `\n`, plus one
/// trailing if the content doesn't end with `\n` and is non-empty.
fn count_lines(content: &[u8]) -> usize {
    if content.is_empty() {
        return 0;
    }
    let nl = content.iter().filter(|&&b| b == b'\n').count();
    if content.last() == Some(&b'\n') {
        nl
    } else {
        nl + 1
    }
}

/// For each line in `wt`, return the corresponding line index in `head` if the
/// line is unchanged, or `None` if the line was inserted/modified in the
/// working tree. Uses imara-diff (already pulled by the gix `blame` feature).
fn map_wt_lines_to_head(head: &[u8], wt: &[u8]) -> Vec<Option<u32>> {
    use gix::diff::blob::{sources::byte_lines, Algorithm, Diff, InternedInput};

    let input = InternedInput::new(byte_lines(head), byte_lines(wt));
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);

    let wt_line_count = input.after.len() as u32;
    let mut mapping = Vec::with_capacity(wt_line_count as usize);
    let mut head_idx: u32 = 0;
    let mut wt_idx: u32 = 0;

    for hunk in diff.hunks() {
        // Lines before this hunk are unchanged — map 1-to-1.
        while wt_idx < hunk.after.start {
            mapping.push(Some(head_idx));
            head_idx += 1;
            wt_idx += 1;
        }
        // Lines inside this hunk's `after` range are newly inserted or
        // replaced — no blame from HEAD.
        while wt_idx < hunk.after.end {
            mapping.push(None);
            wt_idx += 1;
        }
        // Advance `head_idx` past the `before` range (deletions/replacements).
        head_idx = hunk.before.end;
    }
    // Trailing unchanged lines.
    while wt_idx < wt_line_count {
        mapping.push(Some(head_idx));
        head_idx += 1;
        wt_idx += 1;
    }

    mapping
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    /// Spin up a real on-disk git repo so we exercise both gix-blame and the
    /// working-tree diff path end-to-end.
    fn init_repo(dir: &Path) {
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git command");
        };
        run(&["init", "--initial-branch=main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test User"]);
        run(&["config", "commit.gpgsign", "false"]);
    }

    fn commit_file(dir: &Path, file: &str, content: &str, msg: &str) {
        fs::write(dir.join(file), content).unwrap();
        Command::new("git").args(["add", file]).current_dir(dir).output().unwrap();
        Command::new("git").args(["commit", "-m", msg]).current_dir(dir).output().unwrap();
    }

    #[test]
    fn count_lines_basic() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a"), 1);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a\nb"), 2);
        assert_eq!(count_lines(b"a\nb\n"), 2);
        assert_eq!(count_lines(b"a\nb\nc\n"), 3);
    }

    #[test]
    fn map_unchanged_file_is_identity() {
        let mapping = map_wt_lines_to_head(b"a\nb\nc\n", b"a\nb\nc\n");
        assert_eq!(mapping, vec![Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn map_inserted_lines_are_none() {
        // HEAD: a, b, c
        // WT:   a, X, b, c   (X inserted at idx 1)
        let mapping = map_wt_lines_to_head(b"a\nb\nc\n", b"a\nX\nb\nc\n");
        assert_eq!(mapping, vec![Some(0), None, Some(1), Some(2)]);
    }

    #[test]
    fn map_replaced_line_is_none() {
        // HEAD: a, b, c
        // WT:   a, Y, c    (b replaced with Y)
        let mapping = map_wt_lines_to_head(b"a\nb\nc\n", b"a\nY\nc\n");
        assert_eq!(mapping, vec![Some(0), None, Some(2)]);
    }

    #[test]
    fn map_appended_lines_are_none() {
        let mapping = map_wt_lines_to_head(b"a\nb\n", b"a\nb\nc\nd\n");
        assert_eq!(mapping, vec![Some(0), Some(1), None, None]);
    }

    #[test]
    fn blame_clean_file_attributes_all_to_single_commit() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        commit_file(tmp.path(), "a.txt", "one\ntwo\nthree\n", "initial");

        let blame = get_blame(tmp.path(), "a.txt").expect("blame ok");
        assert_eq!(blame.len(), 3);
        let hash = &blame[0].commit.hash;
        assert!(blame.iter().all(|b| b.kind == BlameKind::Committed));
        assert!(blame.iter().all(|b| &b.commit.hash == hash));
        assert_eq!(blame[0].commit.author, "Test User");
        assert_eq!(blame[0].commit.summary, "initial");
    }

    #[test]
    fn blame_marks_uncommitted_wt_changes() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        commit_file(tmp.path(), "a.txt", "one\ntwo\nthree\n", "initial");

        // Modify working tree without committing.
        fs::write(tmp.path().join("a.txt"), "one\nNEW\ntwo\nthree\n").unwrap();

        let blame = get_blame(tmp.path(), "a.txt").expect("blame ok");
        assert_eq!(blame.len(), 4);
        assert_eq!(blame[0].kind, BlameKind::Committed);
        assert_eq!(blame[1].kind, BlameKind::Uncommitted);
        assert_eq!(blame[2].kind, BlameKind::Committed);
        assert_eq!(blame[3].kind, BlameKind::Committed);
    }

    #[test]
    fn blame_distinguishes_two_commits() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        commit_file(tmp.path(), "a.txt", "one\ntwo\n", "first");
        commit_file(tmp.path(), "a.txt", "one\ntwo\nthree\n", "second");

        let blame = get_blame(tmp.path(), "a.txt").expect("blame ok");
        assert_eq!(blame.len(), 3);
        assert_eq!(blame[0].commit.summary, "first");
        assert_eq!(blame[1].commit.summary, "first");
        assert_eq!(blame[2].commit.summary, "second");
        assert_ne!(blame[0].commit.hash, blame[2].commit.hash);
    }

    #[test]
    fn blame_untracked_file_is_all_uncommitted() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        commit_file(tmp.path(), "tracked.txt", "x\n", "initial");
        fs::write(tmp.path().join("new.txt"), "a\nb\nc\n").unwrap();

        let blame = get_blame(tmp.path(), "new.txt").expect("blame ok");
        assert_eq!(blame.len(), 3);
        assert!(blame.iter().all(|b| b.kind == BlameKind::Uncommitted));
    }

    #[test]
    fn blame_not_git_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let err = get_blame(tmp.path(), "a.txt").unwrap_err();
        assert!(matches!(err, BlameError::NotGitRepo));
    }

    #[test]
    fn blame_no_commits() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let err = get_blame(tmp.path(), "a.txt").unwrap_err();
        assert!(matches!(err, BlameError::NoCommits));
    }
}
