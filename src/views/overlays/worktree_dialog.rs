use crate::git;
use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::views::components::{button, button_primary, input_container};
use crate::views::simple_input::{SimpleInput, SimpleInputState};
use crate::workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use std::path::PathBuf;

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
    workspace: Entity<Workspace>,
    project_id: String,
    project_path: String,
    branches: Vec<String>,
    filtered_branches: Vec<usize>,
    selected_branch_index: Option<usize>,
    branch_search_input: Entity<SimpleInputState>,
    error_message: Option<String>,
    focus_handle: FocusHandle,
    initialized: bool,
    last_search_query: String,
}

impl WorktreeDialog {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        project_path: String,
        cx: &mut Context<Self>,
    ) -> Self {
        // Get available branches
        let repo_path = PathBuf::from(&project_path);
        let branches = git::get_available_branches_for_worktree(&repo_path);

        let branch_search_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Search or create branch...")
                .icon("icons/search.svg")
        });

        let filtered_branches: Vec<usize> = (0..branches.len()).collect();
        let focus_handle = cx.focus_handle();

        Self {
            workspace,
            project_id,
            project_path,
            branches,
            filtered_branches,
            selected_branch_index: None,
            branch_search_input,
            error_message: None,
            focus_handle,
            initialized: false,
            last_search_query: String::new(),
        }
    }

    fn filter_branches(&mut self, cx: &App) {
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

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorktreeDialogEvent::Close);
    }

    fn get_target_path(&self, branch: &str) -> String {
        let base_path = PathBuf::from(&self.project_path);
        let parent = base_path.parent().unwrap_or(&base_path);
        let repo_name = base_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("repo");
        let safe_branch = branch.replace('/', "-");
        parent.join(format!("{}-wt", repo_name))
            .join(&safe_branch)
            .to_string_lossy()
            .to_string()
    }

    fn create_worktree(&mut self, cx: &mut Context<Self>) {
        let (branch, create_branch) = if let Some(filtered_idx) = self.selected_branch_index {
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

        let target_path = self.get_target_path(&branch);
        let project_id = self.project_id.clone();

        // Create the worktree project
        let result = self.workspace.update(cx, |ws, cx| {
            ws.create_worktree_project(&project_id, &branch, &target_path, create_branch, cx)
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

    fn render_branch_list(&self, t: crate::theme::ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let search_empty = self.branch_search_input.read(cx).value().is_empty();

        if self.filtered_branches.is_empty() {
            return div()
                .p(px(12.0))
                .text_size(px(12.0))
                .text_color(rgb(t.text_muted))
                .child(if search_empty {
                    "No available branches for worktree"
                } else {
                    "No branches match — will create new branch"
                })
                .into_any_element();
        }

        div()
            .id("branch-list-scroll")
            .flex()
            .flex_col()
            .max_h(px(200.0))
            .overflow_y_scroll()
            .children(
                self.filtered_branches.iter().enumerate().map(|(filtered_idx, &branch_idx)| {
                    let is_selected = self.selected_branch_index == Some(filtered_idx);
                    let branch_name = self.branches[branch_idx].clone();

                    div()
                        .id(ElementId::Name(format!("branch-{}", filtered_idx).into()))
                        .px(px(12.0))
                        .py(px(6.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(rgb(t.text_primary))
                        .when(is_selected, |d| d.bg(rgb(t.bg_selection)))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .child(
                            svg()
                                .path("icons/git-branch.svg")
                                .size(px(14.0))
                                .text_color(rgb(t.text_secondary))
                        )
                        .child(branch_name)
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.selected_branch_index = Some(filtered_idx);
                            cx.notify();
                        }))
                })
            )
            .into_any_element()
    }
}

impl_focusable!(WorktreeDialog);

impl Render for WorktreeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        // Focus search input on first render
        if !self.initialized {
            self.initialized = true;
            let search_input = self.branch_search_input.clone();
            search_input.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }

        // Filter branches based on search input
        self.filter_branches(cx);

        let branch_search_input = self.branch_search_input.clone();
        let search_input_focused = self.branch_search_input.read(cx).focus_handle(cx).is_focused(window);

        div()
            .id("worktree-dialog-backdrop")
            .track_focus(&focus_handle)
            .key_context("WorktreeDialog")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let search_focused = this.branch_search_input.read(cx).focus_handle(cx).is_focused(window);

                match event.keystroke.key.as_str() {
                    "up" => {
                        if search_focused {
                            if let Some(idx) = this.selected_branch_index {
                                if idx > 0 {
                                    this.selected_branch_index = Some(idx - 1);
                                    cx.notify();
                                }
                            }
                        }
                    }
                    "down" => {
                        if search_focused {
                            let max = this.filtered_branches.len().saturating_sub(1);
                            if let Some(idx) = this.selected_branch_index {
                                if idx < max {
                                    this.selected_branch_index = Some(idx + 1);
                                    cx.notify();
                                }
                            } else if !this.filtered_branches.is_empty() {
                                this.selected_branch_index = Some(0);
                                cx.notify();
                            }
                        }
                    }
                    "enter" => {
                        this.create_worktree(cx);
                    }
                    _ => {}
                }
            }))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000080))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .child(
                div()
                    .id("worktree-dialog")
                    .w(px(450.0))
                    .max_h(px(550.0))
                    .flex()
                    .flex_col()
                    .bg(rgb(t.bg_primary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    // Header
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        svg()
                                            .path("icons/git-branch.svg")
                                            .size(px(16.0))
                                            .text_color(rgb(t.border_active))
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                            .child("Create Worktree")
                                    )
                            )
                            .child(
                                div()
                                    .id("close-dialog-btn")
                                    .cursor_pointer()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .child(
                                        svg()
                                            .path("icons/close.svg")
                                            .size(px(14.0))
                                            .text_color(rgb(t.text_secondary))
                                    )
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.close(cx);
                                    }))
                            )
                    )
                    // Content
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .px(px(16.0))
                                    .py(px(12.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    // Search input
                                    .child(
                                        input_container(&t, Some(search_input_focused))
                                            .child(SimpleInput::new(&branch_search_input).text_size(px(12.0))),
                                    )
                                    // Branch list
                                    .child(self.render_branch_list(t, cx))
                            )
                    )
                    // Error message
                    .when_some(self.error_message.clone(), |d, msg| {
                        d.child(
                            div()
                                .px(px(16.0))
                                .py(px(8.0))
                                .bg(rgba(0xff00001a))
                                .text_size(px(12.0))
                                .text_color(rgb(t.error))
                                .child(msg)
                        )
                    })
                    // Footer
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .child(
                                button("cancel-btn", "Cancel", &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.close(cx);
                                    })),
                            )
                            .child(
                                button_primary("create-btn", "Create Worktree", &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.create_worktree(cx);
                                    })),
                            ),
                    )
            )
    }
}
