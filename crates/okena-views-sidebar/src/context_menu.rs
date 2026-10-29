//! Project context menu overlay.

use crate::Cancel;
use gpui::prelude::*;
use gpui::*;
use okena_ui::menu::{context_menu_panel, menu_item, menu_item_with_color, menu_separator};
use okena_ui::theme::theme;
use okena_workspace::requests::ContextMenuRequest;
use okena_workspace::state::{WindowId, Workspace};

/// Pick the hide/show menu label given (a) whether any extra windows exist and
/// (b) whether the project is currently hidden in the window hosting the menu.
///
/// ADR `docs/decisions/0002-window-as-viewport.md`
/// criteria 1 + 2: single-window users keep the legacy "Hide Project" /
/// "Show Project" labels (no learning tax — user story 30); when at least one
/// extra window exists the per-window scope becomes explicit via
/// "Hide from this window" / "Show in this window". The `is_hidden_in_window`
/// half flips the verb so a click whose effect is to unhide reads as "Show"
/// and a click whose effect is to hide reads as "Hide".
pub(crate) fn hide_project_menu_label(
    extras_exist: bool,
    is_hidden_in_window: bool,
) -> &'static str {
    match (extras_exist, is_hidden_in_window) {
        (false, false) => "Hide Project",
        (false, true) => "Show Project",
        (true, false) => "Hide from this window",
        (true, true) => "Show in this window",
    }
}

