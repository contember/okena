//! Git action handlers.
//!
//! Each handler resolves the project path and delegates to the `okena_git`
//! query layer. All return DTOs serialize infallibly for well-formed types
//! (see the module-level `expect_used` allow in the parent module).

use super::{ActionResult, Workspace};
use okena_core::types::DiffMode;
use std::path::Path;

fn with_project_path(
    ws: &Workspace,
    project_id: String,
    f: impl FnOnce(&Path) -> ActionResult,
) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => f(Path::new(&p.path)),
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

fn run_project_git_command<E>(
    ws: &Workspace,
    project_id: String,
    f: impl FnOnce(&Path) -> Result<(), E>,
) -> ActionResult
where
    E: std::fmt::Display,
{
    with_project_path(ws, project_id, |path| match f(path) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(e.to_string()),
    })
}

pub(super) fn status(ws: &Workspace, project_id: String) -> ActionResult {
    with_project_path(ws, project_id, |path| {
        let status = okena_git::get_git_status(path);
        ActionResult::Ok(Some(serde_json::to_value(status).expect("BUG: GitStatus must serialize")))
    })
}

pub(super) fn diff_summary(ws: &Workspace, project_id: String) -> ActionResult {
    with_project_path(ws, project_id, |path| {
        let summary = okena_git::get_diff_file_summary(path);
        ActionResult::Ok(Some(serde_json::to_value(summary).expect("BUG: FileDiffSummary must serialize")))
    })
}

pub(super) fn diff(ws: &Workspace, project_id: String, mode: DiffMode, ignore_whitespace: bool) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            match okena_git::get_diff_with_options(std::path::Path::new(&path), mode, ignore_whitespace) {
                Ok(diff) => ActionResult::Ok(Some(serde_json::to_value(diff).expect("BUG: DiffResult must serialize"))),
                Err(e) => ActionResult::Err(e.to_string()),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn branches(ws: &Workspace, project_id: String) -> ActionResult {
    with_project_path(ws, project_id, |path| {
        let branches = okena_git::get_available_branches_for_worktree(path);
        ActionResult::Ok(Some(serde_json::to_value(branches).expect("BUG: branches must serialize")))
    })
}

pub(super) fn file_contents(ws: &Workspace, project_id: String, file_path: String, mode: DiffMode) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let repo_path = p.path.clone();
            let (old, new) = okena_git::get_file_contents_for_diff(
                std::path::Path::new(&repo_path),
                &file_path,
                mode,
            );
            ActionResult::Ok(Some(serde_json::json!({
                "old_content": old,
                "new_content": new,
            })))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn commit_graph(ws: &Workspace, project_id: String, count: usize, branch: Option<String>) -> ActionResult {
    with_project_path(ws, project_id, |path| {
        let entries = okena_git::fetch_commit_log(
            path,
            count,
            branch.as_deref(),
        );
        ActionResult::Ok(Some(serde_json::to_value(entries).expect("BUG: CommitLogEntry must serialize")))
    })
}

pub(super) fn list_branches(ws: &Workspace, project_id: String) -> ActionResult {
    with_project_path(ws, project_id, |path| {
        let branches = okena_git::list_branches(path);
        ActionResult::Ok(Some(serde_json::to_value(branches).expect("BUG: branches must serialize")))
    })
}

pub(super) fn list_worktrees(ws: &Workspace, project_id: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            let (git_root, subdir) = okena_git::resolve_git_root_and_subdir(std::path::Path::new(&path));
            let norm_git_root = okena_git::repository::normalize_path(&git_root);
            let worktrees = okena_git::repository::list_git_worktrees(&git_root);
            ActionResult::Ok(Some(serde_json::json!({
                "git_root": norm_git_root.to_string_lossy(),
                "subdir": subdir.to_string_lossy(),
                "worktrees": worktrees,
            })))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn worktree_close_info(ws: &Workspace, project_id: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let project_path = p.path.clone();
            let main_repo_path = ws.worktree_parent_path(&project_id);
            let path = std::path::Path::new(&project_path);
            let is_dirty = okena_git::has_uncommitted_changes(path);
            let branch = okena_git::get_current_branch(path);
            let default_branch = main_repo_path
                .as_ref()
                .and_then(|p| okena_git::get_default_branch(std::path::Path::new(p)));
            let unpushed_count = okena_git::count_unpushed_commits(path).unwrap_or(0);
            ActionResult::Ok(Some(serde_json::json!({
                "is_dirty": is_dirty,
                "branch": branch,
                "default_branch": default_branch,
                "unpushed_count": unpushed_count,
            })))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn generate_worktree_branch_name(ws: &Workspace, project_id: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            let (git_root, _subdir) = okena_git::resolve_git_root_and_subdir(std::path::Path::new(&path));
            let branch = okena_git::branch_names::generate_branch_name(&git_root);
            ActionResult::Ok(Some(serde_json::json!({ "branch": branch })))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn list_branches_classified(ws: &Workspace, project_id: String) -> ActionResult {
    with_project_path(ws, project_id, |path| {
        let branches = okena_git::list_branches_classified(path);
        ActionResult::Ok(Some(serde_json::to_value(branches).expect("BUG: BranchList must serialize")))
    })
}

pub(super) fn checkout_local_branch(ws: &Workspace, project_id: String, branch: String) -> ActionResult {
    run_project_git_command(ws, project_id, |path| okena_git::checkout_local_branch(path, &branch))
}

pub(super) fn checkout_remote_branch(ws: &Workspace, project_id: String, remote_branch: String) -> ActionResult {
    run_project_git_command(ws, project_id, |path| okena_git::checkout_remote_branch(path, &remote_branch))
}

pub(super) fn create_and_checkout_branch(
    ws: &Workspace,
    project_id: String,
    new_name: String,
    start_point: Option<String>,
) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            match okena_git::create_and_checkout_branch(
                std::path::Path::new(&path),
                &new_name,
                start_point.as_deref(),
            ) {
                Ok(()) => ActionResult::Ok(None),
                Err(e) => ActionResult::Err(e.to_string()),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn stage_file(ws: &Workspace, project_id: String, file_path: String) -> ActionResult {
    run_project_git_command(ws, project_id, |path| okena_git::stage_file(path, &file_path))
}

pub(super) fn unstage_file(ws: &Workspace, project_id: String, file_path: String) -> ActionResult {
    run_project_git_command(ws, project_id, |path| okena_git::unstage_file(path, &file_path))
}

pub(super) fn discard_file(ws: &Workspace, project_id: String, file_path: String) -> ActionResult {
    run_project_git_command(ws, project_id, |path| okena_git::discard_file_changes(path, &file_path))
}

pub(super) fn blame(ws: &Workspace, project_id: String, relative_path: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            match okena_git::get_blame(std::path::Path::new(&path), &relative_path) {
                Ok(lines) => {
                    let wire: Vec<_> = lines
                        .into_iter()
                        .map(|l| serde_json::json!({
                            "line_number": l.line_number,
                            "commit": {
                                "hash": l.commit.hash,
                                "short_hash": l.commit.short_hash,
                                "author": l.commit.author,
                                "author_email": l.commit.author_email,
                                "timestamp": l.commit.timestamp,
                                "summary": l.commit.summary,
                            },
                            "kind": match l.kind {
                                okena_git::BlameKind::Committed => "Committed",
                                okena_git::BlameKind::Uncommitted => "Uncommitted",
                            },
                        }))
                        .collect();
                    ActionResult::Ok(Some(serde_json::Value::Array(wire)))
                }
                Err(e) => ActionResult::Err(e.to_string()),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}
