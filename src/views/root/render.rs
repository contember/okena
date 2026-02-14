use crate::keybindings::{ShowKeybindings, ShowSessionManager, ShowThemeSelector, ShowCommandPalette, ShowSettings, OpenSettingsFile, ShowFileSearch, ShowProjectSwitcher, ShowDiffViewer, NewProject, ToggleSidebar, ToggleSidebarAutoHide, CreateWorktree, CheckForUpdates, InstallUpdate, FocusSidebar, ShowPairingDialog};
use crate::action_dispatch::ActionDispatcher;
use crate::settings::open_settings_file;
use crate::theme::theme;
use crate::views::layout::navigation::clear_pane_map;
use crate::views::layout::split_pane::{compute_resize, render_project_divider, render_sidebar_divider, DragState};
use crate::views::panels::project_column::ProjectColumn;
use gpui::*;
use gpui::prelude::*;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use super::RootView;

impl RootView {
    pub(super) fn render_projects_grid(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Check if a remote project is focused — render it instead of local grid
        if let Some(ref rm) = self.remote_manager {
            let focused = rm.read(cx).focused_remote()
                .map(|(c, p)| (c.to_string(), p.to_string()));
            if let Some((conn_id, proj_id)) = focused {
                return self.render_remote_project_column(&conn_id, &proj_id, cx).into_any_element();
            }
        }

        // Sync project columns to handle newly added projects
        self.sync_project_columns(cx);

        let visible_projects: Vec<_> = {
            let workspace = self.workspace.read(cx);
            // When zoomed, show only the zoomed project's column
            if let Some(pid) = workspace.focus_manager.fullscreen_project_id() {
                vec![pid.to_string()]
            } else {
                workspace.visible_projects().iter().map(|p| p.id.clone()).collect()
            }
        };

        let num_projects = visible_projects.len();

        // Get widths for each project
        // When only one project is visible (focused), always use 100%
        // Otherwise, normalize widths so they sum to 100%
        let widths: Vec<f32> = if num_projects == 1 {
            vec![100.0]
        } else if num_projects == 0 {
            vec![]
        } else {
            let workspace = self.workspace.read(cx);
            let raw_widths: Vec<f32> = visible_projects.iter()
                .map(|id| workspace.get_project_width(id, num_projects))
                .collect();

            // Normalize widths to sum to 100%
            let total: f32 = raw_widths.iter().sum();
            if total > 0.0 {
                raw_widths.iter().map(|w| w / total * 100.0).collect()
            } else {
                vec![100.0 / num_projects as f32; num_projects]
            }
        };

        // Shared bounds reference for resize calculation
        let container_bounds = Rc::new(RefCell::new(Bounds {
            origin: Point::default(),
            size: Size { width: px(800.0), height: px(600.0) },
        }));

        // Build interleaved columns and dividers
        let mut elements: Vec<AnyElement> = Vec::new();

        for (i, project_id) in visible_projects.iter().enumerate() {
            let width_percent = widths.get(i).copied().unwrap_or(100.0 / num_projects as f32);

            if let Some(col) = self.project_columns.get(project_id).cloned() {
                let col_element = div()
                    .flex_basis(relative(width_percent / 100.0))
                    .h_full()
                    .min_w(px(200.0))
                    .child(col)
                    .into_any_element();

                elements.push(col_element);

                // Add divider after each column except the last
                if i < num_projects - 1 {
                    let divider = render_project_divider(
                        self.workspace.clone(),
                        i,
                        visible_projects.clone(),
                        container_bounds.clone(),
                        &self.active_drag,
                        cx,
                    );
                    elements.push(divider.into_any_element());
                }
            }
        }

        div()
            .id("projects-grid")
            .flex_1()
            .h_full()
            .flex()
            .overflow_hidden()
            // Canvas to capture container bounds
            .child(canvas(
                {
                    let container_bounds = container_bounds.clone();
                    move |bounds, _window, _cx| {
                        *container_bounds.borrow_mut() = bounds;
                    }
                },
                |_bounds, _prepaint, _window, _cx| {},
            ).absolute().size_full())
            // Mouse handlers are on root div - no need to duplicate here
            .children(elements)
            .into_any_element()
    }

