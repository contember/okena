//! Jujutsu (jj) VCS backend.
//!
//! Implements jj-specific VCS operations that produce the same types as the
//! git module so callers can treat both backends uniformly.

use std::path::Path;
use crate::git::{GitStatus, FileDiffSummary};
use crate::git::diff::{parse_unified_diff, DiffResult, DiffMode};

/// Check whether `path` (or any of its ancestors) contains a `.jj/` directory.
///
/// This is a pure filesystem walk — no subprocess is spawned.
pub fn is_jj_repo(path: &Path) -> bool {
    let mut current = path;
    loop {
        if current.join(".jj").is_dir() {
            return true;
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

/// Get the VCS status for a jj repository.
///
/// Returns `None` if `path` is not inside a jj repo or if jj commands fail.
pub fn get_status(path: &Path) -> Option<GitStatus> {
    if !is_jj_repo(path) {
        return None;
    }

    let path_str = path.to_str()?;

    // Determine branch name: try bookmarks first, fall back to short change ID.
    let branch = {
        let output = crate::process::safe_output(
            crate::process::command("jj")
                .args(["log", "-r", "@", "--no-graph", "-T", "separate(\" \", bookmarks)", "-R", path_str]),
        )
        .ok()?;

        let bookmark = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !bookmark.is_empty() {
            // Use only the first bookmark name
            let first = bookmark.split_whitespace().next().unwrap_or(&bookmark).to_string();
            Some(first)
        } else {
            // Fall back to short change ID
            let id_output = crate::process::safe_output(
                crate::process::command("jj")
                    .args(["log", "-r", "@", "--no-graph", "-T", "change_id.short(8)", "-R", path_str]),
            )
            .ok()?;
            let id = String::from_utf8_lossy(&id_output.stdout).trim().to_string();
            if id.is_empty() { None } else { Some(id) }
        }
    };

    // Get diff stats by running `jj diff --git` and parsing the unified diff.
    let diff_output = crate::process::safe_output(
        crate::process::command("jj")
            .args(["diff", "--git", "-R", path_str]),
    )
    .ok()?;

    let diff_text = String::from_utf8_lossy(&diff_output.stdout);
    let diff_result = parse_unified_diff(&diff_text);

    let lines_added: usize = diff_result.files.iter().map(|f| f.lines_added).sum();
    let lines_removed: usize = diff_result.files.iter().map(|f| f.lines_removed).sum();

    Some(GitStatus { branch, lines_added, lines_removed })
}

/// Get a per-file diff summary for a jj repository.
pub fn get_diff_file_summary(path: &Path) -> Vec<FileDiffSummary> {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return vec![],
    };

    let output = match crate::process::safe_output(
        crate::process::command("jj")
            .args(["diff", "--git", "-R", path_str]),
    ) {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let diff_text = String::from_utf8_lossy(&output.stdout);
    let diff_result = parse_unified_diff(&diff_text);

    let mut summaries: Vec<FileDiffSummary> = diff_result.files.iter().map(|file| {
        let path_str = file.new_path.as_deref()
            .or(file.old_path.as_deref())
            .unwrap_or("unknown")
            .to_string();
        // A file is "new" when the old path is /dev/null (git unified diff convention)
        let is_new = file.old_path.as_deref() == Some("/dev/null");
        FileDiffSummary {
            path: path_str,
            added: file.lines_added,
            removed: file.lines_removed,
            is_new,
        }
    }).collect();

    summaries.sort_by(|a, b| a.path.cmp(&b.path));
    summaries
}

/// Get the full diff for a jj repository.
///
/// `_mode` is ignored — jj has no staging area, so both `WorkingTree` and
/// `Staged` return the same working-copy diff.
pub fn get_diff_with_options(path: &Path, _mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String> {
    let path_str = path.to_str()
        .ok_or_else(|| "invalid path".to_string())?;

    let mut args = vec!["diff", "--git", "-R", path_str];
    if ignore_whitespace {
        args.push("--ignore-all-space");
    }

    let output = crate::process::safe_output(
        crate::process::command("jj").args(&args),
    )
    .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(stderr);
    }

    let diff_text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_unified_diff(&diff_text))
}

/// Get (old, new) file contents for a side-by-side diff view.
///
/// - Old content comes from the parent revision (`@-`) via `jj file show`.
/// - New content is read directly from the filesystem.
/// - `_mode` is ignored (no staging area in jj).
pub fn get_file_contents_for_diff(
    repo_path: &Path,
    file_path: &str,
    _mode: DiffMode,
) -> (Option<String>, Option<String>) {
    let repo_str = match repo_path.to_str() {
        Some(s) => s,
        None => return (None, None),
    };

    // Old content: retrieve from the parent commit (@-).
    let old_content = crate::process::safe_output(
        crate::process::command("jj")
            .args(["file", "show", "--revision", "@-", "-R", repo_str, file_path]),
    )
    .ok()
    .and_then(|output| {
        if output.status.success() {
            String::from_utf8(output.stdout).ok()
        } else {
            None
        }
    });

    // New content: read from the working copy on disk.
    let new_content = std::fs::read_to_string(repo_path.join(file_path)).ok();

    (old_content, new_content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_jj_repo_nonexistent_path() {
        assert!(!is_jj_repo(Path::new("/nonexistent/path")));
    }

    #[test]
    fn test_get_status_non_jj_path() {
        assert!(get_status(Path::new("/tmp")).is_none());
    }

    #[test]
    fn test_jj_diff_parses_through_unified_parser() {
        // Sample jj diff --git output
        let sample = "diff --git a/src/main.rs b/src/main.rs\n\
                       index abc1234..def5678 100644\n\
                       --- a/src/main.rs\n\
                       +++ b/src/main.rs\n\
                       @@ -1,3 +1,4 @@\n\
                        fn main() {\n\
                       +    println!(\"hello\");\n\
                        }\n";
        let result = parse_unified_diff(sample);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].lines_added, 1);
    }
}
