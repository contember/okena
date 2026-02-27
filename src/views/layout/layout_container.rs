//! Recursive layout container that renders terminal/split/tabs nodes
//!
//! The LayoutContainer is the core component for rendering terminal layouts.
//! It handles:
//! - Terminal panes (single terminals)
//! - Split panes (horizontal/vertical splits)
//! - Tab groups (via the `tabs` submodule)

use crate::action_dispatch::ActionDispatcher;
use crate::keybindings::{CloseTerminal, ToggleFullscreen};
use okena_core::api::ActionRequest;
use crate::remote::app_broadcaster::AppStateBroadcaster;
use crate::terminal::backend::TerminalBackend;
use crate::theme::{theme, with_alpha};
use crate::ui::ClickDetector;
use crate::views::components::{
    cancel_rename, finish_rename, start_rename_with_blur, RenameState,
};
use crate::views::root::TerminalsRegistry;
use crate::views::layout::app_pane::AppPaneEntity;
use crate::views::layout::app_registry;
use crate::views::layout::pane_drag::{PaneDrag, DropZone};
use crate::views::layout::remote_app_pane::RemoteAppPane;
use crate::views::layout::split_pane::{ActiveDrag, render_split_divider};
use crate::views::layout::terminal_pane::TerminalPane;
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::state::{LayoutNode, SplitDirection, Workspace};
use gpui::*;
use gpui::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Recursive layout container that renders terminal/split/tabs nodes
pub struct LayoutContainer {
    pub(super) workspace: Entity<Workspace>,
    pub(super) request_broker: Entity<RequestBroker>,
    pub(super) project_id: String,
    pub(super) project_path: String,
    pub(super) layout_path: Vec<usize>,
    pub(super) backend: Arc<dyn TerminalBackend>,
    pub(super) terminals: TerminalsRegistry,
    /// Stored terminal pane entity (for single terminal case)
    terminal_pane: Option<Entity<TerminalPane>>,
    /// Stored app pane entity (for single app case)
    app_pane: Option<AppPaneEntity>,
    /// Cached child layout containers keyed by their layout path.
    /// Without this, split/tabs would recreate entities every render, which breaks focus.
    pub(super) child_containers: HashMap<Vec<usize>, Entity<LayoutContainer>>,
    /// Shared bounds of this container (updated during prepaint via canvas)
    pub(super) container_bounds_ref: Rc<RefCell<Bounds<Pixels>>>,
    /// Animation state for recently dropped tab (tab_index, animation_progress)
    /// progress goes from 1.0 (just dropped) to 0.0 (animation complete)
    pub(super) drop_animation: Option<(usize, f32)>,
    /// Shared drag state for resize operations
    pub(super) active_drag: ActiveDrag,
    /// Double-click detector for tab rename (keyed by tab index)
    pub(super) tab_click_detector: ClickDetector<usize>,
    /// Double-click detector for empty tab bar area (new tab)
    pub(super) empty_area_click_detector: ClickDetector<()>,
    /// Rename state for tab bar (keyed by terminal_id)
    pub(super) tab_rename_state: Option<RenameState<String>>,
    /// Action dispatcher for routing terminal actions (local or remote)
    pub(super) action_dispatcher: Option<ActionDispatcher>,
    /// App state broadcaster for publishing KruhPane state to remote subscribers
    pub(super) app_broadcaster: Option<Arc<AppStateBroadcaster>>,
}

impl LayoutContainer {
    pub fn new(
        workspace: Entity<Workspace>,
        request_broker: Entity<RequestBroker>,
        project_id: String,
        project_path: String,
        layout_path: Vec<usize>,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        active_drag: ActiveDrag,
        action_dispatcher: Option<ActionDispatcher>,
        app_broadcaster: Option<Arc<AppStateBroadcaster>>,
    ) -> Self {
        Self {
            workspace,
            request_broker,
            project_id,
            project_path,
            layout_path,
            backend,
            terminals,
            terminal_pane: None,
            app_pane: None,
            child_containers: HashMap::new(),
            container_bounds_ref: Rc::new(RefCell::new(Bounds {
                origin: Point::default(),
                size: Size { width: px(800.0), height: px(600.0) },
            })),
            drop_animation: None,
            active_drag,
            tab_click_detector: ClickDetector::new(),
            empty_area_click_detector: ClickDetector::new(),
            tab_rename_state: None,
            action_dispatcher,
            app_broadcaster,
        }
    }

