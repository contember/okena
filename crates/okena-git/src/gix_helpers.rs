//! Helpers for opening `gix` repositories. Centralizes discovery so each
//! call site doesn't need to think about the `ThreadSafeRepository` dance.

use std::path::Path;

/// Discover and open a `gix` repository starting from `path`, walking upward.
/// Returns `None` if no git repository is found or if opening fails for any
/// reason — mirrors the soft-fail semantics of the previous CLI-based callers.
pub(crate) fn open(path: &Path) -> Option<gix::Repository> {
    gix::ThreadSafeRepository::discover(path)
        .ok()
        .map(|r| r.to_thread_local())
}

/// List untracked files honoring `.gitignore`, with paths relative to
/// `query_path` (matches the previous `git -C path ls-files --others
/// --exclude-standard` behavior, including for monorepo subdirs).
pub(crate) fn list_untracked_files(query_path: &Path) -> Vec<String> {
    let Some(repo) = open(query_path) else {
        return vec![];
    };
    let Some(workdir) = repo.workdir() else {
        return vec![];
    };

    // Compute the prefix from workdir to query_path. Empty when query_path
    // is the workdir itself.
    let canonical_query = query_path.canonicalize().unwrap_or_else(|_| query_path.to_path_buf());
    let canonical_workdir = workdir.canonicalize().unwrap_or_else(|_| workdir.to_path_buf());
    let prefix: String = canonical_query
        .strip_prefix(&canonical_workdir)
        .ok()
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            if s.is_empty() { String::new() } else { format!("{}/", s) }
        })
        .unwrap_or_default();

    let Ok(platform) = repo.status(gix::progress::Discard) else {
        return vec![];
    };
    let Ok(iter) = platform
        .untracked_files(gix::status::UntrackedFiles::Files)
        .into_iter(None)
    else {
        return vec![];
    };

    let mut result = Vec::new();
    for item in iter.flatten() {
        let gix::status::Item::IndexWorktree(
            gix::status::index_worktree::Item::DirectoryContents { entry, .. },
        ) = item
        else {
            continue;
        };
        if !matches!(entry.status, gix::dir::entry::Status::Untracked) {
            continue;
        }
        let rela = entry.rela_path.to_string();
        if prefix.is_empty() {
            result.push(rela);
        } else if let Some(stripped) = rela.strip_prefix(&prefix) {
            result.push(stripped.to_string());
        }
    }
    result
}
