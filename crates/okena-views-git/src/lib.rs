pub mod diff_viewer;

/// VCS backend detection — simplified enum for the views-git crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsBackend {
    Git,
    Jujutsu,
}

/// Detect which VCS backend a path uses.
pub fn detect_vcs(path: &std::path::Path) -> Option<VcsBackend> {
    // Check for jj first (it co-exists with .git)
    if path.join(".jj").is_dir() {
        return Some(VcsBackend::Jujutsu);
    }
    if okena_git::is_git_repo(path) {
        return Some(VcsBackend::Git);
    }
    None
}
pub mod git_header;
pub mod project_header;
pub mod settings;
pub mod simple_input;
pub mod watcher;
pub mod worktree_dialog;
pub mod close_worktree_dialog;

gpui::actions!(okena_views_git, [Cancel]);
