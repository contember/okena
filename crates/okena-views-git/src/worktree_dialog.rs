//! Worktree creation dialog. Search/pick an existing branch (or type a new
//! name) or pick a PR; on confirm runs `git worktree add` via the workspace.
//!
//! The `Render` impl lives in `worktree_dialog/view.rs`.

use okena_core::api::{ActionRequest, WorktreePullRequest};
use okena_transport::remote_action::RemoteActionClient;

use crate::simple_input::SimpleInputState;

use gpui::prelude::*;
use gpui::*;
mod view;

/// Events emitted by the worktree dialog
#[derive(Clone)]
pub enum WorktreeDialogEvent {
    /// Dialog closed without creating a worktree (cancelled)
    Close,
    /// User confirmed creation. The daemon owns worktree creation, so the host
    /// dispatches `ActionRequest::CreateWorktree { project_id, branch,
    /// create_branch }`; the new worktree project (and its terminals) mirror
    /// back. `project_id` is the parent project the worktree is created from.
    RequestCreate {
        project_id: String,
        branch: String,
        create_branch: bool,
    },
}

impl EventEmitter<WorktreeDialogEvent> for WorktreeDialog {}

/// Dialog for creating a new worktree from a project.
///
/// The dialog only collects user intent (which branch / PR, or a new branch
/// name). On confirm it emits `WorktreeDialogEvent::RequestCreate`; the host
/// dispatches `ActionRequest::CreateWorktree` to the daemon, which owns the
/// actual worktree creation (path computation, fetch, `git worktree add`,
/// project registration, terminals and hooks). The new worktree project then
/// mirrors back. Hence the dialog holds no `workspace`, git-root, path-template
/// or hooks state — only branch selection.
pub struct WorktreeDialog {
    client: RemoteActionClient,
    daemon_project_id: String,
    pub(super) project_id: String,
    pub(super) branches: Vec<String>,
    pub(super) filtered_branches: Vec<usize>,
    pub(super) selected_branch_index: Option<usize>,
    pub(super) branch_search_input: Entity<SimpleInputState>,
    pub(super) error_message: Option<String>,
    pub(super) loading_branches: bool,
    pub(super) focus_handle: FocusHandle,
    pub(super) initialized: bool,
    pub(super) last_search_query: String,
    pub(super) pr_mode: bool,
    pub(super) pr_list: Vec<WorktreePullRequest>,
    pub(super) loading_prs: bool,
    pub(super) pr_error: Option<String>,
    pub(super) selected_pr_branch: Option<String>,
    pub(super) prs_loaded_once: bool,
}

