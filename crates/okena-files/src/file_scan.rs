//! GPUI-free file scanning for the fuzzy finder index.
//!
//! Walks a project directory with the `ignore` crate and produces a flat list
//! of [`FileEntry`] values. The scanning logic has no GPUI dependency, so it is
//! usable from a headless daemon (via `ProjectFs` / the remote action handlers)
//! as well as from the GUI's `FileSearchDialog`.

use crate::content_search::ALWAYS_IGNORE;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

/// Maximum number of files to keep in the fuzzy finder index.
///
/// The file viewer tree is now lazy (see `crate::list_directory`), so the cap
/// only constrains the Cmd+P fuzzy finder. 25k covers all but the very
/// largest monorepos while keeping `nucleo` per-keystroke matching snappy.
const MAX_FILES: usize = 25_000;

/// A file entry in the search list.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    /// Full path to the file
    pub path: PathBuf,
    /// Path relative to project root
    pub relative_path: String,
    /// Just the filename
    pub filename: String,
}

/// Scan files in the project directory using the `ignore` crate.
///
/// `show_ignored` is additive: regular (non-gitignored) files are scanned
/// first, then gitignored files are appended up to `MAX_FILES`. Without
/// this two-pass split, a single huge gitignored directory (e.g. an
/// Android `build/` tree) can fill the cap alphabetically and crowd out
/// real project files later in the walk.
pub fn scan_files(project_path: &Path, show_ignored: bool) -> Vec<FileEntry> {
    let mut files = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    collect_files(project_path, false, &mut files, &mut seen);
    if show_ignored {
        collect_files(project_path, true, &mut files, &mut seen);
    }

    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    files
}

/// Walk the project, appending entries to `files` until `MAX_FILES` is
/// reached. `seen` tracks already-collected paths so the gitignored pass
/// doesn't duplicate the regular pass.
fn collect_files(
    project_path: &Path,
    include_ignored: bool,
    files: &mut Vec<FileEntry>,
    seen: &mut std::collections::HashSet<PathBuf>,
) {
    let mut walk_builder = WalkBuilder::new(project_path);
    walk_builder
        .hidden(false)
        .git_ignore(!include_ignored)
        .git_global(!include_ignored)
        .git_exclude(!include_ignored)
        .max_depth(Some(15));

    let mut override_builder = ignore::overrides::OverrideBuilder::new(project_path);
    for pattern in ALWAYS_IGNORE {
        let _ = override_builder.add(pattern);
    }
    if let Ok(overrides) = override_builder.build() {
        walk_builder.overrides(overrides);
    }

    for entry in walk_builder.build().flatten() {
        if files.len() >= MAX_FILES {
            break;
        }

        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if !seen.insert(path.to_path_buf()) {
            continue;
        }

        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        let relative_path = path
            .strip_prefix(project_path)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| filename.clone());

        files.push(FileEntry {
            path: path.to_path_buf(),
            relative_path,
            filename,
        });
    }
}