    /// Update the project path (e.g. after tilde expansion).
    pub fn set_project_path(&mut self, path: String) {
        self.project_path = path;
    }

    fn ensure_terminal_pane(
        &mut self,
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
        cx: &mut Context<Self>,
    ) {
        // Check if we need to create a new pane or update existing one
        let needs_new_pane = match &self.terminal_pane {
            None => true,
            Some(pane) => {
                // Check if terminal_id matches - if not, we need a new pane
                let current_id = pane.read(cx).terminal_id();
                current_id != terminal_id
            }
        };

        if needs_new_pane {
            let workspace = self.workspace.clone();
            let request_broker = self.request_broker.clone();
            let project_id = self.project_id.clone();
            let project_path = self.project_path.clone();
            let layout_path = self.layout_path.clone();
            let backend = self.backend.clone();
            let terminals = self.terminals.clone();
            let remote_ctx = self.action_dispatcher.clone();

            self.terminal_pane = Some(cx.new(move |cx| {
                TerminalPane::new(
                    workspace,
                    request_broker,
                    project_id,
                    project_path,
                    layout_path,
                    terminal_id,
                    minimized,
                    detached,
                    backend,
                    terminals,
                    remote_ctx,
                    cx,
                )
            }));
        } else if let Some(pane) = &self.terminal_pane {
            // Update minimized and detached states if they changed
            pane.update(cx, |pane, cx| {
                pane.set_minimized(minimized, cx);
                pane.set_detached(detached, cx);
            });
        }
    }

    fn ensure_app_pane(
        &mut self,
        app_id: &Option<String>,
        app_kind: &str,
        app_config: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Check if we already have the right app pane
        if let Some(ref existing) = self.app_pane {
            if existing.app_id() == app_id.as_deref() {
                return;
            }
        }

        // For remote connections, create a RemoteAppPane driven by server-pushed state
        if let Some(ActionDispatcher::Remote { manager, .. }) = &self.action_dispatcher {
            let Some(app_id_str) = app_id.clone() else {
                return;
            };

            let dispatcher = self.action_dispatcher.clone();
            let project_id = self.project_id.clone();
            let manager = manager.clone();
            let app_id_for_reg = app_id_str.clone();
            let app_kind_str = app_kind.to_string();

            let pane = cx.new(|cx| {
                RemoteAppPane::new(
                    app_id_str.clone(),
                    app_kind_str,
                    None, // state arrives via WebSocket AppStateChanged events
                    dispatcher,
                    project_id,
                    cx,
                )
            });

            let focus = pane.read(cx).focus_handle.clone();
            manager.update(cx, |m, _| {
                m.register_app_pane(app_id_for_reg.clone(), pane.downgrade());
            });

            let (display_name, icon_path) = app_registry::find_app(app_kind)
                .map(|d| (d.display_name, d.icon_path))
                .unwrap_or(("App", "icons/kruh.svg"));

            self.app_pane = Some(AppPaneEntity::new(
                pane,
                Some(app_id_for_reg),
                display_name,
                icon_path,
                focus,
            ));
            return;
        }

        // Create new app pane via registry (local project)
        self.app_pane = app_registry::create_app_pane(
            app_kind,
            app_id,
            app_config,
            self.workspace.clone(),
            self.project_id.clone(),
            self.project_path.clone(),
            self.layout_path.clone(),
            self.app_broadcaster.clone(),
            window,
            cx,
        );
    }