    /// Render a single remote project column when a remote project is focused.
    fn render_remote_project_column(
        &mut self,
        conn_id: &str,
        proj_id: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = format!("{}:{}", conn_id, proj_id);

        // Invalidate cached column if remote state has been updated
        // (re-create on each focus to pick up fresh layout)
        if !self.remote_project_columns.contains_key(&key) {
            if let Some(ref rm) = self.remote_manager {
                let rm_read = rm.read(cx);
                if let (Some(backend), Some(state)) =
                    (rm_read.backend_for(conn_id), rm_read.remote_state(conn_id))
                {
                    if let Some(api_project) =
                        state.projects.iter().find(|p| p.id == proj_id)
                    {
                        let layout = api_project.layout.as_ref().map(|l| {
                            crate::workspace::state::LayoutNode::from_api_prefixed(l, &format!("remote:{}", conn_id))
                        });
                        let workspace = self.workspace.clone();
                        let request_broker = self.request_broker.clone();
                        let terminals = self.terminals.clone();
                        let active_drag = self.active_drag.clone();
                        let pid = proj_id.to_string();
                        let pname = api_project.name.clone();
                        let ppath = api_project.path.clone();
                        let action_dispatcher = self.remote_manager.as_ref().map(|rm| ActionDispatcher::Remote {
                            connection_id: conn_id.to_string(),
                            manager: rm.clone(),
                        });
                        let col = cx.new(move |cx| {
                            ProjectColumn::new_remote(
                                workspace,
                                request_broker,
                                pid,
                                pname,
                                ppath,
                                backend,
                                terminals,
                                active_drag,
                                layout,
                                action_dispatcher,
                                cx,
                            )
                        });
                        self.remote_project_columns.insert(key.clone(), col);
                    }
                }
            }
        }

        if let Some(col) = self.remote_project_columns.get(&key).cloned() {
            div()
                .id("remote-project-grid")
                .flex_1()
                .h_full()
                .flex()
                .overflow_hidden()
                .child(
                    div()
                        .flex_basis(relative(1.0))
                        .h_full()
                        .min_w(px(200.0))
                        .child(col),
                )
                .into_any_element()
        } else {
            let t = crate::theme::theme(cx);
            div()
                .id("remote-project-grid")
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(t.text_muted))
                .child("Remote project not available")
                .into_any_element()
        }
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Get overlay visibility state from overlay manager
        let om = self.overlay_manager.read(cx);
        let has_context_menu = om.has_context_menu();
        let has_folder_context_menu = om.has_folder_context_menu();
        let has_remote_context_menu = om.has_remote_context_menu();
        let has_terminal_context_menu = om.has_terminal_context_menu();
        let has_tab_context_menu = om.has_tab_context_menu();

        // Clear the pane map at the start of each render cycle
        // Each terminal pane will re-register itself during prepaint
        clear_pane_map();

        // Get active drag for global mouse handling
        let active_drag = self.active_drag.clone();
        let workspace = self.workspace.clone();

        // Capture sidebar state for mouse move handler
        let sidebar_auto_hide = self.sidebar_ctrl.is_auto_hide();
        let sidebar_hover_shown = self.sidebar_ctrl.is_hover_shown();
        let current_sidebar_width = self.sidebar_ctrl.current_width();

        // Clone overlay_manager for action handlers
        let overlay_manager = self.overlay_manager.clone();

        let focus_handle = self.focus_handle.clone();

        // Focus root if nothing else is focused (allows global keybindings to work)
        if window.focused(cx).is_none() {
            window.focus(&focus_handle, cx);
        }

