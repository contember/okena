use crate::keybindings::{ShowKeybindings, ShowSessionManager, ShowThemeSelector, ShowCommandPalette, ShowSettings, OpenSettingsFile, ShowFileSearch, ShowContentSearch, ShowProjectSwitcher, ShowDiffViewer, ShowHookLog, NewProject, ToggleSidebar, ToggleSidebarAutoHide, TogglePaneSwitcher, CreateWorktree, CheckForUpdates, InstallUpdate, FocusSidebar, FocusActiveProject, ShowPairingDialog, StartAllServices, StopAllServices, ClearFocus, EqualizeLayout};
use crate::settings::{open_settings_file, settings_entity};
use crate::theme::theme;
use crate::views::layout::navigation::{get_pane_map, prune_pane_map};
use crate::views::layout::split_pane::{compute_resize, render_project_divider, render_sidebar_divider, DragState};
use crate::workspace::requests::{OverlayRequest, ProjectOverlay, ProjectOverlayKind};
use crate::ui::tokens::{ui_text_md, ui_text_xl};
use gpui::*;
use gpui::prelude::*;
use std::future::Future;

use super::RootView;

impl RootView {
    /// Normalize raw project widths to percentages summing to 100%.
    fn normalize_widths(raw_widths: &[f32]) -> Vec<f32> {
        let total: f32 = raw_widths.iter().sum();
        if total > 0.0 {
            raw_widths.iter().map(|w| w / total * 100.0).collect()
        } else {
            let n = raw_widths.len();
            vec![100.0 / n as f32; n]
        }
    }

    /// Convert normalized percentage widths to pixel widths.
    fn to_pixel_widths(widths: &[f32], container_width: f32, min_col_width: f32) -> Vec<f32> {
        let num_dividers = widths.len().saturating_sub(1) as f32;
        let available_width = (container_width - num_dividers * 1.0).max(0.0);
        widths.iter()
            .map(|w| (available_width * w / 100.0).max(min_col_width))
            .collect()
    }

    /// Scroll the projects grid horizontally to ensure the focused project column is visible.
    pub(super) fn scroll_to_focused_project(&self, focused_id: Option<&str>, center: bool, cx: &Context<Self>) {
        let focused_id = match focused_id {
            Some(id) => id,
            None => return,
        };

        let workspace = self.workspace.read(cx);

        // Don't scroll when zoomed to a single project
        if workspace.focus_manager.fullscreen_project_id().is_some() {
            return;
        }

        let visible_projects: Vec<String> = workspace.visible_projects()
            .iter().map(|p| p.id.clone()).collect();
        let num_projects = visible_projects.len();
        if num_projects <= 1 {
            return;
        }

        // Find the focused project's index
        let focused_idx = match visible_projects.iter().position(|id| id == focused_id) {
            Some(idx) => idx,
            None => return,
        };

        let settings = settings_entity(cx).read(cx).settings.clone();
        let container_width = f32::from(self.projects_grid_bounds.borrow().size.width);

        let raw_widths: Vec<f32> = visible_projects.iter()
            .map(|id| workspace.get_project_width(self.window_id, id, num_projects))
            .collect();
        let widths = Self::normalize_widths(&raw_widths);
        let pixel_widths = Self::to_pixel_widths(&widths, container_width, settings.min_column_width);

        // Compute the left edge (x offset) of the focused column
        let mut col_left: f32 = 0.0;
        for i in 0..focused_idx {
            col_left += pixel_widths[i] + 1.0; // +1 for divider
        }

        let new_offset = if center {
            // Center the focused column in the viewport
            let col_center = col_left + pixel_widths[focused_idx] / 2.0;
            -(col_center - container_width / 2.0)
        } else {
            let col_right = col_left + pixel_widths[focused_idx];
            let current_offset = f32::from(self.projects_scroll_handle.offset().x);
            let viewport_left = -current_offset;
            let viewport_right = viewport_left + container_width;

            if col_left < viewport_left {
                -col_left
            } else if col_right > viewport_right {
                -(col_right - container_width)
            } else {
                return; // already visible
            }
        };

        let max_offset = self.projects_scroll_handle.max_offset();
        let clamped = new_offset.clamp(-f32::from(max_offset.x), 0.0);
        self.projects_scroll_handle.set_offset(point(px(clamped), px(0.0)));
    }

