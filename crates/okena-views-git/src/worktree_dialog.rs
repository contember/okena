//! Worktree creation dialog. Search/pick an existing branch (or type a new
//! name) or pick a PR; on confirm runs `git worktree add` via the workspace.
//!
//! The `Render` impl lives in `worktree_dialog/view.rs`.

use okena_git as git;
use okena_git::repository::{compute_target_paths, normalize_path};
use okena_core::process::command;
use okena_workspace::settings::{HooksConfig, WorktreeConfig};
use okena_workspace::state::Workspace;

use crate::simple_input::SimpleInputState;

use gpui::prelude::*;
use gpui::*;
use std::path::{Path, PathBuf};

mod view;

#[derive(Clone, Debug)]
pub(super) struct PrInfo {
    pub(super) number: u32,
    pub(super) title: String,
    pub(super) branch: String,
}

/// Events emitted by the worktree dialog
#[derive(Clone)]
pub enum WorktreeDialogEvent {
    /// Dialog closed without creating a worktree (cancelled)
    Close,
    /// Worktree was successfully created, contains the new project ID
    Created(String),
}

impl EventEmitter<WorktreeDialogEvent> for WorktreeDialog {}

/// Dialog for creating a new worktree from a project
pub struct WorktreeDialog {
    pub(super) workspace: Entity<Workspace>,
    pub(super) project_id: String,
    pub(super) project_path: String,
    /// The git repository root (may differ from project_path in monorepos)
    pub(super) git_root: PathBuf,
    /// Relative path from git root to project (empty if project is at repo root)
    pub(super) subdir: PathBuf,
    pub(super) branches: Vec<String>,
    pub(super) filtered_branches: Vec<usize>,
    pub(super) selected_branch_index: Option<usize>,
    pub(super) branch_search_input: Entity<SimpleInputState>,
    pub(super) error_message: Option<String>,
    pub(super) focus_handle: FocusHandle,
    pub(super) initialized: bool,
    pub(super) last_search_query: String,
    pub(super) pr_mode: bool,
    pub(super) pr_list: Vec<PrInfo>,
    pub(super) loading_prs: bool,
    pub(super) pr_error: Option<String>,
    pub(super) selected_pr_branch: Option<String>,
    pub(super) prs_loaded_once: bool,
    pub(super) path_template: String,
    pub(super) hooks_config: HooksConfig,
}

impl WorktreeDialog {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        project_path: String,
        worktree_config: WorktreeConfig,
        hooks_config: HooksConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        // Determine git repo root: if parent is already a worktree, use its
        // stored main_repo_path; otherwise detect via `git rev-parse --show-toplevel`.
        let project_pathbuf = PathBuf::from(&project_path);
        let parent_main_repo = workspace.read(cx).worktree_parent_path(&project_id)
            .map(PathBuf::from);
        let git_root = parent_main_repo
            .or_else(|| git::get_repo_root(&project_pathbuf))
            .unwrap_or_else(|| project_pathbuf.clone());
        // Normalize both paths before strip_prefix to handle relative paths,
        // symlinks, or platform-specific path representations
        let normalized_project = normalize_path(&project_pathbuf);
        let normalized_root = normalize_path(&git_root);
        let subdir = normalized_project.strip_prefix(&normalized_root)
            .unwrap_or(Path::new(""))
            .to_path_buf();

        // Get available branches using the git root
        let branches = git::get_available_branches_for_worktree(&git_root);

        // Pre-generate a branch name suggestion
        let generated_branch = okena_git::branch_names::generate_branch_name(&git_root);

        let branch_search_input = cx.new(|cx| {
            let mut input = SimpleInputState::new(cx)
                .placeholder("Search or create branch...")
                .icon("icons/search.svg");
            input.set_value(&generated_branch, cx);
            input
        });

        let filtered_branches: Vec<usize> = (0..branches.len()).collect();
        let focus_handle = cx.focus_handle();
        let path_template = worktree_config.path_template;

        Self {
            workspace,
            project_id,
            project_path,
            git_root,
            subdir,
            branches,
            filtered_branches,
            selected_branch_index: None,
            branch_search_input,
            error_message: None,
            focus_handle,
            initialized: false,
            last_search_query: String::new(),
            pr_mode: false,
            pr_list: vec![],
            loading_prs: false,
            pr_error: None,
            selected_pr_branch: None,
            prs_loaded_once: false,
            path_template,
            hooks_config,
        }
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
            self.filtered_branches = self.branches
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

    /// Returns (worktree_path, project_path).
    /// `worktree_path` is where `git worktree add` creates the checkout (at the repo root level).
    /// `project_path` is the subdirectory within that worktree where the project lives
    /// (same as worktree_path when project is at repo root).
    fn get_target_paths(&self, branch: &str) -> (String, String) {
        compute_target_paths(&self.git_root, &self.subdir, &self.path_template, branch)
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
                self.error_message = Some("Please select a branch or type a new branch name".to_string());
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

        let (worktree_path, project_path) = self.get_target_paths(&branch);
        let project_id = self.project_id.clone();
        let git_root = self.git_root.clone();
        let hooks_config = self.hooks_config.clone();

        // Create the worktree project
        let result = self.workspace.update(cx, |ws, cx| {
            ws.create_worktree_project(&project_id, &branch, &git_root, &worktree_path, &project_path, create_branch, &hooks_config, cx)
        });

        match result {
            Ok(new_project_id) => {
                cx.emit(WorktreeDialogEvent::Created(new_project_id));
            }
            Err(e) => {
                self.error_message = Some(e);
                cx.notify();
            }
        }
    }

    pub(super) fn load_prs(&mut self, cx: &mut Context<Self>) {
        self.loading_prs = true;
        self.pr_error = None;
        cx.notify();

        let project_path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                let output = command("gh")
                    .args(["pr", "list", "--json", "number,title,headRefName", "--limit", "20"])
                    .current_dir(&project_path)
                    .output();

                match output {
                    Ok(output) if output.status.success() => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let parsed: Result<Vec<serde_json::Value>, _> = serde_json::from_str(&stdout);
                        match parsed {
                            Ok(items) => {
                                let prs: Vec<PrInfo> = items
                                    .into_iter()
                                    .filter_map(|v| {
                                        Some(PrInfo {
                                            number: v.get("number")?.as_u64()? as u32,
                                            title: v.get("title")?.as_str()?.to_string(),
                                            branch: v.get("headRefName")?.as_str()?.to_string(),
                                        })
                                    })
                                    .collect();
                                Ok(prs)
                            }
                            Err(e) => Err(format!("Failed to parse PR data: {}", e)),
                        }
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        Err(stderr.trim().to_string())
                    }
                    Err(_) => Err("GitHub CLI not found. Install gh: https://cli.github.com".to_string()),
                }
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