        div()
            .id("root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_primary))
            .track_focus(&focus_handle)
            // Global mouse move handler for resize and auto-hide
            .on_mouse_move(cx.listener({
                let active_drag = active_drag.clone();
                let workspace = workspace.clone();
                move |this, event: &MouseMoveEvent, _window, cx| {
                    // Handle resize drag
                    if let Some(ref state) = *active_drag.borrow() {
                        match state {
                            DragState::Sidebar => {
                                // Handle sidebar resize
                                let new_width = f32::from(event.position.x);
                                this.sidebar_ctrl.set_width(new_width, &mut this.app_settings);
                                cx.notify();
                            }
                            _ => {
                                // Handle split and project column resize
                                compute_resize(event.position, state, &workspace, cx);
                            }
                        }
                    }

                    // Handle auto-hide: check if mouse left the sidebar area
                    if sidebar_auto_hide && sidebar_hover_shown {
                        // Add small margin for smoother interaction
                        let hide_threshold = current_sidebar_width + 10.0;
                        if f32::from(event.position.x) > hide_threshold {
                            this.hide_sidebar_on_leave(cx);
                        }
                    }
                }
            }))
            // Global mouse up handler to end resize (registered via window event
            // to reliably fire regardless of which child element the cursor is over)
            .child(canvas(
                |_bounds, _window, _cx| {},
                {
                    let active_drag = active_drag.clone();
                    let terminals = self.terminals.clone();
                    move |_bounds, _prepaint, window, _cx| {
                        let active_drag = active_drag.clone();
                        let terminals = terminals.clone();
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, _window, _cx| {
                            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                                let was_dragging = active_drag.borrow().is_some();
                                *active_drag.borrow_mut() = None;

                                if was_dragging {
                                    let terminals_guard = terminals.lock();
                                    for terminal in terminals_guard.values() {
                                        terminal.flush_pending_resize();
                                    }
                                }
                            }
                        });
                    }
                },
            ).absolute().size_full())
            // Handle sidebar toggle action from title bar
            .on_action(cx.listener(|this, _: &ToggleSidebar, _window, cx| {
                this.toggle_sidebar(cx);
            }))
            // Handle toggle sidebar auto-hide action
            .on_action(cx.listener(|this, _: &ToggleSidebarAutoHide, _window, cx| {
                this.toggle_sidebar_auto_hide(cx);
            }))
            // Handle focus sidebar action (keyboard navigation)
            .on_action(cx.listener(|this, _: &FocusSidebar, window, cx| {
                // Ensure sidebar is visible
                if !this.sidebar_ctrl.is_open() && !this.sidebar_ctrl.is_hover_shown() {
                    this.toggle_sidebar(cx);
                }
                let current_focus = window.focused(cx);
                let handle = this.sidebar.read(cx).focus_handle().clone();
                this.sidebar.update(cx, |sidebar, cx| {
                    sidebar.saved_focus = current_focus;
                    sidebar.activate_cursor(cx);
                });
                window.focus(&handle, cx);
            }))
            // Handle show keybindings action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowKeybindings, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_keybindings_help(cx));
                }
            }))
            // Handle show session manager action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowSessionManager, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_session_manager(cx));
                }
            }))
            // Handle show theme selector action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowThemeSelector, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_theme_selector(cx));
                }
            }))
            // Handle show command palette action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowCommandPalette, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_command_palette(cx));
                }
            }))
            // Handle show settings panel action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowSettings, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_settings_panel(cx));
                }
            }))
            // Handle show pairing dialog action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowPairingDialog, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_pairing_dialog(cx));
                }
            }))
            // Handle new project action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &NewProject, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_add_project_dialog(cx));
                }
            }))
            // Handle open settings file action
            .on_action(cx.listener(|_this, _: &OpenSettingsFile, _window, _cx| {
                open_settings_file();
            }))
            // Handle check for updates action
            .on_action(cx.listener(|_this, _: &CheckForUpdates, _window, cx| {
                if let Some(update_info) = cx.try_global::<crate::updater::GlobalUpdateInfo>() {
                    let info = update_info.0.clone();

                    // Prevent concurrent manual checks
                    if !info.try_start_manual() {
                        return;
                    }

                    info.set_status(crate::updater::UpdateStatus::Checking);
                    let token = info.current_token();
                    cx.notify();
                    cx.spawn(async move |_this, cx| {
                        match crate::updater::checker::check_for_update().await {
                            Ok(Some(release)) => {
                                if info.is_homebrew() {
                                    info.set_status(crate::updater::UpdateStatus::BrewUpdate {
                                        version: release.version,
                                    });
                                    let _ = _this.update(cx, |_, cx| cx.notify());
                                } else {
                                    // Set downloading status and notify before the blocking download
                                    info.set_status(crate::updater::UpdateStatus::Downloading {
                                        version: release.version.clone(),
                                        progress: 0,
                                    });
                                    let _ = _this.update(cx, |_, cx| cx.notify());

                                    // Download with periodic UI refresh for progress
                                    let download = crate::updater::downloader::download_asset(
                                        release.asset_url,
                                        release.asset_name,
                                        release.version.clone(),
                                        info.clone(),
                                        token,
                                        release.checksum_url,
                                    );
                                    let mut download = std::pin::pin!(download);

                                    let download_result: anyhow::Result<std::path::PathBuf> = loop {
                                        let polled = std::future::poll_fn(|task_cx| {
                                            match download.as_mut().poll(task_cx) {
                                                std::task::Poll::Ready(r) => std::task::Poll::Ready(Some(r)),
                                                std::task::Poll::Pending => std::task::Poll::Ready(None),
                                            }
                                        }).await;
                                        match polled {
                                            Some(r) => break r,
                                            None => {
                                                smol::Timer::after(std::time::Duration::from_millis(250)).await;
                                                let _ = _this.update(cx, |_, cx| cx.notify());
                                            }
                                        }
                                    };

                                    match download_result {
                                        Ok(path) => {
                                            info.set_status(crate::updater::UpdateStatus::Ready {
                                                version: release.version,
                                                path,
                                            });
                                            let _ = _this.update(cx, |_, cx| cx.notify());
                                        }
                                        Err(e) => {
                                            log::error!("Download failed: {}", e);
                                            info.set_status(crate::updater::UpdateStatus::Failed {
                                                error: e.to_string(),
                                            });
                                            let _ = _this.update(cx, |_, cx| cx.notify());
                                        }
                                    }
                                }
                            }
                            Ok(None) => {
                                info.set_status(crate::updater::UpdateStatus::Idle);
                                let _ = _this.update(cx, |_, cx| cx.notify());
                            }
                            Err(e) => {
                                log::error!("Update check failed: {}", e);
                                info.set_status(crate::updater::UpdateStatus::Failed {
                                    error: e.to_string(),
                                });
                                let _ = _this.update(cx, |_, cx| cx.notify());
                            }
                        }

                        info.finish_manual();
                    })
                    .detach();
                }
            }))
            // Handle install update action (dispatched from status bar)
            .on_action(cx.listener(|_this, _: &InstallUpdate, _window, cx| {
                if let Some(update_info) = cx.try_global::<crate::updater::GlobalUpdateInfo>() {
                    let info = update_info.0.clone();
                    if let crate::updater::UpdateStatus::Ready { version, path } = info.status() {
                        info.set_status(crate::updater::UpdateStatus::Installing {
                            version: version.clone(),
                        });
                        cx.notify();
                        cx.spawn(async move |_this, cx| {
                            let result = smol::unblock({
                                move || crate::updater::installer::install_update(&path)
                            }).await;
                            match result {
                                Ok(_) => {
                                    info.set_status(crate::updater::UpdateStatus::ReadyToRestart {
                                        version,
                                    });
                                }
                                Err(e) => {
                                    log::error!("Install failed: {}", e);
                                    info.set_status(crate::updater::UpdateStatus::Failed {
                                        error: e.to_string(),
                                    });
                                }
                            }
                            let _ = _this.update(cx, |_, cx| cx.notify());
                        }).detach();
                    }
                }
            }))
            // Handle create worktree action
            .on_action(cx.listener(|this, _: &CreateWorktree, _window, cx| {
                this.create_worktree_from_focus(cx);
            }))
            // Handle show file search action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                let workspace = workspace.clone();
                move |_this, _: &ShowFileSearch, _window, cx| {
                    // Get the focused or first visible project path
                    let project_path = workspace.read(cx).focus_manager.focused_terminal_state()
                        .map(|f| f.project_id.clone())
                        .or_else(|| {
                            workspace.read(cx).visible_projects()
                                .first()
                                .map(|p| p.id.clone())
                        })
                        .and_then(|id| {
                            workspace.read(cx).project(&id).map(|p| p.path.clone())
                        });

                    if let Some(path) = project_path {
                        overlay_manager.update(cx, |om, cx| {
                            om.toggle_file_search(std::path::PathBuf::from(path), cx);
                        });
                    }
                }
            }))
            // Handle show project switcher action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowProjectSwitcher, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_project_switcher(cx));
                }
            }))
            // Handle show diff viewer action (from keybinding or command palette - no path data)
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                let workspace = workspace.clone();
                move |_this, _: &ShowDiffViewer, _window, cx| {
                    // Get the focused or first visible project path
                    let project_path = workspace.read(cx).focus_manager.focused_terminal_state()
                        .map(|f| f.project_id.clone())
                        .or_else(|| {
                            workspace.read(cx).visible_projects()
                                .first()
                                .map(|p| p.id.clone())
                        })
                        .and_then(|id| {
                            workspace.read(cx).project(&id).map(|p| p.path.clone())
                        });

                    if let Some(path) = project_path {
                        overlay_manager.update(cx, |om, cx| {
                            om.show_diff_viewer(path, None, cx);
                        });
                    }
                }
            }))
            // Title bar at the top (with window controls)
            // On macOS fullscreen: hide title bar completely (traffic lights auto-hide)
            // On macOS non-fullscreen: show minimal title bar for traffic lights
            // On other platforms: show full title bar
            .when(!cfg!(target_os = "macos") || !window.is_fullscreen(), |d| {
                d.child(self.title_bar.clone())
            })
            // Main content area
            .child(
                // Content below title bar
                div()
                    .flex_1()
                    .flex()
                    .min_h_0()
                    .relative()
                    // Auto-hide hover zone (invisible strip on the left edge)
                    .when(self.sidebar_ctrl.is_auto_hide() && !self.sidebar_ctrl.is_open() && !self.sidebar_ctrl.is_hover_shown(), |d| {
                        d.child(
                            div()
                                .id("sidebar-hover-zone")
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(px(8.0))
                                .hover(|s| s.cursor_pointer())
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                                    this.show_sidebar_on_hover(cx);
                                }))
                                .on_mouse_move(cx.listener(|this, _, _window, cx| {
                                    this.show_sidebar_on_hover(cx);
                                }))
                        )
                    })
                    .child(
                        // Sidebar container - animated width
                        {
                            let sidebar_width = self.sidebar_ctrl.current_width();
                            let configured_width = self.sidebar_ctrl.width();
                            let show_sidebar = self.sidebar_ctrl.should_render();

                            div()
                                .id("sidebar-container")
                                .h_full()
                                .w(px(sidebar_width))
                                .overflow_hidden()
                                .flex_shrink_0()
                                .when(show_sidebar, |d| {
                                    d.child(
                                        // Inner wrapper to maintain sidebar at full width for clipping effect
                                        div()
                                            .w(px(configured_width))
                                            .h_full()
                                            .child(self.sidebar.clone())
                                    )
                                })
                        }
                    )
                    // Sidebar resize divider (only when sidebar is visible)
                    .when(self.sidebar_ctrl.should_render(), |d| {
                        d.child(render_sidebar_divider(&self.active_drag, cx))
                    })
                    .child(
                        // Main area
                        div()
                            .id("main-area")
                            .flex_1()
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .child(
                                // Projects grid (zoom is handled by LayoutContainer)
                                div()
                                    .id("projects-container")
                                    .flex_1()
                                    .min_h_0()
                                    .size_full()
                                    .child(self.render_projects_grid(cx)),
                            ),
                    ),
            )
            // Status bar at the bottom
            .child(self.status_bar.clone())
            // App menu dropdown (renders on top of everything, not on macOS where native menu is used)
            .when(!cfg!(target_os = "macos") && self.title_bar.read(cx).is_menu_open(), |d| {
                d.child(self.title_bar.update(cx, |tb, cx| tb.render_menu(cx)))
            })
            // Context menu overlay (positioned popup, separate from modals)
            .when(has_context_menu, |d| {
                d.children(self.overlay_manager.read(cx).render_context_menu())
            })
            // Folder context menu overlay (positioned popup, separate from modals)
            .when(has_folder_context_menu, |d| {
                d.children(self.overlay_manager.read(cx).render_folder_context_menu())
            })
            // Remote connection context menu overlay (positioned popup)
            .when(has_remote_context_menu, |d| {
                d.children(self.overlay_manager.read(cx).render_remote_context_menu())
            })
            // Terminal context menu overlay (positioned popup)
            .when(has_terminal_context_menu, |d| {
                d.children(self.overlay_manager.read(cx).render_terminal_context_menu())
            })
            // Tab context menu overlay (positioned popup)
            .when(has_tab_context_menu, |d| {
                d.children(self.overlay_manager.read(cx).render_tab_context_menu())
            })
            // Single active modal overlay (renders on top of everything)
            .when_some(self.overlay_manager.read(cx).render_modal(), |d, modal| {
                d.child(modal)
            })
            // Toast notifications (bottom-right, on top of everything)
            .child(self.toast_overlay.clone())
    }
}