    fn render_app(
        &mut self,
        app_id: &Option<String>,
        app_kind: &str,
        app_config: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.ensure_app_pane(app_id, app_kind, app_config, window, cx);

        // Clear terminal pane if we had one (switching from terminal to app)
        self.terminal_pane = None;

        let app_element = self
            .app_pane
            .as_ref()
            .map(|pane| pane.into_any_element());

        let in_tab_group = self.is_in_tab_group(cx);

        let mut container = div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .relative();

        // Show standalone tab bar if not already inside a Tabs container
        if !in_tab_group {
            container = container.child(self.render_standalone_tab_bar(window, cx));
        }

        let pane_id = app_id.clone().unwrap_or_else(|| format!("app-none-{:?}", self.layout_path));

        // Close action for Cmd+W on app panes
        let app_id_for_close = app_id.clone();
        let dispatcher_for_close = self.action_dispatcher.clone();
        let project_id_for_close = self.project_id.clone();

        // Fullscreen action for Shift+Escape on app panes
        let dispatcher_for_fullscreen = self.action_dispatcher.clone();
        let project_id_for_fullscreen = self.project_id.clone();

        // Get focus handle from the app pane for key context tracking
        let app_focus_handle = self.app_pane.as_ref().map(|p| p.focus_handle().clone());

        let mut content = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .relative()
            .on_action(cx.listener(move |_this, _: &CloseTerminal, _window, cx| {
                if let Some(ref aid) = app_id_for_close {
                    if let Some(ref dispatcher) = dispatcher_for_close {
                        dispatcher.dispatch(ActionRequest::CloseApp {
                            project_id: project_id_for_close.clone(),
                            app_id: aid.clone(),
                        }, cx);
                    }
                }
            }))
            .on_action(cx.listener(move |_this, _: &ToggleFullscreen, _window, cx| {
                if let Some(ref dispatcher) = dispatcher_for_fullscreen {
                    dispatcher.dispatch(ActionRequest::SetFullscreen {
                        project_id: project_id_for_fullscreen.clone(),
                        terminal_id: None,
                    }, cx);
                }
            }))
            .children(app_element)
            .child(self.render_drop_zones(Some(pane_id), cx, &self.active_drag.clone()));

        // Add TerminalPane key context so CloseTerminal keybinding (Cmd+W) works
        if let Some(focus) = app_focus_handle {
            content = content
                .key_context("TerminalPane")
                .track_focus(&focus);
        }

        container.child(content)
    }

