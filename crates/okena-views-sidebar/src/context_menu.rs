//! Project context menu overlay.

use crate::Cancel;
use okena_git;
use okena_ui::menu::{context_menu_panel, menu_item, menu_item_with_color, menu_separator};
use okena_ui::theme::theme;
use okena_workspace::requests::ContextMenuRequest;
use okena_workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;

/// Event emitted by ContextMenu
pub enum ContextMenuEvent {
    Close,
    AddTerminal { project_id: String },
    CreateWorktree { project_id: String, project_path: String },
    QuickCreateWorktree { project_id: String },
    ManageWorktrees { project_id: String, position: gpui::Point<gpui::Pixels> },
    RenameProject { project_id: String, project_name: String },
    RenameDirectory { project_id: String, project_path: String },
    CloseWorktree { project_id: String },
    DeleteProject { project_id: String },
    ConfigureHooks { project_id: String },
    ReloadServices { project_id: String },
    FocusParent { project_id: String },
    CopyPath { path: String },
    BrowseFiles { project_path: String },
    ShowDiff { project_id: String },
    FocusProject { project_id: String },
    HideProject { project_id: String },
}

impl okena_ui::overlay::CloseEvent for ContextMenuEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Close) }
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

    fn quick_create_worktree(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::QuickCreateWorktree {
            project_id: self.request.project_id.clone(),
        });
    }

    fn manage_worktrees(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::ManageWorktrees {
            project_id: self.request.project_id.clone(),
            position: self.request.position,
        });
    }

    fn reload_services(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::ReloadServices {
            project_id: self.request.project_id.clone(),
        });
    }

    fn focus_parent(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::FocusParent {
            project_id: self.request.project_id.clone(),
        });
    }

    fn copy_path(&self, path: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path));
        cx.emit(ContextMenuEvent::Close);
    }

    fn browse_files(&self, project_path: String, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::BrowseFiles { project_path });
    }

    fn show_diff(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::ShowDiff {
            project_id: self.request.project_id.clone(),
        });
    }

    fn focus_project(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::FocusProject {
            project_id: self.request.project_id.clone(),
        });
    }

    fn hide_project(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::HideProject {
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
        let is_git_repo = okena_git::is_git_repo(std::path::Path::new(&project_path));
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
                    // Browse Files
                    .child(
                        menu_item("context-menu-browse-files", "icons/file.svg", "Browse Files", &t)
                            .on_click(cx.listener({
                                let project_path = project_path.clone();
                                move |this, _, _window, cx| {
                                    this.browse_files(project_path.clone(), cx);
                                }
                            })),
                    )
                    // Show Diff
                    .when(is_git_repo, |d| {
                        d.child(
                            menu_item("context-menu-show-diff", "icons/git-commit.svg", "Show Diff", &t)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.show_diff(cx);
                                })),
                        )
                    })
                    .child(menu_separator(&t))
                    // Copy Path
                    .child(
                        menu_item("context-menu-copy-path", "icons/copy.svg", "Copy Path", &t)
                            .on_click(cx.listener({
                                let project_path = project_path.clone();
                                move |this, _, _window, cx| {
                                    this.copy_path(project_path.clone(), cx);
                                }
                            })),
                    )
                    // Focus Project
                    .child(
                        menu_item("context-menu-focus-project", "icons/fullscreen.svg", "Focus Project", &t)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.focus_project(cx);
                            })),
                    )
                    // Hide Project
                    .child(
                        menu_item("context-menu-hide-project", "icons/eye-off.svg", "Hide Project", &t)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.hide_project(cx);
                            })),
                    )
                    .child(menu_separator(&t))
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
                    // Quick Create Worktree (only for git repos that are not already worktrees)
                    .when(is_git_repo && !is_worktree, |d| {
                        d.child(
                            menu_item("context-menu-quick-create-wt", "icons/plus.svg", "Quick Create Worktree", &t)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.quick_create_worktree(cx);
                                })),
                        )
                    })
                    // Manage Worktrees (only for git repos that are not already worktrees)
                    .when(is_git_repo && !is_worktree, |d| {
                        d.child(
                            menu_item("context-menu-manage-wt", "icons/git-branch.svg", "Manage Worktrees", &t)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.manage_worktrees(cx);
                                })),
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
                    // Reload Services option
                    .child(
                        menu_item("context-menu-reload-services", "icons/file.svg", "Reload Services", &t)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.reload_services(cx);
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

okena_ui::impl_focusable!(ContextMenu);