/// A window offered as a "Move to window" target: positional name (windows
/// are not named, ADR-0002) plus the projects it shows, so it can be told apart.
pub(crate) fn move_target_label(window_name: &str, project_names: &[&str]) -> String {
    const SHOWN: usize = 3;
    match project_names {
        [] => format!("{window_name} (empty)"),
        names if names.len() <= SHOWN => format!("{window_name}: {}", names.join(", ")),
        names => format!(
            "{window_name}: {} +{}",
            names[..SHOWN].join(", "),
            names.len() - SHOWN
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{hide_project_menu_label, move_target_label};

    #[test]
    fn move_target_label_lists_shown_projects() {
        assert_eq!(move_target_label("Window 2", &[]), "Window 2 (empty)");
        assert_eq!(
            move_target_label("Main window", &["api", "web"]),
            "Main window: api, web"
        );
        assert_eq!(
            move_target_label("Window 3", &["a", "b", "c", "d", "e"]),
            "Window 3: a, b, c +2"
        );
    }

    #[test]
    fn single_window_visible_reads_hide_project() {
        assert_eq!(hide_project_menu_label(false, false), "Hide Project");
    }

    #[test]
    fn single_window_hidden_reads_show_project() {
        assert_eq!(hide_project_menu_label(false, true), "Show Project");
    }

    #[test]
    fn multi_window_visible_reads_hide_from_this_window() {
        assert_eq!(
            hide_project_menu_label(true, false),
            "Hide from this window"
        );
    }

    #[test]
    fn multi_window_hidden_reads_show_in_this_window() {
        assert_eq!(hide_project_menu_label(true, true), "Show in this window");
    }
}

/// Event emitted by ContextMenu
pub enum ContextMenuEvent {
    Close,
    AddTerminal {
        project_id: String,
    },
    CreateWorktree {
        project_id: String,
    },
    QuickCreateWorktree {
        project_id: String,
    },
    ManageWorktrees {
        project_id: String,
        position: gpui::Point<gpui::Pixels>,
    },
    RenameProject {
        project_id: String,
        project_name: String,
    },
    /// Sort the project into a status board column (`None` = unassigned).
    SetStatus {
        project_id: String,
        status: Option<String>,
    },
    /// Flip the status-lanes view of this project's family in this window.
    ToggleStatusLanes {
        project_id: String,
    },
    RenameDirectory {
        project_id: String,
        project_path: String,
    },
    ChangeProjectPath {
        project_id: String,
        project_path: String,
        /// Whether the project's path is on this machine's disk, so the dialog
        /// knows if it may check the typed path itself.
        shares_local_filesystem: bool,
    },
    CloseWorktree {
        project_id: String,
    },
    DeleteProject {
        project_id: String,
    },
    ConfigureHooks {
        project_id: String,
    },
    ReloadServices {
        project_id: String,
    },
    FocusParent {
        project_id: String,
    },
    CopyPath {
        path: String,
    },
    BrowseFiles {
        project_id: String,
    },
    ShowDiff {
        project_id: String,
    },
    FocusProject {
        project_id: String,
    },
    HideProject {
        project_id: String,
    },
    /// Show the project in `target` and hide it in this window. `None` opens
    /// a new window for it.
    MoveToWindow {
        project_id: String,
        target: Option<WindowId>,
    },
    ToggleProjectPinned {
        project_id: String,
    },
}

impl okena_ui::overlay::CloseEvent for ContextMenuEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

/// Project context menu component
pub struct ContextMenu {
    /// Identifies the window-scoped slot on the shared `Workspace` this menu
    /// addresses. Read at render time to (a) look up the project's hidden
    /// state in this window's `hidden_project_ids` and (b) emit a toggle that
    /// targets the same window via the existing `ToggleProjectVisibility`
    /// event flow. Mirrors `FolderContextMenu::window_id`.
    window_id: WindowId,
    workspace: Entity<Workspace>,
    request: ContextMenuRequest,
    focus_handle: FocusHandle,
    submenus: okena_ui::submenu::SubmenuState,
}

impl okena_ui::submenu::SubmenuHost for ContextMenu {
    fn submenu_state(&mut self) -> &mut okena_ui::submenu::SubmenuState {
        &mut self.submenus
    }
}

impl ContextMenu {
    pub fn new(
        window_id: WindowId,
        workspace: Entity<Workspace>,
        request: ContextMenuRequest,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            window_id,
            workspace,
            request,
            focus_handle,
            submenus: okena_ui::submenu::SubmenuState::default(),
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

    fn create_worktree(&self, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::CreateWorktree {
            project_id: self.request.project_id.clone(),
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

    fn change_project_path(
        &self,
        project_path: String,
        shares_local_filesystem: bool,
        cx: &mut Context<Self>,
    ) {
        cx.emit(ContextMenuEvent::ChangeProjectPath {
            project_id: self.request.project_id.clone(),
            project_path,
            shares_local_filesystem,
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

    /// Toggle the project's pinned state. The daemon owns the authoritative
    /// `ProjectData.pinned` flag, so emit an event that routes up to
    /// `WindowView`, which dispatches `ActionRequest::ToggleProjectPinned`; the
    /// new pinned state mirrors back. The GUI must not mutate its read-only
    /// mirror directly.
    fn toggle_pinned(&self, cx: &mut Context<Self>) {
        let project_id = self.request.project_id.clone();
        cx.emit(ContextMenuEvent::ToggleProjectPinned { project_id });
        self.close(cx);
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

    fn browse_files(&self, project_id: String, cx: &mut Context<Self>) {
        cx.emit(ContextMenuEvent::BrowseFiles { project_id });
    }

    fn open_in_file_manager(&self, path: &str, cx: &mut Context<Self>) {
        cx.open_with_system(std::path::Path::new(path));
        self.close(cx);
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
        // Seed the rename with the label the sidebar shows (a worktree's live
        // branch), not the stored name it may be hiding.
        let branch = ws
            .remote_snapshot(&self.request.project_id)
            .and_then(|s| s.git_status.as_ref())
            .and_then(|g| g.branch.as_deref());
        let project_name = project
            .map(|p| crate::sidebar::SidebarProjectInfo::display_name(p, branch))
            .unwrap_or_default();
        let project_path = project.map(|p| p.path.clone()).unwrap_or_default();
        let is_worktree = project.map(|p| p.worktree_info.is_some()).unwrap_or(false);
        let is_pinned = project.map(|p| p.pinned).unwrap_or(false);
        let is_git_repo = ws
            .remote_snapshot(&self.request.project_id)
            .and_then(|s| s.git_status.as_ref())
            .is_some();
        // Not `is_remote`: every project mirrored from a daemon connection
        // carries that, including the local daemon a `--daemon-client` desktop
        // talks to — i.e. most projects on a normal setup. What the change-path
        // dialog needs to know is whether the project's path lives on *this*
        // disk, so it can pre-validate against it. That is the connection,
        // mirroring `ActionDispatcher::shares_local_filesystem`.
        let shares_local_filesystem = project
            .map(|p| {
                p.connection_id
                    .as_deref()
                    .is_none_or(|id| id == okena_transport::client::LOCAL_DAEMON_CONNECTION_ID)
            })
            .unwrap_or(false);
        let project_path_for_rename_dir = project_path.clone();
        let project_path_for_change_path = project_path.clone();
        let project_name_for_rename = project_name.clone();
        let extras_exist = !ws.data().extra_windows.is_empty();
        let is_hidden_in_window = ws
            .data()
            .window(self.window_id)
            .map(|w| w.hidden_project_ids.contains(&self.request.project_id))
            .unwrap_or(false);
        let hide_project_label = hide_project_menu_label(extras_exist, is_hidden_in_window);
        let hide_project_icon = if is_hidden_in_window {
            "icons/eye.svg"
        } else {
            "icons/eye-off.svg"
        };
        let current_status = project.and_then(|p| p.status.clone());

        // Status board column: the current one ticked, "No status" last and
        // ticked when the project has none (or a column that no longer exists).
        let board_columns = okena_ui::board::board_columns(cx);
        let known = current_status
            .as_deref()
            .is_some_and(|s| board_columns.iter().any(|c| c.id == s));
        let mut status_items: Vec<okena_ui::submenu::SubmenuItem> = board_columns
            .iter()
            .map(|column| {
                let selected = current_status.as_deref() == Some(column.id.as_str());
                let status = Some(column.id.clone());
                okena_ui::submenu::SubmenuItem::new(
                    format!("context-menu-status-{}", column.id),
                    if selected {
                        "icons/check.svg"
                    } else {
                        "icons/focus.svg"
                    },
                    column.name.clone(),
                    cx.listener(move |this: &mut Self, _, _window, cx| {
                        cx.emit(ContextMenuEvent::SetStatus {
                            project_id: this.request.project_id.clone(),
                            status: status.clone(),
                        });
                    }),
                )
            })
            .collect();
        status_items.push(okena_ui::submenu::SubmenuItem::new(
            "context-menu-status-none",
            if known {
                "icons/focus.svg"
            } else {
                "icons/check.svg"
            },
            "No status",
            cx.listener(|this: &mut Self, _, _window, cx| {
                cx.emit(ContextMenuEvent::SetStatus {
                    project_id: this.request.project_id.clone(),
                    status: None,
                });
            }),
        ));
        let status_label = match current_status
            .as_deref()
            .and_then(|id| board_columns.iter().find(|c| c.id == id))
        {
            Some(column) => format!("Status: {}", column.name),
            None => "Status".to_string(),
        };
        let status_board = okena_ui::board::status_board_enabled(cx);
        let lanes_on = ws.status_lanes(self.window_id, &self.request.project_id);

        // Every other open window, positionally named, then a fresh window.
        let other_windows = std::iter::once(WindowId::Main)
            .chain(
                ws.data()
                    .extra_windows
                    .iter()
                    .map(|w| WindowId::Extra(w.id)),
            )
            .enumerate()
            .filter(|(_, id)| *id != self.window_id)
            .map(|(index, id)| {
                let name = match id {
                    WindowId::Main => "Main window".to_string(),
                    WindowId::Extra(_) => format!("Window {}", index + 1),
                };
                let projects: Vec<&str> = ws
                    .visible_projects(id, None, false)
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect();
                (id, move_target_label(&name, &projects))
            })
            .collect::<Vec<_>>();
        let new_window_label = if other_windows.is_empty() {
            "Move to new window"
        } else {
            "New window"
        };
        let mut move_items: Vec<okena_ui::submenu::SubmenuItem> = other_windows
            .into_iter()
            .map(|(id, label)| {
                okena_ui::submenu::SubmenuItem::new(
                    format!("context-menu-move-to-{id:?}"),
                    "icons/external-link.svg",
                    label,
                    cx.listener(move |this: &mut Self, _, _window, cx| {
                        cx.emit(ContextMenuEvent::MoveToWindow {
                            project_id: this.request.project_id.clone(),
                            target: Some(id),
                        });
                    }),
                )
            })
            .collect();
        move_items.push(okena_ui::submenu::SubmenuItem::new(
            "context-menu-move-to-new-window",
            "icons/plus.svg",
            new_window_label,
            cx.listener(|this: &mut Self, _, _window, cx| {
                cx.emit(ContextMenuEvent::MoveToWindow {
                    project_id: this.request.project_id.clone(),
                    target: None,
                });
            }),
        ));
        let move_group = okena_ui::submenu::Submenu::new(
            "context-menu-move-to-window",
            "icons/external-link.svg",
            "Move to window",
        )
        .items(move_items)
        .render(&mut self.submenus, &t, window, cx);

        let status_group = okena_ui::submenu::Submenu::new(
            "context-menu-status",
            "icons/arrow-up-down.svg",
            status_label,
        )
        .items(status_items)
        .render(&mut self.submenus, &t, window, cx);

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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(deferred(
                anchored().position(position).snap_to_window().child(
                    context_menu_panel("project-context-menu", &t)
                        // Add Terminal option
                        .child(
                            menu_item(
                                "context-menu-add-terminal",
                                "icons/plus.svg",
                                "Add Terminal",
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    this.add_terminal(cx);
                                },
                            )),
                        )
                        // Browse Files
                        .child(
                            menu_item(
                                "context-menu-browse-files",
                                "icons/file.svg",
                                "Browse Files",
                                &t,
                            )
                            .on_click(cx.listener({
                                let project_id = self.request.project_id.clone();
                                move |this, _, _window, cx| {
                                    this.browse_files(project_id.clone(), cx);
                                }
                            })),
                        )
                        .when(shares_local_filesystem, |d| {
                            d.child(
                                menu_item(
                                    "context-menu-open-directory",
                                    "icons/folder.svg",
                                    "Open in file manager",
                                    &t,
                                )
                                .on_click(cx.listener({
                                    let project_path = project_path.clone();
                                    move |this, _, _window, cx| {
                                        this.open_in_file_manager(&project_path, cx);
                                    }
                                })),
                            )
                        })
                        // Show Diff
                        .when(is_git_repo, |d| {
                            d.child(
                                menu_item(
                                    "context-menu-show-diff",
                                    "icons/git-commit.svg",
                                    "Show Diff",
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        this.show_diff(cx);
                                    },
                                )),
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
                            menu_item(
                                "context-menu-focus-project",
                                "icons/fullscreen.svg",
                                "Focus Project",
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    this.focus_project(cx);
                                },
                            )),
                        )
                        // Pin / Unpin (anchors the project to the top of the
                        // activity-sorted view; harmless in manual mode)
                        .child(
                            menu_item(
                                "context-menu-toggle-pinned",
                                "icons/bookmark.svg",
                                if is_pinned { "Unpin" } else { "Pin to top" },
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    this.toggle_pinned(cx);
                                },
                            )),
                        )
                        // Hide / Show Project (label + icon depend on extras presence and per-window hidden state)
                        .child(
                            menu_item(
                                "context-menu-hide-project",
                                hide_project_icon,
                                hide_project_label,
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    this.hide_project(cx);
                                },
                            )),
                        )
                        .child(move_group)
                        .child(menu_separator(&t))
                        // Create Worktree option (only for git repos that are not already worktrees)
                        .when(is_git_repo && !is_worktree, |d| {
                            d.child(
                                menu_item(
                                    "context-menu-create-worktree",
                                    "icons/git-branch.svg",
                                    "Create Worktree...",
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        this.create_worktree(cx);
                                    },
                                )),
                            )
                        })
                        // Quick Create Worktree (only for git repos that are not already worktrees)
                        .when(is_git_repo && !is_worktree, |d| {
                            d.child(
                                menu_item(
                                    "context-menu-quick-create-wt",
                                    "icons/plus.svg",
                                    "Quick Create Worktree",
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        this.quick_create_worktree(cx);
                                    },
                                )),
                            )
                        })
                        // Manage Worktrees (only for git repos that are not already worktrees)
                        .when(is_git_repo && !is_worktree, |d| {
                            d.child(
                                menu_item(
                                    "context-menu-manage-wt",
                                    "icons/git-branch.svg",
                                    "Manage Worktrees",
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        this.manage_worktrees(cx);
                                    },
                                )),
                            )
                        })
                        // Separator (only if worktree items above were shown)
                        .when(is_git_repo && !is_worktree, |d| d.child(menu_separator(&t)))
                        .when(status_board, |menu| {
                            menu.child(status_group).child(
                                menu_item(
                                    "context-menu-status-lanes",
                                    if lanes_on {
                                        "icons/check.svg"
                                    } else {
                                        "icons/kanban.svg"
                                    },
                                    "Status lanes",
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        cx.emit(ContextMenuEvent::ToggleStatusLanes {
                                            project_id: this.request.project_id.clone(),
                                        });
                                    },
                                )),
                            )
                        })
                        // Rename option
                        .child(
                            menu_item(
                                "context-menu-rename",
                                "icons/edit.svg",
                                "Rename Project",
                                &t,
                            )
                            .on_click(cx.listener({
                                let project_name = project_name_for_rename.clone();
                                move |this, _, _window, cx| {
                                    this.rename_project(project_name.clone(), cx);
                                }
                            })),
                        )
                        // Rename Directory option
                        .child(
                            menu_item(
                                "context-menu-rename-dir",
                                "icons/folder.svg",
                                "Rename Directory...",
                                &t,
                            )
                            .on_click(cx.listener({
                                let project_path = project_path_for_rename_dir.clone();
                                move |this, _, _window, cx| {
                                    this.rename_directory(project_path.clone(), cx);
                                }
                            })),
                        )
                        // Change Folder Path option. Offered for remote projects
                        // too — the daemon that owns one validates the target
                        // against its own disk. Only this dialog's *local*
                        // pre-checks have to stand down, hence the flag.
                        .child(
                            menu_item(
                                "context-menu-change-path",
                                "icons/folder.svg",
                                "Change Folder Path...",
                                &t,
                            )
                            .on_click(cx.listener({
                                let project_path = project_path_for_change_path.clone();
                                move |this, _, _window, cx| {
                                    this.change_project_path(
                                        project_path.clone(),
                                        shares_local_filesystem,
                                        cx,
                                    );
                                }
                            })),
                        )
                        // Configure Hooks option
                        .child(
                            menu_item(
                                "context-menu-configure-hooks",
                                "icons/terminal.svg",
                                "Configure Hooks...",
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    this.configure_hooks(cx);
                                },
                            )),
                        )
                        // Reload Services option
                        .child(
                            menu_item(
                                "context-menu-reload-services",
                                "icons/file.svg",
                                "Reload Services",
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    this.reload_services(cx);
                                },
                            )),
                        )
                        // Focus Parent Project option (only for worktree projects)
                        .when(is_worktree, |d| {
                            d.child(
                                menu_item(
                                    "context-menu-focus-parent",
                                    "icons/chevron-up.svg",
                                    "Focus Parent Project",
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        this.focus_parent(cx);
                                    },
                                )),
                            )
                        })
                        // Close Worktree option (only for worktree projects)
                        .when(is_worktree, |d| {
                            d.child(
                                menu_item_with_color(
                                    "context-menu-close-worktree",
                                    "icons/git-branch.svg",
                                    "Close Worktree",
                                    t.warning,
                                    t.warning,
                                    &t,
                                )
                                .on_click(cx.listener(
                                    |this, _, _window, cx| {
                                        this.close_worktree(cx);
                                    },
                                )),
                            )
                        })
                        // Delete option
                        .child(
                            menu_item_with_color(
                                "context-menu-delete",
                                "icons/trash.svg",
                                "Delete Project",
                                t.error,
                                t.error,
                                &t,
                            )
                            .on_click(cx.listener(
                                |this, _, _window, cx| {
                                    this.delete_project(cx);
                                },
                            )),
                        ),
                ),
            ))
    }
}

okena_ui::impl_focusable!(ContextMenu);
