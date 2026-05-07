//! Lazy directory listing for the file viewer tree.
//!
//! Returns immediate children (one level deep) of a project-relative path,
//! respecting `.gitignore`, global gitignore, and the project's `ALWAYS_IGNORE`
//! overrides. Used by the file viewer to expand folders on demand instead of
//! pre-scanning the entire project up to `MAX_FILES`.

use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One direct child of a directory.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// List the immediate children of `relative_path` inside `project_root`.
///
/// `relative_path = ""` lists the project root.
/// `show_ignored = true` includes `.gitignore`d entries, but never `.git/` or
/// other `ALWAYS_IGNORE` patterns.
///
/// Entries are sorted with directories first, then files, both alphabetically.
pub fn list_directory(
    project_root: &Path,
    relative_path: &str,
    show_ignored: bool,
) -> Result<Vec<DirEntry>, String> {
    let target = if relative_path.is_empty() {
        project_root.to_path_buf()
    } else {
        project_root.join(relative_path)
    };

    let metadata = std::fs::metadata(&target)
        .map_err(|e| format!("Cannot read directory: {}", e))?;
    if !metadata.is_dir() {
        return Err(format!("Not a directory: {}", target.display()));
    }

    let mut walk_builder = WalkBuilder::new(&target);
    walk_builder
        .hidden(false)
        .git_ignore(!show_ignored)
        .git_global(!show_ignored)
        .git_exclude(!show_ignored)
        .max_depth(Some(1));

    // Overrides anchored at the project root so absolute patterns (e.g.
    // `.claude/worktrees/`) match the same way regardless of which subdir
    // we're listing from. Patterns without an internal slash (e.g. `.git/`)
    // match at any depth, which is what we want.
    let mut override_builder = ignore::overrides::OverrideBuilder::new(project_root);
    for pattern in crate::content_search::ALWAYS_IGNORE {
        let _ = override_builder.add(pattern);
    }
    if let Ok(overrides) = override_builder.build() {
        walk_builder.overrides(overrides);
    }

    let mut entries = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for walk_entry in walk_builder.build().flatten() {
        let path = walk_entry.path();
        if path == target {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let name = name.to_string();
        if !seen.insert(name.clone()) {
            continue;
        }
        let is_dir = walk_entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or_else(|| path.is_dir());
        entries.push(DirEntry { name, is_dir });
    }

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn names(entries: &[DirEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn lists_top_level_with_dirs_first() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "z_file.txt", "");
        write(root, "a_file.txt", "");
        fs::create_dir(root.join("z_dir")).unwrap();
        fs::create_dir(root.join("a_dir")).unwrap();

        let entries = list_directory(root, "", false).unwrap();
        assert_eq!(names(&entries), vec!["a_dir", "z_dir", "a_file.txt", "z_file.txt"]);
        assert!(entries[0].is_dir && entries[1].is_dir);
        assert!(!entries[2].is_dir && !entries[3].is_dir);
    }

    #[test]
    fn lists_subdirectory() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "sub/inner.txt", "");
        write(root, "sub/nested/deep.txt", "");

        let entries = list_directory(root, "sub", false).unwrap();
        // Direct children only — `nested` is shown as a dir but its contents
        // aren't recursed into.
        assert_eq!(names(&entries), vec!["nested", "inner.txt"]);
    }

    fn git_init(root: &Path) {
        std::process::Command::new("git")
            .arg("init")
            .arg("--quiet")
            .current_dir(root)
            .output()
            .ok();
    }

    #[test]
    fn respects_gitignore_when_show_ignored_false() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        git_init(root);
        write(root, ".gitignore", "ignored.txt\nbuild/\n");
        write(root, "ignored.txt", "");
        write(root, "kept.txt", "");
        fs::create_dir(root.join("build")).unwrap();
        fs::create_dir(root.join("src")).unwrap();

        let entries = list_directory(root, "", false).unwrap();
        let n = names(&entries);
        assert!(!n.contains(&"ignored.txt"), "got {:?}", n);
        assert!(!n.contains(&"build"), "got {:?}", n);
        assert!(n.contains(&"kept.txt"));
        assert!(n.contains(&"src"));
    }

    #[test]
    fn show_ignored_true_includes_gitignored() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        git_init(root);
        write(root, ".gitignore", "ignored.txt\n");
        write(root, "ignored.txt", "");
        write(root, "kept.txt", "");

        let entries = list_directory(root, "", true).unwrap();
        let n = names(&entries);
        assert!(n.contains(&"ignored.txt"));
        assert!(n.contains(&"kept.txt"));
    }

    #[test]
    fn always_ignores_dot_git() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        git_init(root);
        write(root, "src/main.rs", "");

        let entries = list_directory(root, "", false).unwrap();
        let n1 = names(&entries);
        assert!(!n1.contains(&".git"), "got {:?}", n1);

        // Also true with show_ignored = true — `.git` is in ALWAYS_IGNORE.
        let entries = list_directory(root, "", true).unwrap();
        let n2 = names(&entries);
        assert!(!n2.contains(&".git"), "got {:?}", n2);
    }

    #[test]
    fn always_ignores_claude_worktrees_at_project_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        git_init(root);
        write(root, ".claude/worktrees/agent-1/file.txt", "");
        write(root, ".claude/commands/foo.md", "");
        write(root, "src/main.rs", "");

        // From the project root, `.claude` should still appear (commands lives
        // there), but `worktrees` inside it should not.
        let top = list_directory(root, "", false).unwrap();
        assert!(names(&top).contains(&".claude"));

        let claude = list_directory(root, ".claude", false).unwrap();
        let n = names(&claude);
        assert!(n.contains(&"commands"), "got {:?}", n);
        assert!(!n.contains(&"worktrees"), "got {:?}", n);
    }

    #[test]
    fn errors_on_missing_path() {
        let tmp = TempDir::new().unwrap();
        let err = list_directory(tmp.path(), "nope/nada", false).unwrap_err();
        assert!(err.contains("Cannot read directory"), "err = {}", err);
    }

    #[test]
    fn errors_on_file_target() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "file.txt", "x");
        let err = list_directory(tmp.path(), "file.txt", false).unwrap_err();
        assert!(err.contains("Not a directory"), "err = {}", err);
    }
}
