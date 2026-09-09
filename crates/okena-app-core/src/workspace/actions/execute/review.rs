//! Review composition handler: turn one diff into role volumes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use okena_core::types::DiffMode;
use okena_git::{DiffLineType, DiffResult, FileDiff};
use okena_review::{ChangedFile, CompositionLimits, SourceLoader};

use super::{ActionResult, Workspace};

pub(super) fn composition(
    ws: &Workspace,
    project_id: String,
    mode: DiffMode,
    ignore_whitespace: bool,
) -> ActionResult {
    let Some(project) = ws.project(&project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    let path = project.path.clone();
    let repo = Path::new(&path);
    let diff = match okena_git::get_diff_with_options(repo, mode.clone(), ignore_whitespace) {
        Ok(diff) => diff,
        Err(error) => return ActionResult::Err(error.to_string()),
    };

    let files = changed_files(&diff);
    let loader = GitSources::new(repo, mode);
    let composition = okena_review::compose(&files, &loader, CompositionLimits::default());
    ActionResult::Ok(Some(
        serde_json::to_value(composition).expect("BUG: ChangeComposition must serialize"),
    ))
}

fn changed_files(diff: &DiffResult) -> Vec<ChangedFile> {
    diff.files.iter().map(changed_file).collect()
}

fn changed_file(file: &FileDiff) -> ChangedFile {
    let mut added = Vec::new();
    let mut deleted = Vec::new();
    for hunk in &file.hunks {
        for line in &hunk.lines {
            match line.line_type {
                DiffLineType::Added => push_line(&mut added, line.new_line_num),
                DiffLineType::Removed => push_line(&mut deleted, line.old_line_num),
                DiffLineType::Context | DiffLineType::Header => {}
            }
        }
    }
    added.sort_unstable();
    deleted.sort_unstable();
    ChangedFile {
        old_path: file.old_path.clone(),
        new_path: file.new_path.clone(),
        is_binary: file.is_binary,
        added_lines: added,
        deleted_lines: deleted,
    }
}

fn push_line(out: &mut Vec<u32>, number: Option<usize>) {
    if let Some(number) = number.and_then(|number| u32::try_from(number).ok()) {
        out.push(number);
    }
}

/// Blob contents for the comparison's two sides, read at most once per path.
///
/// The two revisions are resolved once. Going through
/// `get_file_contents_for_diff` per file would re-run `git merge-base` — a
/// subprocess — for every file in the comparison.
struct GitSources<'a> {
    repo: &'a Path,
    revisions: Option<(String, String)>,
    mode: DiffMode,
    cache: RefCell<HashMap<(bool, String), Option<String>>>,
}

impl<'a> GitSources<'a> {
    fn new(repo: &'a Path, mode: DiffMode) -> Self {
        Self {
            repo,
            revisions: revisions(repo, &mode),
            mode,
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn load(&self, path: &str, head_side: bool) -> Option<String> {
        let key = (head_side, path.to_string());
        if let Some(cached) = self.cache.borrow().get(&key) {
            return cached.clone();
        }
        let value = match &self.revisions {
            Some((base, head)) => {
                let revision = if head_side { head } else { base };
                okena_git::get_file_from_git(self.repo, revision, path)
            }
            // Working tree and index have no revision to resolve; one call
            // yields both sides, so cache them together.
            None => {
                let (base, head) =
                    okena_git::get_file_contents_for_diff(self.repo, path, self.mode.clone());
                self.cache.borrow_mut().insert(
                    (!head_side, path.to_string()),
                    if head_side {
                        base.clone()
                    } else {
                        head.clone()
                    },
                );
                if head_side { head } else { base }
            }
        };
        self.cache.borrow_mut().insert(key, value.clone());
        value
    }
}

/// The (base, head) revisions a mode compares, when both are revisions.
fn revisions(repo: &Path, mode: &DiffMode) -> Option<(String, String)> {
    match mode {
        DiffMode::WorkingTree | DiffMode::Staged => None,
        DiffMode::Commit(hash) => Some((format!("{hash}^"), hash.clone())),
        DiffMode::BranchCompare { base, head } => {
            let effective_base = repo
                .to_str()
                .and_then(|repo| okena_git::merge_base(repo, base, head))
                .unwrap_or_else(|| base.clone());
            Some((effective_base, head.clone()))
        }
    }
}

impl SourceLoader for GitSources<'_> {
    fn head(&self, path: &str) -> Option<String> {
        self.load(path, true)
    }

    fn base(&self, path: &str) -> Option<String> {
        self.load(path, false)
    }
}

#[cfg(test)]
mod tests {
    use okena_git::diff::{DiffHunk, DiffLine};

    use super::*;

    fn line(line_type: DiffLineType, old: Option<usize>, new: Option<usize>) -> DiffLine {
        DiffLine {
            line_type,
            content: String::new(),
            old_line_num: old,
            new_line_num: new,
        }
    }

    #[test]
    fn hunk_lines_become_ascending_per_side_line_numbers() {
        let file = FileDiff {
            old_path: Some("src/a.rs".into()),
            new_path: Some("src/a.rs".into()),
            is_binary: false,
            lines_added: 2,
            lines_removed: 1,
            hunks: vec![DiffHunk {
                header: "@@ -10,2 +10,3 @@".into(),
                old_start: 10,
                new_start: 10,
                lines: vec![
                    line(DiffLineType::Header, None, None),
                    line(DiffLineType::Context, Some(10), Some(10)),
                    line(DiffLineType::Removed, Some(11), None),
                    line(DiffLineType::Added, None, Some(11)),
                    line(DiffLineType::Added, None, Some(12)),
                ],
            }],
        };
        let changed = changed_file(&file);
        assert_eq!(changed.added_lines, vec![11, 12]);
        assert_eq!(changed.deleted_lines, vec![11]);
    }

    #[test]
    fn a_binary_file_carries_its_paths_and_no_lines() {
        let file = FileDiff {
            old_path: None,
            new_path: Some("assets/icon.png".into()),
            is_binary: true,
            lines_added: 0,
            lines_removed: 0,
            hunks: Vec::new(),
        };
        let changed = changed_file(&file);
        assert!(changed.is_binary);
        assert!(changed.added_lines.is_empty());
        assert_eq!(changed.path(), "assets/icon.png");
    }
}
