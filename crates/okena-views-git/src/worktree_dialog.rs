//! Worktree creation dialog. Search/pick an existing branch (or type a new
//! name) or pick a PR; on confirm runs `git worktree add` via the workspace.
//!
//! The `Render` impl lives in `worktree_dialog/view.rs`.

use okena_core::api::ActionRequest;
use okena_transport::remote_action::RemoteActionClient;

use crate::simple_input::{InputChangedEvent, SimpleInputState};
use list_selection::ListSelection;
use pr_picker::PrPicker;

use gpui::prelude::*;
use gpui::*;
mod list_selection;
mod pr_picker;
mod view;

/// Which tab of the dialog is active.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Branch,
    Pr,
}

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
    mode: Mode,
    pub(super) branches: Vec<String>,
    /// Branches matching the search input.
    branch_selection: ListSelection<String, String>,
    pub(super) branch_search_input: Entity<SimpleInputState>,
    pr_picker: Entity<PrPicker>,
    pub(super) error_message: Option<String>,
    pub(super) loading_branches: bool,
    pub(super) focus_handle: FocusHandle,
    pub(super) initialized: bool,
    _branch_search_subscription: Subscription,
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

        let branch_search_subscription = cx.subscribe(
            &branch_search_input,
            |this, _, _: &InputChangedEvent, cx| {
                this.filter_branches(cx);
                cx.notify();
            },
        );
        let pr_picker = cx.new(|cx| PrPicker::new(client.clone(), daemon_project_id.clone(), cx));

        let focus_handle = cx.focus_handle();

        let mut dialog = Self {
            client,
            daemon_project_id,
            project_id,
            mode: Mode::Branch,
            branches: Vec::new(),
            branch_selection: ListSelection::new(String::clone),
            branch_search_input,
            pr_picker,
            error_message: None,
            loading_branches: true,
            focus_handle,
            initialized: false,
            _branch_search_subscription: branch_search_subscription,
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
                            this.branches = branches;
                            this.filter_branches(cx);
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

    /// Show the branches matching the search input. A new filter drops the
    /// selection: with nothing selected, the typed text names a new branch.
    fn filter_branches(&mut self, cx: &App) {
        let query = self.branch_search_input.read(cx).value().to_lowercase();
        let matching = self
            .branches
            .iter()
            .filter(|branch| branch.to_lowercase().contains(&query))
            .cloned()
            .collect();
        self.branch_selection.set_items(matching);
        self.branch_selection.clear();
    }

    /// Switch tabs and focus the active tab's input.
    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        self.error_message = None;
        match mode {
            Mode::Branch => self
                .branch_search_input
                .update(cx, |input, cx| input.focus(window, cx)),
            Mode::Pr => self.pr_picker.update(cx, |picker, cx| {
                picker.ensure_loaded(cx);
                picker.input.update(cx, |input, cx| input.focus(window, cx));
            }),
        }
        cx.notify();
    }

    fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Branch => {
                if down {
                    self.branch_selection.move_down();
                } else {
                    self.branch_selection.move_up();
                }
                cx.notify();
            }
            Mode::Pr => self.pr_picker.update(cx, |picker, cx| {
                if down {
                    picker.move_down(cx);
                } else {
                    picker.move_up(cx);
                }
            }),
        }
    }

    pub(super) fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorktreeDialogEvent::Close);
    }

    pub(super) fn create_worktree(&mut self, cx: &mut Context<Self>) {
        let picked = match self.mode {
            Mode::Pr => self
                .pr_picker
                .read(cx)
                .picked_branch()
                .map(|branch| (branch, false))
                .map_err(str::to_string),
            Mode::Branch => self.picked_branch(cx),
        };
        let (branch, create_branch) = match picked {
            Ok(picked) => picked,
            Err(message) => {
                self.error_message = Some(message);
                cx.notify();
                return;
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

    /// The selected branch, or else the typed text — an existing branch if it
    /// names one exactly, otherwise a branch to create.
    fn picked_branch(&self, cx: &App) -> Result<(String, bool), String> {
        if let Some(branch) = self.branch_selection.selected() {
            return Ok((branch.clone(), false));
        }
        let name = self.branch_search_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return Err("Please select a branch or type a new branch name".to_string());
        }
        let create_branch = !self.branches.contains(&name);
        Ok((name, create_branch))
    }
}
