//! Recursive layout container that renders terminal/split/tabs nodes
//!
//! The LayoutContainer is the core component for rendering terminal layouts.
//! It handles:
//! - Terminal panes (single terminals)
//! - Split panes (horizontal/vertical splits)
//! - Tab groups (via the `tabs` submodule)

use crate::terminal::backend::TerminalBackend;
use crate::theme::theme;
use crate::views::root::TerminalsRegistry;
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
    /// Cached child layout containers keyed by their layout path.
    /// Without this, split/tabs would recreate entities every render, which breaks focus.
    pub(super) child_containers: HashMap<Vec<usize>, Entity<LayoutContainer>>,
    /// Shared bounds of this container (updated during prepaint via canvas)
    pub(super) container_bounds_ref: Rc<RefCell<Bounds<Pixels>>>,
    /// Tab context menu state: (tab_index, position, num_tabs)
    pub(super) tab_context_menu: Option<(usize, Point<Pixels>, usize)>,
    /// Animation state for recently dropped tab (tab_index, animation_progress)
    /// progress goes from 1.0 (just dropped) to 0.0 (animation complete)
    pub(super) drop_animation: Option<(usize, f32)>,
    /// Shared drag state for resize operations
    pub(super) active_drag: ActiveDrag,
    /// External layout override (for remote projects not in workspace)
    pub(super) external_layout: Option<LayoutNode>,
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
            child_containers: HashMap::new(),
            container_bounds_ref: Rc::new(RefCell::new(Bounds {
                origin: Point::default(),
                size: Size { width: px(800.0), height: px(600.0) },
            })),
            tab_context_menu: None,
            drop_animation: None,
            active_drag,
            external_layout: None,
        }
    }

    /// Set an external layout override (for remote projects).
    pub fn set_external_layout(&mut self, layout: LayoutNode) {
        self.external_layout = Some(layout);
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

    fn render_terminal(
        &mut self,
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Ensure terminal pane exists (created once, not every render)
        self.ensure_terminal_pane(terminal_id, minimized, detached, cx);

        div()
            .size_full()
            .min_h_0()
            .child(self.terminal_pane.clone().unwrap())
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
            let is_hidden = match child {
                LayoutNode::Terminal { minimized, detached, .. } => *minimized || *detached,
                _ => false,
            };
            if !is_hidden {
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
                        )
                    })
                })
                .clone();

            // Add divider before this child (if not first visible child)
            if visible_idx > 0 {
                let divider = render_split_divider(
                    self.workspace.clone(),
                    self.project_id.clone(),
                    visible_idx - 1,
                    direction,
                    self.layout_path.clone(),
                    container_bounds_ref.clone(),
                    &self.active_drag,
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
        // Use external layout if set (remote projects), otherwise read from workspace
        let layout = if let Some(ref ext) = self.external_layout {
            Some(ext.clone())
        } else {
            self.get_layout(workspace).cloned()
        };

        // Clean up stale entities when layout type changes
        match &layout {
            Some(LayoutNode::Terminal { .. }) => {
                // When rendering a terminal, clear any cached child containers from previous split/tabs
                if !self.child_containers.is_empty() {
                    self.child_containers.clear();
                }
            }
            Some(LayoutNode::Split { .. }) | Some(LayoutNode::Tabs { .. }) => {
                // When rendering split/tabs, clear any cached terminal_pane from previous terminal
                if self.terminal_pane.is_some() {
                    self.terminal_pane = None;
                }
            }
            None => {
                // Clear everything when no layout
                self.terminal_pane = None;
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