    pub(super) fn render_projects_grid(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Execute pending center-scroll (deferred from unfocus to let layout update first).
        // We wait until the scroll handle reports overflow (max_offset > 0), which means
        // the layout has been recalculated with all projects visible.
        if let Some(project_id) = self.pending_center_scroll.take() {
            let workspace = self.workspace.read(cx);
            let num_visible = workspace.visible_projects().len();
            let is_zoomed = workspace.focus_manager.focused_project_id().is_some();

            if is_zoomed || num_visible <= 1 {
                // Still zoomed or only one project — no centering needed
            } else if self.projects_scroll_handle.max_offset().x > px(0.0) {
                self.scroll_to_focused_project(Some(&project_id), true, cx);
            } else {
                // Layout hasn't updated yet — re-queue for next frame
                self.pending_center_scroll = Some(project_id);
                cx.notify();
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

        // Evict stale pane map entries for projects no longer rendered
        // (e.g. worktree columns hidden in overview mode)
        {
            let visible_ids: std::collections::HashSet<&str> = visible_projects.iter()
                .map(|s| s.as_str()).collect();
            prune_pane_map(&visible_ids);
        }

        // Empty state when folder filter yields no results
        if num_projects == 0 {
            let has_folder_filter = self.workspace.read(cx).active_folder_filter(self.window_id).is_some();
            if has_folder_filter {
                let t = theme(cx);
                let workspace = self.workspace.clone();
                let window_id = self.window_id;
                return div()
                    .id("projects-grid-empty")
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(ui_text_xl(cx))
                            .text_color(rgb(t.text_muted))
                            .child("No projects in this folder"),
                    )
                    .child(
                        div()
                            .id("clear-folder-filter")
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.border_active))
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child("Show all projects")
                            .on_click(move |_, _window, cx| {
                                workspace.update(cx, |ws, cx| {
                                    ws.set_folder_filter(window_id, None, cx);
                                });
                            }),
                    )
                    .into_any_element();
            }
        }

        // Get widths for each project
        let settings = settings_entity(cx).read(cx).settings.clone();

        let widths: Vec<f32> = if num_projects <= 1 {
            vec![100.0; num_projects]
        } else {
            let workspace = self.workspace.read(cx);
            let raw_widths: Vec<f32> = visible_projects.iter()
                .map(|id| workspace.get_project_width(self.window_id, id, num_projects))
                .collect();
            Self::normalize_widths(&raw_widths)
        };

        // Persistent bounds reference for resize calculation (survives across renders)
        let container_bounds = self.projects_grid_bounds.clone();

        // Compute pixel widths from percentages, accounting for divider widths
        let container_width = f32::from(container_bounds.borrow().size.width);
        let pixel_widths = Self::to_pixel_widths(&widths, container_width, settings.min_column_width);

        // Build interleaved columns and dividers
        let mut elements: Vec<AnyElement> = Vec::new();

        for (i, project_id) in visible_projects.iter().enumerate() {
            let pixel_width = pixel_widths.get(i).copied().unwrap_or(200.0);

            if let Some(col) = self.project_columns.get(project_id).cloned() {
                let col_element = div()
                    .w(px(pixel_width))
                    .flex_shrink_0()
                    .h_full()
                    .child(AnyView::from(col).cached(
                        StyleRefinement::default().size_full()
                    ))
                    .into_any_element();

                elements.push(col_element);

                // Add divider after each column except the last
                if i < num_projects - 1 {
                    let min_col_width = settings_entity(cx).read(cx).settings.min_column_width;
                    let divider = render_project_divider(
                        self.window_id,
                        self.workspace.clone(),
                        i,
                        visible_projects.clone(),
                        container_bounds.clone(),
                        &self.active_drag,
                        min_col_width,
                        cx,
                    );
                    elements.push(divider.into_any_element());
                }
            }
        }

        let t = theme(cx);
        let scroll_handle = self.projects_scroll_handle.clone();
        let scrollbar_color = rgb(t.scrollbar);

        let scroll_handle_for_wheel = self.projects_scroll_handle.clone();