impl WorktreeDialog {
    pub fn new(
        client: RemoteActionClient,
        daemon_project_id: String,
        project_id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let branch_search_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Search or create branch...")
                .icon("icons/search.svg")
        });

        let focus_handle = cx.focus_handle();

        let mut dialog = Self {
            client,
            daemon_project_id,
            project_id,
            branches: Vec::new(),
            filtered_branches: Vec::new(),
            selected_branch_index: None,
            branch_search_input,
            error_message: None,
            loading_branches: true,
            focus_handle,
            initialized: false,
            last_search_query: String::new(),
            pr_mode: false,
            pr_list: vec![],
            loading_prs: false,
            pr_error: None,
            selected_pr_branch: None,
            prs_loaded_once: false,
        };
        dialog.load_initial_data(cx);
        dialog
    }

    fn load_initial_data(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let project_id = self.daemon_project_id.clone();
        cx.spawn(async move |this, cx| {
            let (branches, generated_branch) = smol::unblock(move || {
                let branches = client
                    .post_action(ActionRequest::GitBranches {
                        project_id: project_id.clone(),
                    })
                    .and_then(|value| value.ok_or_else(|| "Missing branch list".to_string()))
                    .and_then(|value| {
                        serde_json::from_value::<Vec<String>>(value)
                            .map_err(|error| format!("Invalid branch list: {error}"))
                    });
                let generated_branch = client
                    .post_action(ActionRequest::GenerateWorktreeBranchName { project_id })
                    .and_then(|value| {
                        value.ok_or_else(|| "Missing generated branch name".to_string())
                    })
                    .and_then(|value| {
                        value
                            .get("branch")
                            .and_then(serde_json::Value::as_str)
                            .map(String::from)
                            .ok_or_else(|| "Invalid generated branch name".to_string())
                    });
                (branches, generated_branch)
            })
            .await;

            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    let mut errors = Vec::new();
                    match branches {
                        Ok(branches) => {
                            this.filtered_branches = (0..branches.len()).collect();
                            this.branches = branches;
                        }
                        Err(error) => errors.push(error),
                    }
                    match generated_branch {
                        Ok(branch) => {
                            if this.branch_search_input.read(cx).value().is_empty() {
                                this.branch_search_input.update(cx, |input, cx| {
                                    input.set_value(&branch, cx);
                                });
                            }
                        }
                        Err(error) => errors.push(error),
                    }
                    if !errors.is_empty() {
                        this.error_message = Some(errors.join("; "));
                    }
                    this.loading_branches = false;
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(super) fn filter_branches(&mut self, cx: &App) {
        let query = self.branch_search_input.read(cx).value().to_lowercase();

        // Only re-filter and reset selection if the query actually changed
        if query == self.last_search_query {
            return;
        }
        self.last_search_query = query.clone();

        if query.is_empty() {
            self.filtered_branches = (0..self.branches.len()).collect();
        } else {
            self.filtered_branches = self
                .branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect();
        }
        // Reset selection when filter changes
        self.selected_branch_index = None;
    }

    pub(super) fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorktreeDialogEvent::Close);
    }

    pub(super) fn create_worktree(&mut self, cx: &mut Context<Self>) {
        let (branch, create_branch) = if self.pr_mode {
            // PR mode: use selected PR branch
            if let Some(ref pr_branch) = self.selected_pr_branch {
                (pr_branch.clone(), false)
            } else {
                self.error_message = Some("Please select a pull request".to_string());
                cx.notify();
                return;
            }
        } else if let Some(filtered_idx) = self.selected_branch_index {
            // Use selected existing branch
            if let Some(&branch_idx) = self.filtered_branches.get(filtered_idx) {
                if let Some(branch) = self.branches.get(branch_idx) {
                    (branch.clone(), false)
                } else {
                    self.error_message = Some("Invalid branch selection".to_string());
                    cx.notify();
                    return;
                }
            } else {
                self.error_message = Some("Invalid branch selection".to_string());
                cx.notify();
                return;
            }
        } else {
            // No branch selected — use input text as new branch name
            let name = self.branch_search_input.read(cx).value().trim().to_string();
            if name.is_empty() {
                self.error_message =
                    Some("Please select a branch or type a new branch name".to_string());
                cx.notify();
                return;
            }
            // If it exactly matches an existing branch, use it directly
            if self.branches.iter().any(|b| b == &name) {
                (name, false)
            } else {
                (name, true)
            }
        };

        // The daemon owns worktree creation. Emit a request so the host
        // dispatches `ActionRequest::CreateWorktree`; the new worktree project
        // and its terminals mirror back. The GUI no longer mutates the
        // read-only mirror or computes the worktree path itself (the daemon
        // does, from its settings).
        let project_id = self.project_id.clone();
        cx.emit(WorktreeDialogEvent::RequestCreate {
            project_id,
            branch,
            create_branch,
        });
    }

    pub(super) fn load_prs(&mut self, cx: &mut Context<Self>) {
        self.loading_prs = true;
        self.pr_error = None;
        cx.notify();

        let client = self.client.clone();
        let project_id = self.daemon_project_id.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::GitListPullRequests {
                        project_id,
                        limit: 20,
                    })
                    .and_then(|value| value.ok_or_else(|| "Missing pull request list".to_string()))
                    .and_then(|value| {
                        serde_json::from_value::<Vec<WorktreePullRequest>>(value)
                            .map_err(|error| format!("Invalid pull request list: {error}"))
                    })
            })
            .await;

            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    match result {
                        Ok(prs) => {
                            this.pr_list = prs;
                        }
                        Err(e) => {
                            this.pr_error = Some(e);
                        }
                    }
                    this.loading_prs = false;
                    cx.notify();
                })
            });
        })
        .detach();
    }
}
