//! Git action handlers.
//!
//! Each handler resolves the project path and delegates to the `crate::git`
//! query layer. All return DTOs serialize infallibly for well-formed types
//! (see the module-level `expect_used` allow in the parent module).

use super::{ActionResult, Workspace};
use okena_core::types::DiffMode;

pub(super) fn status(ws: &Workspace, project_id: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            let status = crate::git::get_git_status(std::path::Path::new(&path));
            ActionResult::Ok(Some(serde_json::to_value(status).expect("BUG: GitStatus must serialize")))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn diff_summary(ws: &Workspace, project_id: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            let summary = crate::git::get_diff_file_summary(std::path::Path::new(&path));
            ActionResult::Ok(Some(serde_json::to_value(summary).expect("BUG: FileDiffSummary must serialize")))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn diff(ws: &Workspace, project_id: String, mode: DiffMode, ignore_whitespace: bool) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            match crate::git::get_diff_with_options(std::path::Path::new(&path), mode, ignore_whitespace) {
                Ok(diff) => ActionResult::Ok(Some(serde_json::to_value(diff).expect("BUG: DiffResult must serialize"))),
                Err(e) => ActionResult::Err(e.to_string()),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn branches(ws: &Workspace, project_id: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            let branches = crate::git::get_available_branches_for_worktree(std::path::Path::new(&path));
            ActionResult::Ok(Some(serde_json::to_value(branches).expect("BUG: branches must serialize")))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn file_contents(ws: &Workspace, project_id: String, file_path: String, mode: DiffMode) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let repo_path = p.path.clone();
            let (old, new) = crate::git::get_file_contents_for_diff(
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
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            let entries = crate::git::fetch_commit_log(
                std::path::Path::new(&path),
                count,
                branch.as_deref(),
            );
            ActionResult::Ok(Some(serde_json::to_value(entries).expect("BUG: CommitLogEntry must serialize")))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn list_branches(ws: &Workspace, project_id: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            let branches = crate::git::list_branches(std::path::Path::new(&path));
            ActionResult::Ok(Some(serde_json::to_value(branches).expect("BUG: branches must serialize")))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn stage_file(ws: &Workspace, project_id: String, file_path: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            match crate::git::stage_file(std::path::Path::new(&path), &file_path) {
                Ok(()) => ActionResult::Ok(None),
                Err(e) => ActionResult::Err(e.to_string()),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn unstage_file(ws: &Workspace, project_id: String, file_path: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            match crate::git::unstage_file(std::path::Path::new(&path), &file_path) {
                Ok(()) => ActionResult::Ok(None),
                Err(e) => ActionResult::Err(e.to_string()),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn discard_file(ws: &Workspace, project_id: String, file_path: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = p.path.clone();
            match crate::git::discard_file_changes(std::path::Path::new(&path), &file_path) {
                Ok(()) => ActionResult::Ok(None),
                Err(e) => ActionResult::Err(e.to_string()),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
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