        div()
            .id("projects-grid-wrapper")
            .flex_1()
            .h_full()
            .min_w_0()
            .overflow_x_hidden()
            .relative()
            // Horizontal scrolling of project columns: shift+scroll or native touchpad horizontal scroll
            .on_scroll_wheel(cx.listener(move |_this, event: &ScrollWheelEvent, _window, cx| {
                let delta = event.delta.pixel_delta(px(17.0));
                let scroll_amount = if event.modifiers.shift {
                    // Shift+scroll: use horizontal delta if present, otherwise convert vertical
                    if !delta.x.is_zero() { delta.x } else { delta.y }
                } else if !delta.x.is_zero() {
                    // Native touchpad horizontal scroll
                    delta.x
                } else {
                    return;
                };
                let max_offset = scroll_handle_for_wheel.max_offset();
                if max_offset.x <= px(2.0) {
                    return;
                }
                let current = scroll_handle_for_wheel.offset();
                let new_x = (current.x + scroll_amount).clamp(-max_offset.x, px(0.0));
                scroll_handle_for_wheel.set_offset(point(new_x, current.y));
                cx.notify();
            }))
            .child(
                div()
                    .id("projects-grid")
                    .size_full()
                    .flex()
                    .overflow_x_hidden()
                    .track_scroll(&self.projects_scroll_handle)
                    // Canvas to capture container bounds (updates persistent bounds for next render)
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
            )
            // Horizontal scrollbar overlay (absolute positioned at bottom)
            .child({
                let hscroll_bounds = self.hscroll_bounds.clone();
                div()
                    .id("hscrollbar")
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(6.0))
                    .cursor(CursorStyle::Arrow)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            let max_offset = this.projects_scroll_handle.max_offset();
                            if max_offset.x <= px(2.0) {
                                return;
                            }
                            this.hscroll_dragging = true;
                            // Jump to clicked position
                            if let Some(bounds) = *this.hscroll_bounds.borrow() {
                                let track_width = f32::from(bounds.size.width);
                                let relative_x = f32::from(event.position.x) - f32::from(bounds.origin.x);
                                let ratio = (relative_x / track_width).clamp(0.0, 1.0);
                                let new_x = -ratio * f32::from(max_offset.x);
                                this.projects_scroll_handle.set_offset(point(px(new_x), px(0.0)));
                            }
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        if !this.hscroll_dragging {
                            return;
                        }
                        let max_offset = this.projects_scroll_handle.max_offset();
                        if max_offset.x <= px(2.0) {
                            return;
                        }
                        if let Some(bounds) = *this.hscroll_bounds.borrow() {
                            let track_width = f32::from(bounds.size.width);
                            let relative_x = f32::from(event.position.x) - f32::from(bounds.origin.x);
                            let ratio = (relative_x / track_width).clamp(0.0, 1.0);
                            let new_x = -ratio * f32::from(max_offset.x);
                            this.projects_scroll_handle.set_offset(point(px(new_x), px(0.0)));
                        }
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                            if this.hscroll_dragging {
                                this.hscroll_dragging = false;
                                cx.notify();
                            }
                        }),
                    )
                    .child(canvas(
                        {
                            let hscroll_bounds = hscroll_bounds.clone();
                            move |bounds, _window, _cx| {
                                *hscroll_bounds.borrow_mut() = Some(bounds);
                            }
                        },
                        move |bounds, _, window, _cx| {
                            let max_scroll = scroll_handle.max_offset();
                            if max_scroll.x <= px(2.0) {
                                return;
                            }
                            let offset = scroll_handle.offset();
                            let track_width = f32::from(bounds.size.width);
                            let content_width = track_width + f32::from(max_scroll.x);
                            let thumb_width = (track_width / content_width * track_width).max(30.0);
                            let scroll_ratio = f32::from(-offset.x) / f32::from(max_scroll.x);
                            let thumb_x = scroll_ratio * (track_width - thumb_width);

                            let thumb_bounds = Bounds {
                                origin: point(bounds.origin.x + px(thumb_x), bounds.origin.y + px(1.0)),
                                size: size(px(thumb_width), px(4.0)),
                            };
                            window.paint_quad(fill(thumb_bounds, scrollbar_color).corner_radii(px(2.0)));
                        },
                    ).size_full())
            })
            .into_any_element()
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
        let has_worktree_list = om.has_worktree_list();
        let has_color_picker = om.has_color_picker();

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
                move |this, event: &MouseMoveEvent, window, cx| {
                    // Handle resize drag
                    if let Some(ref state) = *active_drag.borrow() {
                        match state {
                            DragState::Sidebar => {
                                // Handle sidebar resize
                                let new_width = f32::from(event.position.x);
                                this.sidebar_ctrl.set_width(new_width);
                                // Persist through global SettingsState (debounced)
                                let width = this.sidebar_ctrl.width();
                                settings_entity(cx).update(cx, |s, cx| s.set_sidebar_width(width, cx));
                                cx.notify();
                            }
                            DragState::ServicePanel { project_id, initial_mouse_y, initial_height } => {
                                // Dragging up increases height, dragging down decreases
                                let delta = initial_mouse_y - f32::from(event.position.y);
                                let new_height = initial_height + delta;
                                let project_id = project_id.clone();
                                if let Some(col) = this.project_columns.get(&project_id).cloned() {
                                    col.update(cx, |col, cx| {
                                        col.set_service_panel_height(new_height, cx);
                                    });
                                }
                            }
                            DragState::HookPanel { project_id, initial_mouse_y, initial_height } => {
                                let delta = initial_mouse_y - f32::from(event.position.y);
                                let new_height = initial_height + delta;
                                let project_id = project_id.clone();
                                if let Some(col) = this.project_columns.get(&project_id).cloned() {
                                    col.update(cx, |col, cx| {
                                        col.set_hook_panel_height(new_height, cx);
                                    });
                                }
                            }
                            _ => {
                                // Handle split and project column resize
                                compute_resize(this.window_id, event.position, state, &workspace, cx);
                                // Bypass all .cached() views so terminal elements
                                // repaint with new bounds during drag.
                                window.refresh();
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
                    let workspace = workspace.clone();
                    move |_bounds, _prepaint, window, _cx| {
                        let active_drag = active_drag.clone();
                        let terminals = terminals.clone();
                        let workspace = workspace.clone();
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, _window, cx| {
                            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                                let was_split_drag = matches!(
                                    *active_drag.borrow(),
                                    Some(DragState::Split { .. })
                                );
                                let was_dragging = active_drag.borrow().is_some();
                                *active_drag.borrow_mut() = None;

                                if was_dragging {
                                    let terminals_guard = terminals.lock();
                                    for terminal in terminals_guard.values() {
                                        terminal.flush_pending_resize();
                                    }
                                }

                                // Persist final split sizes (drag used ui_only notify)
                                if was_split_drag {
                                    workspace.update(cx, |ws, cx| {
                                        ws.notify_data(cx);
                                    });
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
            // Handle clear focus action (show all projects)
            .on_action(cx.listener(|this, _: &ClearFocus, _window, cx| {
                let window_id = this.window_id;
                this.workspace.update(cx, |ws, cx| {
                    ws.set_focused_project(None, cx);
                    ws.set_folder_filter(window_id, None, cx);
                });
            }))
            // Toggle focus on the active terminal's project (zoom in / zoom out)
            .on_action(cx.listener(|this, _: &FocusActiveProject, _window, cx| {
                let ws = this.workspace.read(cx);
                let is_focused = ws.focus_manager.focused_project_id().is_some();
                if is_focused {
                    let window_id = this.window_id;
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project(None, cx);
                        ws.set_folder_filter(window_id, None, cx);
                    });
                } else {
                    let project_id = this.workspace.read(cx).focus_manager
                        .focused_terminal_state()
                        .map(|state| state.project_id);
                    if let Some(project_id) = project_id {
                        this.workspace.update(cx, |ws, cx| {
                            ws.set_focused_project(Some(project_id), cx);
                        });
                    }
                }
            }))
            // Handle equalize layout action
            .on_action(cx.listener(|this, _: &EqualizeLayout, _window, cx| {
                this.workspace.update(cx, |ws, cx| {
                    // Clear custom column widths → equal distribution.
                    ws.data.main_window.project_widths.clear();
                    // Equalize pane sizes in the focused terminal's parent split
                    ws.equalize_focused_split(cx);
                });
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
            // Handle show hook log action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowHookLog, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_hook_log(cx));
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
                move |this, _: &NewProject, _window, cx| {
                    let rm = this.remote_manager.clone();
                    overlay_manager.update(cx, |om, cx| om.toggle_add_project_dialog(rm, cx));
                }
            }))
            // Handle open settings file action
            .on_action(cx.listener(|_this, _: &OpenSettingsFile, _window, _cx| {
                open_settings_file();
            }))
            // Handle check for updates action
            .on_action(cx.listener(|_this, _: &CheckForUpdates, _window, cx| {
                if let Some(update_info) = cx.try_global::<okena_ext_updater::GlobalUpdateInfo>() {
                    let info = update_info.0.clone();

                    // Prevent concurrent manual checks
                    if !info.try_start_manual() {
                        return;
                    }

                    info.set_status(okena_ext_updater::UpdateStatus::Checking);
                    let token = info.current_token();
                    cx.notify();
                    cx.spawn(async move |_this, cx| {
                        match okena_ext_updater::checker::check_for_update(info.app_version()).await {
                            Ok(Some(release)) => {
                                if info.is_homebrew() {
                                    info.set_status(okena_ext_updater::UpdateStatus::BrewUpdate {
                                        version: release.version,
                                    });
                                    let _ = _this.update(cx, |_, cx| cx.notify());
                                } else {
                                    // Set downloading status and notify before the blocking download
                                    info.set_status(okena_ext_updater::UpdateStatus::Downloading {
                                        version: release.version.clone(),
                                        progress: 0,
                                    });
                                    let _ = _this.update(cx, |_, cx| cx.notify());

                                    // Download with periodic UI refresh for progress
                                    let download = okena_ext_updater::downloader::download_asset(
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
                                            info.set_status(okena_ext_updater::UpdateStatus::Ready {
                                                version: release.version,
                                                path,
                                            });
                                            let _ = _this.update(cx, |_, cx| cx.notify());
                                        }
                                        Err(e) => {
                                            log::error!("Download failed: {}", e);
                                            info.set_status(okena_ext_updater::UpdateStatus::Failed {
                                                error: e.to_string(),
                                            });
                                            let _ = _this.update(cx, |_, cx| cx.notify());
                                        }
                                    }
                                }
                            }
                            Ok(None) => {
                                info.set_status(okena_ext_updater::UpdateStatus::Idle);
                                let _ = _this.update(cx, |_, cx| cx.notify());
                            }
                            Err(e) => {
                                log::error!("Update check failed: {}", e);
                                info.set_status(okena_ext_updater::UpdateStatus::Failed {
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
                if let Some(update_info) = cx.try_global::<okena_ext_updater::GlobalUpdateInfo>() {
                    let info = update_info.0.clone();
                    if let okena_ext_updater::UpdateStatus::Ready { version, path } = info.status() {
                        info.set_status(okena_ext_updater::UpdateStatus::Installing {
                            version: version.clone(),
                        });
                        cx.notify();
                        cx.spawn(async move |_this, cx| {
                            let result = smol::unblock({
                                move || okena_ext_updater::installer::install_update(&path)
                            }).await;
                            match result {
                                Ok(_) => {
                                    info.set_status(okena_ext_updater::UpdateStatus::ReadyToRestart {
                                        version,
                                    });
                                }
                                Err(e) => {
                                    log::error!("Install failed: {}", e);
                                    info.set_status(okena_ext_updater::UpdateStatus::Failed {
                                        error: e.to_string(),
                                    });
                                }
                            }
                            let _ = _this.update(cx, |_, cx| cx.notify());
                        }).detach();
                    }
                }
            }))
            // Handle toggle pane switcher action
            .on_action(cx.listener(|this, _: &TogglePaneSwitcher, _window, cx| {
                if this.pane_switch_active {
                    this.pane_switch_active = false;
                    this.pane_switcher_entity = None;
                } else {
                    this.pane_switch_active = true;
                    let pane_map = get_pane_map();
                    this.show_pane_switcher(pane_map, cx);
                }
                cx.notify();
            }))
            // Handle create worktree action
            .on_action(cx.listener(|this, _: &CreateWorktree, _window, cx| {
                this.create_worktree_from_focus(cx);
            }))
            // Handle start all services action
            .on_action(cx.listener({
                let workspace = workspace.clone();
                move |this, _: &StartAllServices, _window, cx| {
                    if let Some(ref sm) = this.service_manager {
                        let project_id = workspace.read(cx).focus_manager
                            .focused_terminal_state()
                            .map(|f| f.project_id.clone());
                        if let Some(pid) = project_id {
                            let path = sm.read(cx).project_path(&pid).cloned();
                            if let Some(path) = path {
                                sm.update(cx, |sm, cx| sm.start_all(&pid, &path, cx));
                            }
                        }
                    }
                }
            }))
            // Handle stop all services action
            .on_action(cx.listener({
                let workspace = workspace.clone();
                move |this, _: &StopAllServices, _window, cx| {
                    if let Some(ref sm) = this.service_manager {
                        let project_id = workspace.read(cx).focus_manager
                            .focused_terminal_state()
                            .map(|f| f.project_id.clone());
                        if let Some(pid) = project_id {
                            sm.update(cx, |sm, cx| sm.stop_all(&pid, cx));
                        }
                    }
                }
            }))
            // Handle show file search action
            .on_action(cx.listener({
                let workspace = workspace.clone();
                move |this, _: &ShowFileSearch, _window, cx| {
                    let project_id = workspace.read(cx).focus_manager.focused_terminal_state()
                        .map(|f| f.project_id.clone())
                        .or_else(|| {
                            workspace.read(cx).visible_projects()
                                .first()
                                .map(|p| p.id.clone())
                        });

                    if let Some(project_id) = project_id {
                        this.request_broker.update(cx, |broker, cx| {
                            broker.push_overlay_request(
                                OverlayRequest::Project(ProjectOverlay { project_id, kind: ProjectOverlayKind::FileSearch }),
                                cx,
                            );
                        });
                    }
                }
            }))
            // Handle show content search action
            .on_action(cx.listener({
                let workspace = workspace.clone();
                move |this, _: &ShowContentSearch, _window, cx| {
                    let project_id = workspace.read(cx).focus_manager.focused_terminal_state()
                        .map(|f| f.project_id.clone())
                        .or_else(|| {
                            workspace.read(cx).visible_projects()
                                .first()
                                .map(|p| p.id.clone())
                        });

                    if let Some(project_id) = project_id {
                        this.request_broker.update(cx, |broker, cx| {
                            broker.push_overlay_request(
                                OverlayRequest::Project(ProjectOverlay { project_id, kind: ProjectOverlayKind::ContentSearch }),
                                cx,
                            );
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
                let workspace = workspace.clone();
                move |this, _: &ShowDiffViewer, _window, cx| {
                    // Get the focused or first visible project ID
                    let project_id = workspace.read(cx).focus_manager.focused_terminal_state()
                        .map(|f| f.project_id.clone())
                        .or_else(|| {
                            workspace.read(cx).visible_projects()
                                .first()
                                .map(|p| p.id.clone())
                        });

                    if let Some(project_id) = project_id {
                        this.request_broker.update(cx, |broker, cx| {
                            broker.push_overlay_request(OverlayRequest::Project(ProjectOverlay {
                                project_id,
                                kind: ProjectOverlayKind::DiffViewer {
                                    file: None, mode: None, commit_message: None, commits: None, commit_index: None,
                                },
                            }), cx);
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
                    .min_w_0()
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
                                            .child(AnyView::from(self.sidebar.clone()).cached(
                                                StyleRefinement::default().size_full()
                                            ))
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
                            .min_w_0()
                            .child(
                                // Projects grid (zoom is handled by LayoutContainer)
                                div()
                                    .id("projects-container")
                                    .flex_1()
                                    .min_h_0()
                                    .min_w_0()
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
            // Color picker popover (positioned popup, rendered at root for full-window backdrop)
            .when(has_color_picker, |d| {
                d.children(self.overlay_manager.read(cx).render_color_picker())
            })
            // Worktree list popover (positioned popup, rendered at root for full-window backdrop)
            .when(has_worktree_list, |d| {
                d.children(self.overlay_manager.read(cx).render_worktree_list())
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
            // Pane switcher overlay (numbered pane badges)
            .when_some(self.pane_switcher_entity.clone(), |d, entity| {
                d.child(entity)
            })
    }
}
