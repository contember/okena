//! Project context menu overlay.

use crate::git;
use crate::theme::theme;
use crate::views::components::{menu_item, menu_item_with_color};
use crate::workspace::state::{ContextMenuRequest, Workspace};
use gpui::prelude::*;
use gpui::*;

/// Event emitted by ContextMenu
pub enum ContextMenuEvent {
    Close,
    AddTerminal { project_id: String },
    CreateWorktree { project_id: String, project_path: String },
    RenameProject { project_id: String, project_name: String },
    CloseWorktree { project_id: String },
    DeleteProject { project_id: String },
}

/// Project context menu component
pub struct ContextMenu {
    workspace: Entity<Workspace>,
    request: ContextMenuRequest,
    focus_handle: FocusHandle,
}

impl ContextMenu {
    pub fn new(
        workspace: Entity<Workspace>,
        request: ContextMenuRequest,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            workspace,
            request,
            focus_handle,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::Close);
    }

    fn add_terminal(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::AddTerminal {
            project_id: self.request.project_id.clone(),
        });
    }

    fn create_worktree(&self, project_path: String, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::CreateWorktree {
            project_id: self.request.project_id.clone(),
            project_path,
        });
    }

    fn rename_project(&self, project_name: String, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::RenameProject {
            project_id: self.request.project_id.clone(),
            project_name,
        });
    }

    fn close_worktree(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::CloseWorktree {
            project_id: self.request.project_id.clone(),
        });
    }

    fn delete_project(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::DeleteProject {
            project_id: self.request.project_id.clone(),
        });
    }
}

impl EventEmitter<ContextMenuEvent> for ContextMenu {}

impl Render for ContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Focus on first render
        window.focus(&self.focus_handle, cx);

        let position = self.request.position;

        // Get project info
        let ws = self.workspace.read(cx);
        let project = ws.project(&self.request.project_id);
        let project_name = project.map(|p| p.name.clone()).unwrap_or_default();
        let project_path = project.map(|p| p.path.clone()).unwrap_or_default();
        let is_worktree = project.map(|p| p.worktree_info.is_some()).unwrap_or(false);
        let is_git_repo = git::get_git_status(std::path::Path::new(&project_path)).is_some();

        let project_path_for_worktree = project_path.clone();
        let project_name_for_rename = project_name.clone();

        div()
            .track_focus(&self.focus_handle)
            .key_context("ContextMenu")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key.as_str() == "escape" {
                    this.close(cx);
                }
            }))
            .absolute()
            .inset_0()
            .id("context-menu-backdrop")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .child(
                div()
                    .absolute()
                    .left(position.x)
                    .top(position.y)
                    .bg(rgb(t.bg_primary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(4.0))
                    .shadow_xl()
                    .min_w(px(160.0))
                    .py(px(4.0))
                    .id("project-context-menu")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    // Add Terminal option
                    .child(
                        menu_item("context-menu-add-terminal", "icons/plus.svg", "Add Terminal", &t)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.add_terminal(cx);
                            })),
                    )
                    // Create Worktree option (only for git repos that are not already worktrees)
                    .when(is_git_repo && !is_worktree, |d| {
                        d.child(
                            menu_item("context-menu-create-worktree", "icons/git-branch.svg", "Create Worktree...", &t)
                                .on_click(cx.listener({
                                    let project_path = project_path_for_worktree.clone();
                                    move |this, _, _window, cx| {
                                        this.create_worktree(project_path.clone(), cx);
                                    }
                                })),
                        )
                    })
                    // Separator
                    .child(
                        div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(rgb(t.border)),
                    )
                    // Rename option
                    .child(
                        menu_item("context-menu-rename", "icons/edit.svg", "Rename Project", &t)
                            .on_click(cx.listener({
                                let project_name = project_name_for_rename.clone();
                                move |this, _, _window, cx| {
                                    this.rename_project(project_name.clone(), cx);
                                }
                            })),
                    )
                    // Close Worktree option (only for worktree projects)
                    .when(is_worktree, |d| {
                        d.child(
                            menu_item_with_color("context-menu-close-worktree", "icons/git-branch.svg", "Close Worktree", t.warning, t.warning, &t)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.close_worktree(cx);
                                })),
                        )
                    })
                    // Delete option
                    .child(
                        menu_item_with_color("context-menu-delete", "icons/trash.svg", "Delete Project", t.error, t.error, &t)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.delete_project(cx);
                            })),
                    ),
            )
    }
}

impl_focusable!(ContextMenu);