    pub(super) fn get_layout<'a>(&self, workspace: &'a Workspace) -> Option<&'a LayoutNode> {
        let project = workspace.project(&self.project_id)?;
        project.layout.as_ref()?.get_at_path(&self.layout_path)
    }

    /// Check if a layout node subtree contains the zoomed terminal.
    /// Returns the child index that contains the zoomed terminal, if any.
    pub(super) fn find_zoomed_child_index(
        &self,
        children: &[LayoutNode],
        cx: &Context<Self>,
    ) -> Option<usize> {
        let ws = self.workspace.read(cx);
        let (fs_project_id, fs_terminal_id) = ws.focus_manager.fullscreen_state()?;
        if fs_project_id != self.project_id {
            return None;
        }

        for (i, child) in children.iter().enumerate() {
            let ids = child.collect_terminal_ids();
            if ids.iter().any(|id| id == fs_terminal_id) {
                return Some(i);
            }
        }
        None
    }

    /// Check if this layout container is a terminal inside a Tabs container.
    fn is_in_tab_group(&self, cx: &Context<Self>) -> bool {
        if self.layout_path.is_empty() {
            return false;
        }
        let parent_path = &self.layout_path[..self.layout_path.len() - 1];
        let ws = self.workspace.read(cx);
        if let Some(project) = ws.project(&self.project_id) {
            if let Some(LayoutNode::Tabs { .. }) = project.layout.as_ref().and_then(|l| l.get_at_path(parent_path)) {
                return true;
            }
        }
        false
    }

    /// Start renaming a tab.
    pub(super) fn start_tab_rename(
        &mut self,
        terminal_id: String,
        current_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_rename_state = Some(start_rename_with_blur(
            terminal_id,
            &current_name,
            "Tab name...",
            |this: &mut LayoutContainer, _window, cx| {
                this.finish_tab_rename(cx);
            },
            window,
            cx,
        ));
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        cx.notify();
    }

    /// Finish renaming a tab.
    pub(super) fn finish_tab_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((terminal_id, new_name)) = finish_rename(&mut self.tab_rename_state, cx) {
            if let Some(ref dispatcher) = self.action_dispatcher {
                dispatcher.dispatch(
                    ActionRequest::RenameTerminal {
                        project_id: self.project_id.clone(),
                        terminal_id,
                        name: new_name,
                    },
                    cx,
                );
            }
        }
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    /// Cancel renaming a tab.
    pub(super) fn cancel_tab_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.tab_rename_state);
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    fn render_terminal(
        &mut self,
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Ensure terminal pane exists (created once, not every render)
        self.ensure_terminal_pane(terminal_id.clone(), minimized, detached, cx);

        let in_tab_group = self.is_in_tab_group(cx);
        let is_zoomed = terminal_id.as_ref().map_or(false, |tid| {
            let ws = self.workspace.read(cx);
            ws.focus_manager.is_terminal_fullscreened(&self.project_id, tid)
        });

        let mut container = div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .relative();

        // Show standalone tab bar if not already inside a Tabs container and not zoomed
        // (zoomed terminals show their own zoom header instead)
        if !in_tab_group && !is_zoomed {
            container = container.child(self.render_standalone_tab_bar(window, cx));
        }

        container
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(self.terminal_pane.clone().unwrap())
                    .child(self.render_drop_zones(terminal_id, cx, &self.active_drag.clone())),
            )
    }

    /// Render the 5-zone drop overlay for pane drag-and-drop.
    fn render_drop_zones(
        &self,
        terminal_id: Option<String>,
        cx: &mut Context<Self>,
        active_drag: &ActiveDrag,
    ) -> impl IntoElement {
        let t = theme(cx);
        let highlight = with_alpha(t.border_active, 0.3);
        let project_id = self.project_id.clone();
        let tid = terminal_id.clone();
        let id_suffix = terminal_id.unwrap_or_else(|| format!("none-{:?}", self.layout_path));
        let dispatcher = self.action_dispatcher.clone();

        let make_zone = |zone: DropZone, id_suffix: &str, active_drag: &ActiveDrag| -> Stateful<Div> {
            let zone_id = format!("drop-zone-{}-{:?}", id_suffix, zone);
            let pid = project_id.clone();
            let this_tid = tid.clone();
            let active_drag_for_hover = active_drag.clone();
            let active_drag_for_drop = active_drag.clone();
            let dispatcher = dispatcher.clone();

            let zone_str = match zone {
                DropZone::Top => "top",
                DropZone::Bottom => "bottom",
                DropZone::Left => "left",
                DropZone::Right => "right",
                DropZone::Center => "center",
            };

            div()
                .id(ElementId::Name(zone_id.into()))
                .drag_over::<PaneDrag>(move |style, _, _, _| {
                    // Don't show drop highlight when a resize is in progress
                    if active_drag_for_hover.borrow().is_some() {
                        return style;
                    }
                    style.bg(highlight)
                })
                .on_drop(cx.listener({
                    let pid = pid.clone();
                    let this_tid = this_tid.clone();
                    move |_this, drag: &PaneDrag, _window, cx| {
                        // Ignore drop when a resize is/was in progress
                        if active_drag_for_drop.borrow().is_some() {
                            return;
                        }
                        // Self-drop check
                        if Some(drag.pane_id.as_str()) == this_tid.as_deref() {
                            return;
                        }
                        // Same project check (v1)
                        if drag.project_id != pid {
                            return;
                        }
                        if let Some(ref target_id) = this_tid {
                            if let Some(ref dispatcher) = dispatcher {
                                dispatcher.dispatch(ActionRequest::MovePaneTo {
                                    project_id: drag.project_id.clone(),
                                    terminal_id: drag.pane_id.clone(),
                                    target_project_id: pid.clone(),
                                    target_terminal_id: target_id.clone(),
                                    zone: zone_str.to_string(),
                                }, cx);
                            }
                        }
                    }
                }))
        };

        // 3-column layout: Left | Middle(Top/Center/Bottom) | Right
        // Zero overlap, full coverage
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .flex_row()
            .child(
                // Left zone: 25% width, 100% height
                make_zone(DropZone::Left, &id_suffix, active_drag)
                    .w(relative(0.25))
                    .h_full(),
            )
            .child(
                // Middle column: 50% width, contains Top/Center/Bottom
                div()
                    .w(relative(0.50))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        // Top zone: 25% height
                        make_zone(DropZone::Top, &id_suffix, active_drag)
                            .w_full()
                            .h(relative(0.25)),
                    )
                    .child(
                        // Center zone: 50% height
                        make_zone(DropZone::Center, &id_suffix, active_drag)
                            .w_full()
                            .h(relative(0.50)),
                    )
                    .child(
                        // Bottom zone: 25% height
                        make_zone(DropZone::Bottom, &id_suffix, active_drag)
                            .w_full()
                            .h(relative(0.25)),
                    ),
            )
            .child(
                // Right zone: 25% width, 100% height
                make_zone(DropZone::Right, &id_suffix, active_drag)
                    .w(relative(0.25))
                    .h_full(),
            )
    }

    fn render_split(
        &mut self,
        direction: SplitDirection,
        sizes: &[f32],
        children: &[LayoutNode],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let num_children = children.len();
        let project_id = self.project_id.clone();
        let layout_path = self.layout_path.clone();

        // If a zoomed terminal exists in one of our children, render only that child at full size
        if let Some(zoomed_idx) = self.find_zoomed_child_index(children, cx) {
            let mut child_path = self.layout_path.clone();
            child_path.push(zoomed_idx);

            let container = self
                .child_containers
                .entry(child_path.clone())
                .or_insert_with(|| {
                    cx.new(|_cx| {
                        LayoutContainer::new(
                            self.workspace.clone(),
                            self.request_broker.clone(),
                            self.project_id.clone(),
                            self.project_path.clone(),
                            child_path.clone(),
                            self.backend.clone(),
                            self.terminals.clone(),
                            self.active_drag.clone(),
                            self.action_dispatcher.clone(),
                            self.app_broadcaster.clone(),
                        )
                    })
                })
                .clone();

            return div()
                .id(ElementId::Name(format!("split-container-{}-{:?}", project_id, layout_path).into()))
                .size_full()
                .min_h_0()
                .min_w_0()
                .child(container);
        }

        let is_horizontal = direction == SplitDirection::Horizontal;

        // Clean up stale child containers (e.g., when a child was removed)
        let valid_paths: std::collections::HashSet<Vec<usize>> = (0..num_children)
            .map(|i| {
                let mut path = self.layout_path.clone();
                path.push(i);
                path
            })
            .collect();
        self.child_containers.retain(|path, _| valid_paths.contains(path));

        // Shared reference to container bounds (updated by canvas during prepaint)
        let container_bounds_ref = self.container_bounds_ref.clone();

        // Check which children are hidden (minimized or detached) and collect sizes for visible ones
        let mut visible_children_info: Vec<(usize, f32)> = Vec::new();
        for (i, child) in children.iter().enumerate() {
            if !child.is_all_hidden() {
                let size = sizes.get(i).copied().unwrap_or(100.0 / num_children as f32);
                visible_children_info.push((i, size));
            }
        }

        // Normalize visible sizes to sum to 100%
        let total_visible_size: f32 = visible_children_info.iter().map(|(_, s)| s).sum();
        let normalized_sizes: Vec<f32> = if total_visible_size > 0.0 {
            visible_children_info.iter().map(|(_, s)| s / total_visible_size * 100.0).collect()
        } else {
            vec![100.0 / visible_children_info.len().max(1) as f32; visible_children_info.len()]
        };

        // Build interleaved children and dividers
        let mut elements: Vec<AnyElement> = Vec::new();

        for (visible_idx, (original_idx, _)) in visible_children_info.iter().enumerate() {
            let mut child_path = self.layout_path.clone();
            child_path.push(*original_idx);

            let container = self
                .child_containers
                .entry(child_path.clone())
                .or_insert_with(|| {
                    cx.new(|_cx| {
                        LayoutContainer::new(
                            self.workspace.clone(),
                            self.request_broker.clone(),
                            self.project_id.clone(),
                            self.project_path.clone(),
                            child_path.clone(),
                            self.backend.clone(),
                            self.terminals.clone(),
                            self.active_drag.clone(),
                            self.action_dispatcher.clone(),
                            self.app_broadcaster.clone(),
                        )
                    })
                })
                .clone();

            // Add divider before this child (if not first visible child)
            if visible_idx > 0 {
                let left_original_idx = visible_children_info[visible_idx - 1].0;
                let divider = render_split_divider(
                    self.workspace.clone(),
                    self.project_id.clone(),
                    left_original_idx,
                    *original_idx,
                    direction,
                    self.layout_path.clone(),
                    container_bounds_ref.clone(),
                    &self.active_drag,
                    self.action_dispatcher.clone(),
                    cx,
                );
                elements.push(divider.into_any_element());
            }

            let size_percent = normalized_sizes[visible_idx];
            let child_element = div()
                .flex_basis(relative(size_percent / 100.0))
                .min_w_0()
                .min_h_0()
                .child(container)
                .into_any_element();

            elements.push(child_element);
        }

        div()
            .id(ElementId::Name(format!("split-container-{}-{:?}", project_id, layout_path).into()))
            // Use a canvas to capture the container bounds during prepaint
            .child(canvas(
                {
                    let container_bounds_ref = container_bounds_ref.clone();
                    move |bounds, _window, _cx| {
                        *container_bounds_ref.borrow_mut() = bounds;
                    }
                },
                |_bounds, _prepaint, _window, _cx| {},
            ).absolute().size_full())
            // Mouse handlers are on RootView - no need to duplicate here
            .flex()
            .when(is_horizontal, |d| d.flex_col())
            .flex_nowrap()
            .size_full()
            .min_h_0()
            .min_w_0()
            .children(elements)
    }
}

