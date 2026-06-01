//! Top-level `Render` impl for the sidebar and the `render_expanded_children`
//! helper that lays out per-project terminal / service / hook groups.

use super::{GroupKind, Sidebar, SidebarItem, SidebarProjectInfo, SidebarServiceInfo};
use crate::activity_order::{order_by_activity, ActivityEntry, ActivityTier};
use crate::drag::{FolderDrag, ProjectDrag};
use gpui::*;
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_ms;
use okena_workspace::requests::SidebarRequest;
use okena_workspace::state::{ProjectData, Workspace};
use std::collections::{HashMap, HashSet};

impl Sidebar {
    /// Build the `project_id -> services` map from the local `ServiceManager`
    /// plus remote project snapshots. Shared by the manual and activity render
    /// paths. Reads only — takes the live `workspace` borrow and a `&App` so it
    /// composes with the immutable borrows already held during render.
    pub(crate) fn collect_project_services(
        &self,
        workspace: &Workspace,
        cx: &App,
    ) -> HashMap<String, Vec<SidebarServiceInfo>> {
        let mut project_services: HashMap<String, Vec<SidebarServiceInfo>> =
            if let Some(ref sm) = self.service_manager {
                let sm = sm.read(cx);
                workspace.data().projects.iter()
                    .filter(|p| sm.has_services(&p.id))
                    .map(|p| {
                        let services = sm.services_for_project(&p.id)
                            .into_iter()
                            .filter(|inst| !inst.is_extra)
                            .map(|inst| SidebarServiceInfo {
                                name: inst.definition.name.clone(),
                                status: inst.status.clone(),
                                ports: inst.detected_ports.clone(),
                                port_host: "localhost".to_string(),
                                is_docker: matches!(inst.kind, okena_services::manager::ServiceKind::DockerCompose { .. }),
                            })
                            .collect();
                        (p.id.clone(), services)
                    })
                    .collect()
            } else {
                HashMap::new()
            };

        // Also populate services from remote project data (for projects not covered by local ServiceManager)
        for project in &workspace.data().projects {
            let Some(snapshot) = workspace.remote_snapshot(&project.id) else { continue };
            if !snapshot.services.is_empty() && !project_services.contains_key(&project.id) {
                let port_host = snapshot.host.clone().unwrap_or_else(|| "localhost".to_string());
                let services = snapshot.services.iter()
                    .filter(|api_svc| !api_svc.is_extra)
                    .map(|api_svc| {
                        SidebarServiceInfo {
                            name: api_svc.name.clone(),
                            status: okena_services::manager::ServiceStatus::from_api(&api_svc.status, api_svc.exit_code),
                            ports: api_svc.ports.clone(),
                            port_host: port_host.clone(),
                            is_docker: api_svc.kind == "docker_compose",
                        }
                    }).collect();
                project_services.insert(project.id.clone(), services);
            }
        }
        project_services
    }

