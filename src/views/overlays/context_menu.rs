//! Project context menu overlay.

use crate::git;
use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::views::components::{context_menu_panel, menu_item, menu_item_with_color, menu_separator};
use crate::workspace::requests::ContextMenuRequest;
use crate::workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;

/// Event emitted by ContextMenu
pub enum ContextMenuEvent {
    Close,
    AddTerminal { project_id: String },
    CreateWorktree { project_id: String, project_path: String },
    RenameProject { project_id: String, project_name: String },
    RenameDirectory { project_id: String, project_path: String },
    CloseWorktree { project_id: String },
    CloseAllWorktrees { project_id: String },
    DeleteProject { project_id: String },
    ConfigureHooks { project_id: String },
    FocusParent { project_id: String },
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

    fn rename_directory(&self, project_path: String, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::RenameDirectory {
            project_id: self.request.project_id.clone(),
            project_path,
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

    fn configure_hooks(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::ConfigureHooks {
            project_id: self.request.project_id.clone(),
        });
    }

    fn close_all_worktrees(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::CloseAllWorktrees {
            project_id: self.request.project_id.clone(),
        });
    }

    fn focus_parent(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::FocusParent {
            project_id: self.request.project_id.clone(),
        });
    }
}

impl EventEmitter<ContextMenuEvent> for ContextMenu {}

impl Render for ContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Focus on first render
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let position = self.request.position;

        // Get project info
        let ws = self.workspace.read(cx);
        let project = ws.project(&self.request.project_id);
        let project_name = project.map(|p| p.name.clone()).unwrap_or_default();
        let project_path = project.map(|p| p.path.clone()).unwrap_or_default();
        let is_worktree = project.map(|p| p.worktree_info.is_some()).unwrap_or(false);
        let is_git_repo = git::get_git_status(std::path::Path::new(&project_path)).is_some();
        let worktree_count = ws.data().projects.iter()
            .filter(|p| p.worktree_info.as_ref().map_or(false, |wt| wt.parent_project_id == self.request.project_id))
            .count();

        let project_path_for_worktree = project_path.clone();
        let project_path_for_rename_dir = project_path.clone();
        let project_name_for_rename = project_name.clone();

        div()
            .track_focus(&self.focus_handle)
            .key_context("ContextMenu")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .absolute()
            .inset_0()
            .occlude()
            .id("context-menu-backdrop")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .child(deferred(
                anchored()
                    .position(position)
                    .snap_to_window()
                    .child(
                        context_menu_panel("project-context-menu", &t)
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
                    // Close All Worktrees option (only for git repos that are not worktrees and have child worktrees)
                    .when(is_git_repo && !is_worktree && worktree_count > 0, |d| {
                        d.child(
                            menu_item_with_color(
                                "context-menu-close-all-worktrees",
                                "icons/git-branch.svg",
                                &format!("Close All Worktrees ({})", worktree_count),
                                t.warning, t.warning, &t,
                            )
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.close_all_worktrees(cx);
                            }))
                        )
                    })
                    // Separator
                    .child(menu_separator(&t))
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
                    // Rename Directory option
                    .child(
                        menu_item("context-menu-rename-dir", "icons/folder.svg", "Rename Directory...", &t)
                            .on_click(cx.listener({
                                let project_path = project_path_for_rename_dir.clone();
                                move |this, _, _window, cx| {
                                    this.rename_directory(project_path.clone(), cx);
                                }
                            })),
                    )
                    // Configure Hooks option
                    .child(
                        menu_item("context-menu-configure-hooks", "icons/terminal.svg", "Configure Hooks...", &t)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.configure_hooks(cx);
                            })),
                    )
                    // Focus Parent Project option (only for worktree projects)
                    .when(is_worktree, |d| {
                        d.child(
                            menu_item("context-menu-focus-parent", "icons/chevron-up.svg", "Focus Parent Project", &t)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.focus_parent(cx);
                                })),
                        )
                    })
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
                ),
            ))
    }
}

impl_focusable!(ContextMenu);