impl Render for LayoutContainer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);
        let layout = self.get_layout(workspace).cloned();

        // Clean up stale entities when layout type changes
        match &layout {
            Some(LayoutNode::Terminal { .. }) => {
                // When rendering a terminal, clear app pane and child containers
                if !self.child_containers.is_empty() {
                    self.child_containers.clear();
                }
                self.app_pane = None;
            }
            Some(LayoutNode::App { .. }) => {
                // When rendering an app, clear terminal pane and child containers
                if !self.child_containers.is_empty() {
                    self.child_containers.clear();
                }
                self.terminal_pane = None;
            }
            Some(LayoutNode::Split { .. }) | Some(LayoutNode::Tabs { .. }) => {
                // When rendering split/tabs, clear leaf panes
                self.terminal_pane = None;
                self.app_pane = None;
            }
            None => {
                // Clear everything when no layout
                self.terminal_pane = None;
                self.app_pane = None;
                self.child_containers.clear();
            }
        }

        match layout {
            Some(LayoutNode::Terminal {
                terminal_id,
                minimized,
                detached,
                ..
            }) => self
                .render_terminal(terminal_id.clone(), minimized, detached, window, cx)
                .into_any_element(),

            Some(LayoutNode::Split {
                direction,
                ref sizes,
                ref children,
            }) => self
                .render_split(direction, sizes, children, window, cx)
                .into_any_element(),

            Some(LayoutNode::Tabs {
                ref children,
                active_tab,
            }) => self
                .render_tabs(children, active_tab, window, cx)
                .into_any_element(),

            Some(LayoutNode::App {
                ref app_id,
                ref app_kind,
                ref app_config,
            }) => self
                .render_app(app_id, app_kind, app_config, window, cx)
                .into_any_element(),

            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(t.text_muted))
                .child("No layout")
                .into_any_element(),
        }
    }
}
