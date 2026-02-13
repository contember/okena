//! Recursive layout container that renders terminal/split/tabs nodes
//!
//! The LayoutContainer is the core component for rendering terminal layouts.
//! It handles:
//! - Terminal panes (single terminals)
//! - Split panes (horizontal/vertical splits)
//! - Tab groups (via the `tabs` submodule)

use crate::terminal::backend::TerminalBackend;
use crate::theme::{theme, with_alpha};
use crate::views::root::TerminalsRegistry;
use crate::views::layout::pane_drag::{PaneDrag, DropZone};
use crate::views::layout::split_pane::{ActiveDrag, render_split_divider, render_grid_row_divider, render_grid_col_divider};
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
        self.ensure_terminal_pane(terminal_id.clone(), minimized, detached, cx);

        div()
            .size_full()
            .min_h_0()
            .relative()
            .child(self.terminal_pane.clone().unwrap())
            .child(self.render_drop_zones(terminal_id, cx, &self.active_drag.clone()))
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
        let workspace = self.workspace.clone();
        let project_id = self.project_id.clone();
        let tid = terminal_id.clone();
        let id_suffix = terminal_id.unwrap_or_else(|| format!("none-{:?}", self.layout_path));

        let make_zone = |zone: DropZone, id_suffix: &str, active_drag: &ActiveDrag| -> Stateful<Div> {
            let zone_id = format!("drop-zone-{}-{:?}", id_suffix, zone);
            let ws = workspace.clone();
            let pid = project_id.clone();
            let this_tid = tid.clone();
            let active_drag_for_hover = active_drag.clone();
            let active_drag_for_drop = active_drag.clone();

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
                        if Some(drag.terminal_id.as_str()) == this_tid.as_deref() {
                            return;
                        }
                        // Same project check (v1)
                        if drag.project_id != pid {
                            return;
                        }
                        if let Some(ref target_id) = this_tid {
                            ws.update(cx, |ws, cx| {
                                ws.move_pane(
                                    &drag.project_id,
                                    &drag.terminal_id,
                                    &pid,
                                    target_id,
                                    zone,
                                    cx,
                                );
                            });
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

    fn render_grid(
        &mut self,
        rows: usize,
        cols: usize,
        row_sizes: &[f32],
        col_sizes: &[f32],
        children: &[LayoutNode],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let project_id = self.project_id.clone();
        let layout_path = self.layout_path.clone();

        // If a zoomed terminal exists in any grid cell, render only that cell
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

            if self.external_layout.is_some() {
                container.update(cx, |c, _| {
                    c.external_layout = Some(children[zoomed_idx].clone());
                });
            }

            return div()
                .id(ElementId::Name(format!("grid-container-{}-{:?}", project_id, layout_path).into()))
                .size_full()
                .min_h_0()
                .min_w_0()
                .child(container);
        }

        // Clean up stale child containers
        let valid_paths: std::collections::HashSet<Vec<usize>> = (0..children.len())
            .map(|i| {
                let mut path = self.layout_path.clone();
                path.push(i);
                path
            })
            .collect();
        self.child_containers.retain(|path, _| valid_paths.contains(path));

        let container_bounds_ref = self.container_bounds_ref.clone();

        // Normalize row sizes
        let total_row: f32 = row_sizes.iter().sum();
        let norm_row_sizes: Vec<f32> = if total_row > 0.0 {
            row_sizes.iter().map(|s| s / total_row * 100.0).collect()
        } else {
            vec![100.0 / rows.max(1) as f32; rows]
        };

        // Normalize col sizes
        let total_col: f32 = col_sizes.iter().sum();
        let norm_col_sizes: Vec<f32> = if total_col > 0.0 {
            col_sizes.iter().map(|s| s / total_col * 100.0).collect()
        } else {
            vec![100.0 / cols.max(1) as f32; cols]
        };

        // Build rows interleaved with horizontal dividers
        let mut row_elements: Vec<AnyElement> = Vec::new();

        for row in 0..rows {
            // Horizontal divider between rows
            if row > 0 {
                let divider = render_grid_row_divider(
                    self.workspace.clone(),
                    self.project_id.clone(),
                    row - 1,
                    row,
                    self.layout_path.clone(),
                    container_bounds_ref.clone(),
                    &self.active_drag,
                    cx,
                );
                row_elements.push(divider.into_any_element());
            }

            // Build columns for this row
            let mut col_elements: Vec<AnyElement> = Vec::new();

            for col in 0..cols {
                // Vertical divider between columns
                if col > 0 {
                    let divider = render_grid_col_divider(
                        self.workspace.clone(),
                        self.project_id.clone(),
                        col - 1,
                        col,
                        self.layout_path.clone(),
                        container_bounds_ref.clone(),
                        &self.active_drag,
                        cx,
                    );
                    col_elements.push(divider.into_any_element());
                }

                let flat_index = row * cols + col;
                let mut child_path = self.layout_path.clone();
                child_path.push(flat_index);

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

                if self.external_layout.is_some() {
                    if let Some(child_layout) = children.get(flat_index) {
                        container.update(cx, |c, _| {
                            c.external_layout = Some(child_layout.clone());
                        });
                    }
                }

                let size_percent = norm_col_sizes.get(col).copied().unwrap_or(100.0 / cols as f32);
                col_elements.push(
                    div()
                        .flex_basis(relative(size_percent / 100.0))
                        .min_w_0()
                        .min_h_0()
                        .child(container)
                        .into_any_element(),
                );
            }

            let row_size_percent = norm_row_sizes.get(row).copied().unwrap_or(100.0 / rows as f32);
            row_elements.push(
                div()
                    .flex_basis(relative(row_size_percent / 100.0))
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .flex_nowrap()
                    .children(col_elements)
                    .into_any_element(),
            );
        }

        div()
            .id(ElementId::Name(format!("grid-container-{}-{:?}", project_id, layout_path).into()))
            .child(canvas(
                {
                    let container_bounds_ref = container_bounds_ref.clone();
                    move |bounds, _window, _cx| {
                        *container_bounds_ref.borrow_mut() = bounds;
                    }
                },
                |_bounds, _prepaint, _window, _cx| {},
            ).absolute().size_full())
            .relative()
            .flex()
            .flex_col()
            .flex_nowrap()
            .size_full()
            .min_h_0()
            .min_w_0()
            .children(row_elements)
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

            // Propagate external layout to child
            if self.external_layout.is_some() {
                container.update(cx, |c, _| {
                    c.external_layout = Some(children[zoomed_idx].clone());
                });
            }

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
                        )
                    })
                })
                .clone();

            // Propagate external layout to child
            if self.external_layout.is_some() {
                container.update(cx, |c, _| {
                    c.external_layout = Some(children[*original_idx].clone());
                });
            }

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
            Some(LayoutNode::Split { .. }) | Some(LayoutNode::Tabs { .. }) | Some(LayoutNode::Grid { .. }) => {
                // When rendering split/tabs/grid, clear any cached terminal_pane from previous terminal
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

            Some(LayoutNode::Grid {
                rows,
                cols,
                ref row_sizes,
                ref col_sizes,
                ref children,
            }) => self
                .render_grid(rows, cols, row_sizes, col_sizes, children, window, cx)
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