    /// Outer sidebar chrome shared by the manual and activity render paths:
    /// header, projects header, and the scrolling body holding `flat_elements`
    /// plus the remote section. Tracks pointer-inside for the activity view's
    /// hover-freeze.
    pub(crate) fn render_sidebar_container(
        &mut self,
        flat_elements: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        div()
            .relative()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_secondary))
            .track_focus(&self.focus_handle)
            .key_context("Sidebar")
            .on_action(cx.listener(Self::handle_sidebar_up))
            .on_action(cx.listener(Self::handle_sidebar_down))
            .on_action(cx.listener(Self::handle_sidebar_confirm))
            .on_action(cx.listener(Self::handle_sidebar_toggle_expand))
            .on_action(cx.listener(Self::handle_sidebar_escape))
            .child(self.render_header(cx))
            .child(self.render_projects_header(cx))
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    // Freeze the activity ordering while the pointer is over the
                    // list so a finishing command / bell doesn't reshuffle rows
                    // under the cursor. Transition-only, so this is cheap.
                    .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                        if this.activity_pointer_inside != *hovered {
                            this.activity_pointer_inside = *hovered;
                            cx.notify();
                        }
                    }))
                    .children(flat_elements)
                    .child(self.render_remote_section(cx)),
            )
    }

    /// A section header for one activity tier (PINNED / NEEDS ATTENTION /
    /// RUNNING / RECENT). Non-interactive — purely a visual divider.
    fn render_activity_tier_header(&self, tier: ActivityTier, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        // A small colored dot echoes the per-row indicators for the two
        // attention-worthy tiers; pinned/recent get none.
        let dot_color = match tier {
            ActivityTier::Attention => Some(t.border_active),
            ActivityTier::Running => Some(t.border_idle),
            ActivityTier::Pinned | ActivityTier::Rest => None,
        };
        div()
            .h(px(22.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .children(dot_color.map(|color| div().size(px(6.0)).rounded_full().bg(rgb(color))))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .child(tier.label()),
            )
    }

    /// Build the flat element list for `ProjectSortMode::Activity`: every
    /// project as a peer (folders and worktree nesting ignored), grouped into
    /// activity tiers with section headers. Self-contained — reads its own
    /// workspace borrow and does not touch the manual path's state.
    fn build_activity_flat_elements(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Phase 1 — gather owned per-project infos + activity inputs while the
        // workspace borrow and the terminals lock are held, then release both
        // before the &mut self render calls below.
        let project_services;
        let mut infos: HashMap<String, SidebarProjectInfo>;
        let manual_index: HashMap<String, usize>;
        let entries: Vec<ActivityEntry>;
        {
            let workspace = self.workspace.read(cx);
            project_services = self.collect_project_services(workspace, cx);
            let terminals = self.terminals.lock();
            let mut e = Vec::new();
            let mut m = HashMap::new();
            let mut idx_map = HashMap::new();
            for (idx, project) in workspace.data().projects.iter().enumerate() {
                let mut info = SidebarProjectInfo::from_project(project, workspace, self.window_id);
                info.is_closing = workspace.is_project_closing(&project.id);
                info.is_creating = workspace.is_creating_project(&project.id);
                let has_attention = info.terminal_ids.iter().any(|tid| {
                    terminals.get(tid.as_str()).is_some_and(|t| t.has_bell() || t.has_notification())
                });
                // "Running" uses cached atomics only (no live /proc read per
                // render): a terminal the user has driven that is not sitting at
                // an idle prompt is treated as actively running a command. The
                // `waiting_for_input` flag is the same background-computed signal
                // the manual view's idle dot uses.
                let is_running = info.terminal_ids.iter().any(|tid| {
                    terminals.get(tid.as_str()).is_some_and(|t| {
                        t.had_user_input() && !t.is_waiting_for_input()
                    })
                });
                e.push(ActivityEntry {
                    id: project.id.clone(),
                    pinned: project.pinned,
                    manual_index: idx,
                    last_activity_at: project.last_activity_at,
                    has_attention,
                    is_running,
                });
                idx_map.insert(project.id.clone(), idx);
                m.insert(project.id.clone(), info);
            }
            entries = e;
            manual_index = idx_map;
            infos = m;
        }
        for (id, info) in infos.iter_mut() {
            if let Some(services) = project_services.get(id) {
                info.services = services.clone();
            }
        }

        // Phase 2 — ordering. Reuse the cached order while the pointer is inside
        // (hover-freeze), dropping any project that has since gone; otherwise
        // recompute and refresh the cache.
        let ordering: Vec<(ActivityTier, String)> =
            if self.activity_pointer_inside && !self.activity_order_cache.is_empty() {
                self.activity_order_cache
                    .iter()
                    .filter(|(_, id)| infos.contains_key(id))
                    .cloned()
                    .collect()
            } else {
                let ord: Vec<(ActivityTier, String)> = order_by_activity(entries)
                    .into_iter()
                    .map(|(tier, entry)| (tier, entry.id))
                    .collect();
                self.activity_order_cache = ord.clone();
                ord
            };

        // Phase 3 — render rows with a section header whenever the tier changes.
        let focused_project_id = self.focus_manager.read(cx).focused_project_id().cloned();
        let mut flat_elements: Vec<AnyElement> = Vec::new();
        // Cursor-key navigation is not wired for the activity view yet; rows
        // render without the keyboard cursor highlight. `flat_idx` is still
        // threaded for render_expanded_children's internal accounting.
        let mut flat_idx: usize = 0;
        let mut last_tier: Option<ActivityTier> = None;
        for (tier, id) in ordering {
            if last_tier != Some(tier) {
                flat_elements.push(self.render_activity_tier_header(tier, cx).into_any_element());
                last_tier = Some(tier);
            }
            let Some(info) = infos.get(&id) else { continue };
            let is_focused_project = focused_project_id.as_ref() == Some(&id);
            let idx = manual_index.get(&id).copied().unwrap_or(0);
            flat_elements.push(
                self.render_project_item(info, idx, false, is_focused_project, window, cx)
                    .into_any_element(),
            );
            flat_idx += 1;
            if self.expanded_projects.contains(&id) {
                self.render_expanded_children(info, 20.0, 34.0, "", None, &mut flat_idx, &mut flat_elements, cx);
            }
        }
        flat_elements
    }

    /// Render expanded children (terminals group + services group) for a project.
    /// Returns elements and advances flat_idx.
    // GPUI render helper: params are render inputs and traversal state.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_expanded_children(
        &self,
        project: &SidebarProjectInfo,
        group_header_padding: f32,
        group_items_padding: f32,
        id_prefix: &str,
        cursor_index: Option<usize>,
        flat_idx: &mut usize,
        flat_elements: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let t = theme(cx);

        // Terminals group
        if !project.terminal_ids.is_empty() {
            let is_collapsed = self.is_group_collapsed(&project.id, &GroupKind::Terminals);
            let is_cursor = cursor_index == Some(*flat_idx);
            let project_id = project.id.clone();
            flat_elements.push(
                crate::item_widgets::sidebar_group_header(
                    ElementId::Name(format!("{}term-group-{}", id_prefix, project.id).into()),
                    GroupKind::Terminals.label(),
                    project.terminal_ids.len(),
                    is_collapsed,
                    is_cursor,
                    group_header_padding,
                    &t,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.toggle_group(&project_id, GroupKind::Terminals);
                    cx.notify();
                }))
                .into_any_element()
            );
            *flat_idx += 1;

            if !is_collapsed {
                let minimized_states: Vec<(String, bool)> = {
                    let ws = self.workspace.read(cx);
                    project.terminal_ids.iter().map(|id| {
                        (id.clone(), ws.is_terminal_minimized(&project.id, id))
                    }).collect()
                };
                for (tid, is_minimized) in &minimized_states {
                    let is_cursor = cursor_index == Some(*flat_idx);
                    let is_inactive_tab = project.inactive_tab_terminals.contains(tid.as_str());
                    let is_in_tab_group = project.tab_group_terminals.contains(tid.as_str());
                    flat_elements.push(
                        self.render_terminal_item(
                            &project.id, tid, &project.terminal_names,
                            *is_minimized, is_inactive_tab, is_in_tab_group,
                            group_items_padding, id_prefix, is_cursor, cx,
                        )
                        .into_any_element()
                    );
                    *flat_idx += 1;
                }
            }
        }

        // Services group
        if !project.services.is_empty() {
            let is_collapsed = self.is_group_collapsed(&project.id, &GroupKind::Services);
            let is_cursor = cursor_index == Some(*flat_idx);
            flat_elements.push(
                self.render_services_group_header(project, is_collapsed, is_cursor, group_header_padding, cx)
                    .into_any_element()
            );
            *flat_idx += 1;

            if !is_collapsed {
                for service in &project.services {
                    let is_cursor = cursor_index == Some(*flat_idx);
                    flat_elements.push(
                        self.render_service_item(project, service, group_items_padding, is_cursor, cx)
                            .into_any_element()
                    );
                    *flat_idx += 1;
                }
            }
        }

        // Hooks group
        if !project.hook_terminals.is_empty() {
            let is_collapsed = self.is_group_collapsed(&project.id, &GroupKind::Hooks);
            let is_cursor = cursor_index == Some(*flat_idx);
            flat_elements.push(
                self.render_hooks_group_header(project, is_collapsed, is_cursor, group_header_padding, cx)
                    .into_any_element()
            );
            *flat_idx += 1;

            if !is_collapsed {
                for hook in &project.hook_terminals {
                    let is_cursor = cursor_index == Some(*flat_idx);
                    flat_elements.push(
                        self.render_hook_item(project, hook, group_items_padding, is_cursor, cx)
                            .into_any_element()
                    );
                    *flat_idx += 1;
                }
            }
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Process pending sidebar requests (drained from Workspace by observer)
        let pending = std::mem::take(&mut self.pending_sidebar_requests);
        for request in pending {
            match request {
                SidebarRequest::RenameProject { project_id, project_name } => {
                    self.start_project_rename(project_id, project_name, window, cx);
                }
                SidebarRequest::RenameFolder { folder_id, folder_name } => {
                    self.start_folder_rename(folder_id, folder_name, window, cx);
                }
                SidebarRequest::QuickCreateWorktree { project_id } => {
                    self.spawn_quick_create_worktree(&project_id, cx);
                }
            }
        }


        // Clear cursor when sidebar loses focus
        if self.cursor_index.is_some() && !self.focus_handle.is_focused(window) {
            self.cursor_index = None;
        }

        // Activity-sorted view is a separate, self-contained build path that
        // ignores project_order and folders. Branch early so the manual path
        // below stays untouched.
        let sort_mode = self
            .workspace
            .read(cx)
            .data()
            .window(self.window_id)
            .map(|w| w.project_sort_mode)
            .unwrap_or_default();
        if sort_mode.is_activity() {
            let flat_elements = self.build_activity_flat_elements(window, cx);
            return self.render_sidebar_container(flat_elements, cx);
        }

        let workspace = self.workspace.read(cx);

        // Collect all projects for lookup
        let all_projects: HashMap<&str, &ProjectData> = workspace.data().projects.iter()
            .map(|p| (p.id.as_str(), p))
            .collect();

        // Build worktree children map using parent's worktree_ids for deterministic ordering
        // Build worktree children map using parent's worktree_ids for deterministic ordering
        let mut worktree_children_map: HashMap<String, Vec<SidebarProjectInfo>> = HashMap::new();
        let all_project_ids: HashSet<&str> = workspace.data().projects.iter().map(|p| p.id.as_str()).collect();
        for parent in &workspace.data().projects {
            if !parent.worktree_ids.is_empty() {
                let mut children = Vec::new();
                for wt_id in &parent.worktree_ids {
                    if let Some(&p) = all_projects.get(wt_id.as_str()) {
                        let mut info = SidebarProjectInfo::from_project(p, workspace, self.window_id);
                        info.is_closing = workspace.is_project_closing(&p.id);
                        info.is_creating = workspace.is_creating_project(&p.id);
                        // Inherit parent project's color for visual association
                        info.folder_color = parent.folder_color;
                        children.push(info);
                    }
                }
                if !children.is_empty() {
                    worktree_children_map.insert(parent.id.clone(), children);
                }
            }
        }

        // Collect services (local ServiceManager + remote snapshots) for all projects
        let mut project_services = self.collect_project_services(workspace, cx);

        // Build sidebar items from project_order
        let mut items: Vec<SidebarItem> = Vec::new();
        for (top_index, id) in workspace.data().project_order.iter().enumerate() {
            // Check if this is a folder
            if let Some(folder) = workspace.data().folders.iter().find(|f| &f.id == id) {
                let mut folder_projects: Vec<SidebarProjectInfo> = folder.project_ids.iter()
                    .filter_map(|pid| all_projects.get(pid.as_str()))
                    .filter(|p| p.worktree_info.is_none() || !all_project_ids.contains(
                        p.worktree_info.as_ref().map(|w| w.parent_project_id.as_str()).unwrap_or("")
                    ))
                    .map(|p| {
                        let mut info = SidebarProjectInfo::from_project(p, workspace, self.window_id);
                        info.is_orphan = p.worktree_info.as_ref().is_some_and(|wt| {
                            !all_project_ids.contains(wt.parent_project_id.as_str())
                        });
                        info.is_closing = workspace.is_project_closing(&p.id);
                        info.is_creating = workspace.is_creating_project(&p.id);
                        info
                    })
                    .collect();
                let mut folder_wt_children: HashMap<String, Vec<SidebarProjectInfo>> = HashMap::new();
                for fp in &mut folder_projects {
                    if let Some(mut children) = worktree_children_map.remove(&fp.id) {
                        fp.worktree_count = children.len();
                        for child in &mut children {
                            if let Some(services) = project_services.remove(&child.id) {
                                child.services = services;
                            }
                        }
                        folder_wt_children.insert(fp.id.clone(), children);
                    }
                    if let Some(services) = project_services.remove(&fp.id) {
                        fp.services = services;
                    }
                }
                items.push(SidebarItem::Folder {
                    folder: folder.clone(),
                    index: top_index,
                    projects: folder_projects,
                    worktree_children: folder_wt_children,
                });
                continue;
            }

            // Check if this is a top-level project (not a worktree child)
            if let Some(&project) = all_projects.get(id.as_str()) {
                if let Some(ref wt_info) = project.worktree_info
                    && all_project_ids.contains(wt_info.parent_project_id.as_str()) {
                        // This is a worktree child shown under its parent, skip
                        continue;
                    }
                let mut wt_children = worktree_children_map.remove(&project.id).unwrap_or_default();
                let mut project_info = SidebarProjectInfo::from_project(project, workspace, self.window_id);
                project_info.is_orphan = project.worktree_info.as_ref().is_some_and(|wt| {
                    !all_project_ids.contains(wt.parent_project_id.as_str())
                });
                project_info.is_closing = workspace.is_project_closing(&project.id);
                project_info.is_creating = workspace.is_creating_project(&project.id);
                project_info.worktree_count = wt_children.len();

                if !wt_children.is_empty() {
                    for child in &mut wt_children {
                        if let Some(services) = project_services.remove(&child.id) {
                            child.services = services;
                        }
                    }
                }
                if let Some(services) = project_services.remove(&project.id) {
                    project_info.services = services;
                }
                items.push(SidebarItem::Project {
                    project: project_info,
                    index: top_index,
                    worktree_children: wt_children,
                });
            }
        }

        // Index for trailing drop zone — must be project_order.len() to place after everything
        let end_index = workspace.data().project_order.len();

        // Snapshot per-window folder collapse state so the for-loop body below
        // does not hold the workspace immutable borrow across mutable
        // self.render_*(.., cx) calls.
        let folder_collapsed_map: HashMap<String, bool> = workspace
            .data()
            .window(self.window_id)
            .unwrap_or(&workspace.data().main_window)
            .folder_collapsed
            .clone();

        // Build cursor items and validate cursor position
        let cursor_items = self.build_cursor_items(cx);
        self.validate_cursor(cursor_items.len());
        let cursor_index = self.cursor_index;

        // Determine which project is focused — only highlight when explicitly focused via sidebar click
        let (focused_project_id, focus_individual) = {
            let fm = self.focus_manager.read(cx);
            (fm.focused_project_id().cloned(), fm.is_focus_individual())
        };

        // Build flat elements with cursor tracking
        let mut flat_elements: Vec<AnyElement> = Vec::new();
        let mut flat_idx: usize = 0;

        // Leading drop zone so items can be dropped before the first entry
        flat_elements.push(
            div()
                .id("sidebar-drop-head")
                .h(px(4.0))
                .w_full()
                .drag_over::<ProjectDrag>(move |style, _, _, _| {
                    style.h(px(8.0)).border_b_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener(move |this, drag: &ProjectDrag, _window, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.move_project(&drag.project_id, 0, cx);
                    });
                }))
                .drag_over::<FolderDrag>(move |style, _, _, _| {
                    style.h(px(8.0)).border_b_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener(move |this, drag: &FolderDrag, _window, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.move_item_in_order(&drag.folder_id, 0, cx);
                    });
                }))
                .into_any_element()
        );

        for item in items {
            match item {
                SidebarItem::Project { project, index, worktree_children } => {
                    let has_worktrees = !worktree_children.is_empty();

                    if has_worktrees && !project.is_orphan {
                        // Group header mode: project becomes a group, main project is first child
                        let is_cursor = cursor_index == Some(flat_idx);
                        // Group header highlights when focused non-individual (showing all)
                        let is_focused_group = focused_project_id.as_ref() == Some(&project.id) && !focus_individual;
                        let all_hidden = !project.show_in_overview && worktree_children.iter().all(|c| !c.show_in_overview);
                        flat_elements.push(
                            self.render_project_group_header(&project, 4.0, "gh", "group-header-item", crate::project_list::GroupHeaderDragConfig::TopLevel { index }, all_hidden, is_cursor, is_focused_group, window, cx).into_any_element()
                        );
                        flat_idx += 1;

                        let is_expanded = self.is_project_expanded(&project.id, true);
                        if is_expanded {
                            // Main project as first child — highlights when focused individual
                            let is_cursor = cursor_index == Some(flat_idx);
                            let is_focused_project = focused_project_id.as_ref() == Some(&project.id) && focus_individual;
                            flat_elements.push(
                                self.render_project_group_child(&project, 20.0, "gc", "group-child-item", is_cursor, is_focused_project, window, cx).into_any_element()
                            );
                            flat_idx += 1;

                            if self.expanded_projects.contains(&project.id) {
                                self.render_expanded_children(&project, 34.0, 48.0, "gm-", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                            }

                            // Worktree children as siblings
                            for (wt_idx, child) in worktree_children.iter().enumerate() {
                                let is_cursor = cursor_index == Some(flat_idx);
                                let is_focused_project = focused_project_id.as_ref() == Some(&child.id);
                                flat_elements.push(
                                    self.render_worktree_item(child, 20.0, wt_idx, is_cursor, is_focused_project, window, cx).into_any_element()
                                );
                                flat_idx += 1;

                                if self.expanded_projects.contains(&child.id) {
                                    self.render_expanded_children(child, 34.0, 48.0, "wt-", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                                }
                            }
                        }
                    } else {
                        // No worktrees or orphan — standard rendering
                        let is_cursor = cursor_index == Some(flat_idx);
                        let is_focused_project = focused_project_id.as_ref() == Some(&project.id);
                        if project.is_orphan {
                            flat_elements.push(
                                self.render_worktree_item(&project, 8.0, 0, is_cursor, is_focused_project, window, cx).into_any_element()
                            );
                        } else {
                            flat_elements.push(
                                self.render_project_item(&project, index, is_cursor, is_focused_project, window, cx).into_any_element()
                            );
                        }
                        flat_idx += 1;

                        let show_children = self.expanded_projects.contains(&project.id);
                        if show_children {
                            self.render_expanded_children(&project, 20.0, 34.0, "", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                        }
                    }
                }
                SidebarItem::Folder { folder, index, projects, worktree_children } => {
                    let is_cursor = cursor_index == Some(flat_idx);
                    let folder_collapsed = folder_collapsed_map
                        .get(&folder.id)
                        .copied()
                        .unwrap_or(false);
                    let idle_terminal_count = if folder_collapsed {
                        let terminals = self.terminals.lock();
                        projects.iter()
                            .flat_map(|p| p.terminal_ids.iter())
                            .filter(|id| terminals.get(id.as_str()).is_some_and(|t| t.is_waiting_for_input()))
                            .count()
                    } else {
                        0
                    };
                    let all_hidden = projects.iter().all(|p| !p.show_in_overview) && worktree_children.values().flat_map(|c| c.iter()).all(|c| !c.show_in_overview);
                    flat_elements.push(
                        self.render_folder_header(&folder, index, projects.len(), idle_terminal_count, all_hidden, is_cursor, window, cx).into_any_element()
                    );
                    flat_idx += 1;

                    // Folder children when not collapsed
                    if !folder_collapsed {
                        for fp in &projects {
                            let fp_wt_children = worktree_children.get(&fp.id);
                            let has_worktrees = fp_wt_children.is_some_and(|c| !c.is_empty());

                            if has_worktrees && !fp.is_orphan {
                                // Group header mode within folder
                                let is_cursor = cursor_index == Some(flat_idx);
                                let is_focused_group = focused_project_id.as_ref() == Some(&fp.id) && !focus_individual;
                                flat_elements.push(
                                    {
                                    let all_hidden = !fp.show_in_overview && fp_wt_children.is_none_or(|c| c.iter().all(|c| !c.show_in_overview));
                                    self.render_project_group_header(fp, 20.0, "fgh", "fgh-item", crate::project_list::GroupHeaderDragConfig::InFolder { folder_id: folder.id.clone() }, all_hidden, is_cursor, is_focused_group, window, cx).into_any_element()
                                    }
                                );
                                flat_idx += 1;

                                let is_expanded = self.is_project_expanded(&fp.id, true);
                                if is_expanded {
                                    // Main project as first child
                                    let is_cursor = cursor_index == Some(flat_idx);
                                    let is_focused_project = focused_project_id.as_ref() == Some(&fp.id) && focus_individual;
                                    flat_elements.push(
                                        self.render_project_group_child(fp, 36.0, "fgc", "fgc-item", is_cursor, is_focused_project, window, cx).into_any_element()
                                    );
                                    flat_idx += 1;

                                    if self.expanded_projects.contains(&fp.id) {
                                        self.render_expanded_children(fp, 50.0, 64.0, "gm-", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                                    }

                                    // Worktree children as siblings
                                    if let Some(wt_children) = fp_wt_children {
                                        for (wt_idx, child) in wt_children.iter().enumerate() {
                                            let is_cursor = cursor_index == Some(flat_idx);
                                            let is_focused_project = focused_project_id.as_ref() == Some(&child.id);
                                            flat_elements.push(
                                                self.render_worktree_item(child, 36.0, wt_idx, is_cursor, is_focused_project, window, cx).into_any_element()
                                            );
                                            flat_idx += 1;

                                            if self.expanded_projects.contains(&child.id) {
                                                self.render_expanded_children(child, 50.0, 64.0, "wt-", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                                            }
                                        }
                                    }
                                }
                            } else {
                                // No worktrees or orphan — standard folder project rendering
                                let is_cursor = cursor_index == Some(flat_idx);
                                let is_focused_project = focused_project_id.as_ref() == Some(&fp.id);
                                if fp.is_orphan {
                                    flat_elements.push(
                                        self.render_worktree_item(fp, 20.0, 0, is_cursor, is_focused_project, window, cx).into_any_element()
                                    );
                                } else {
                                    flat_elements.push(
                                        self.render_folder_project_item(fp, &folder.id, is_cursor, is_focused_project, window, cx).into_any_element()
                                    );
                                }
                                flat_idx += 1;

                                let show_children = self.expanded_projects.contains(&fp.id);
                                if show_children {
                                    self.render_expanded_children(fp, 36.0, 50.0, "", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Trailing drop zone so items can be dropped after the last entry
        flat_elements.push(
            div()
                .id("sidebar-drop-tail")
                .h(px(24.0))
                .flex_1()
                .min_h(px(24.0))
                .drag_over::<ProjectDrag>(move |style, _, _, _| {
                    style.border_t_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener(move |this, drag: &ProjectDrag, _window, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.move_project(&drag.project_id, end_index, cx);
                    });
                }))
                .drag_over::<FolderDrag>(move |style, _, _, _| {
                    style.border_t_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener(move |this, drag: &FolderDrag, _window, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.move_item_in_order(&drag.folder_id, end_index, cx);
                    });
                }))
                .into_any_element()
        );

        self.render_sidebar_container(flat_elements, cx)
    }
}
