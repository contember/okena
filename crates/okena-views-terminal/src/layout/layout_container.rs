//! Recursive layout container that renders terminal/split/tabs nodes

use crate::ActionDispatch;
use okena_core::api::ActionRequest;
use okena_terminal::backend::TerminalBackend;
use okena_files::theme::theme;
use okena_ui::theme::with_alpha;
use okena_ui::click_detector::ClickDetector;
use crate::layout::pane_drag::{PaneDrag, DropZone};
use crate::layout::split_pane::{ActiveDrag, render_split_divider};
use crate::layout::terminal_pane::TerminalPane;
use okena_terminal::TerminalsRegistry;
use okena_workspace::focus::FocusManager;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::{LayoutNode, SplitDirection, Workspace};
use gpui::*;
use gpui::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

// Re-export rename state from okena-ui
pub use okena_ui::rename_state::*;

/// Recursive layout container that renders terminal/split/tabs nodes
pub struct LayoutContainer<D: ActionDispatch> {
    pub(super) workspace: Entity<Workspace>,
    pub(super) focus_manager: Entity<FocusManager>,
    pub(super) request_broker: Entity<RequestBroker>,
    pub(super) project_id: String,
    pub(super) project_path: String,
    pub(super) layout_path: Vec<usize>,
    pub(super) backend: Arc<dyn TerminalBackend>,
    pub(super) terminals: TerminalsRegistry,
    terminal_pane: Option<Entity<TerminalPane<D>>>,
    pub(super) child_containers: HashMap<Vec<usize>, Entity<LayoutContainer<D>>>,
    pub(super) container_bounds_ref: Rc<RefCell<Bounds<Pixels>>>,
    pub(super) drop_animation: Option<(usize, f32)>,
    pub(super) active_drag: ActiveDrag,
    pub(super) tab_click_detector: ClickDetector<usize>,
    pub(super) empty_area_click_detector: ClickDetector<()>,
    pub(super) tab_rename_state: Option<RenameState<String>>,
    pub(super) action_dispatcher: Option<D>,
    pub(super) tab_scroll_handle: ScrollHandle,
    pub(super) last_scrolled_to_tab: Option<usize>,
}

impl<D: ActionDispatch + Send + Sync> LayoutContainer<D> {
    pub fn new(
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        request_broker: Entity<RequestBroker>,
        project_id: String,
        project_path: String,
        layout_path: Vec<usize>,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        active_drag: ActiveDrag,
        action_dispatcher: Option<D>,
    ) -> Self {
        Self {
            workspace,
            focus_manager,
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
            tab_click_detector: ClickDetector::new(),
            empty_area_click_detector: ClickDetector::new(),
            tab_rename_state: None,
            action_dispatcher,
            tab_scroll_handle: ScrollHandle::new(),
            last_scrolled_to_tab: None,
        }
    }

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
        let needs_new_pane = match &self.terminal_pane {
            None => true,
            Some(pane) => {
                let current_id = pane.read(cx).terminal_id();
                current_id != terminal_id
            }
        };

        if needs_new_pane {
            let workspace = self.workspace.clone();
            let focus_manager = self.focus_manager.clone();
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
                    focus_manager,
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

    pub(super) fn find_zoomed_child_index(
        &self,
        children: &[LayoutNode],
        cx: &Context<Self>,
    ) -> Option<usize> {
        let fm = self.focus_manager.read(cx);
        let (fs_project_id, fs_terminal_id) = fm.fullscreen_state()?;
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
            |this: &mut LayoutContainer<D>, _window, cx| {
                this.finish_tab_rename(cx);
            },
            window,
            cx,
        ));
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.clear_focused_terminal(fm, cx));
        });
        cx.notify();
    }

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
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
        });
        cx.notify();
    }

    pub(super) fn cancel_tab_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.tab_rename_state);
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
        });
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
        self.ensure_terminal_pane(terminal_id.clone(), minimized, detached, cx);

        let in_tab_group = self.is_in_tab_group(cx);
        let is_zoomed = terminal_id.as_ref().map_or(false, |tid| {
            let fm = self.focus_manager.read(cx);
            fm.is_terminal_fullscreened(&self.project_id, tid)
        });

        let mut container = div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .relative();

        if !in_tab_group && !is_zoomed {
            container = container.child(self.render_standalone_tab_bar(window, cx));
        }

        container
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .when_some(self.terminal_pane.clone(), |d, pane| {
                        d.child(AnyView::from(pane).cached(
                            StyleRefinement::default().size_full(),
                        ))
                    })
                    .child(self.render_drop_zones(terminal_id, cx, &self.active_drag.clone())),
            )
    }

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
                    if active_drag_for_hover.borrow().is_some() {
                        return style;
                    }
                    style.bg(highlight)
                })
                .on_drop(cx.listener({
                    let pid = pid.clone();
                    let this_tid = this_tid.clone();
                    move |_this, drag: &PaneDrag, _window, cx| {
                        if active_drag_for_drop.borrow().is_some() {
                            return;
                        }
                        if Some(drag.terminal_id.as_str()) == this_tid.as_deref() {
                            return;
                        }
                        if let Some(ref target_id) = this_tid {
                            if let Some(ref dispatcher) = dispatcher {
                                dispatcher.dispatch(ActionRequest::MovePaneTo {
                                    project_id: drag.project_id.clone(),
                                    terminal_id: drag.terminal_id.clone(),
                                    target_project_id: pid.clone(),
                                    target_terminal_id: target_id.clone(),
                                    zone: zone_str.to_string(),
                                }, cx);
                            }
                        }
                    }
                }))
        };

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .flex_row()
            .child(
                make_zone(DropZone::Left, &id_suffix, active_drag)
                    .w(relative(0.25))
                    .h_full(),
            )
            .child(
                div()
                    .w(relative(0.50))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        make_zone(DropZone::Top, &id_suffix, active_drag)
                            .w_full()
                            .h(relative(0.25)),
                    )
                    .child(
                        make_zone(DropZone::Center, &id_suffix, active_drag)
                            .w_full()
                            .h(relative(0.50)),
                    )
                    .child(
                        make_zone(DropZone::Bottom, &id_suffix, active_drag)
                            .w_full()
                            .h(relative(0.25)),
                    ),
            )
            .child(
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
                            self.focus_manager.clone(),
                            self.request_broker.clone(),
                            self.project_id.clone(),
                            self.project_path.clone(),
                            child_path.clone(),
                            self.backend.clone(),
                            self.terminals.clone(),
                            self.active_drag.clone(),
                            self.action_dispatcher.clone(),
                        )
                    })
                })
                .clone();

            return div()
                .id(ElementId::Name(format!("split-container-{}-{:?}", project_id, layout_path).into()))
                .size_full()
                .min_h_0()
                .min_w_0()
                .child(AnyView::from(container).cached(
                    StyleRefinement::default().size_full()
                ));
        }

        let is_horizontal = direction == SplitDirection::Horizontal;

        let valid_paths: std::collections::HashSet<Vec<usize>> = (0..num_children)
            .map(|i| {
                let mut path = self.layout_path.clone();
                path.push(i);
                path
            })
            .collect();
        self.child_containers.retain(|path, _| valid_paths.contains(path));

        let container_bounds_ref = self.container_bounds_ref.clone();

        let mut visible_children_info: Vec<(usize, f32)> = Vec::new();
        for (i, child) in children.iter().enumerate() {
            if !child.is_all_hidden() {
                let size = sizes.get(i).copied().unwrap_or(100.0 / num_children as f32);
                visible_children_info.push((i, size));
            }
        }

        let total_visible_size: f32 = visible_children_info.iter().map(|(_, s)| s).sum();
        let normalized_sizes: Vec<f32> = if total_visible_size > 0.0 {
            visible_children_info.iter().map(|(_, s)| s / total_visible_size * 100.0).collect()
        } else {
            vec![100.0 / visible_children_info.len().max(1) as f32; visible_children_info.len()]
        };

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
                            self.focus_manager.clone(),
                            self.request_broker.clone(),
                            self.project_id.clone(),
                            self.project_path.clone(),
                            child_path.clone(),
                            self.backend.clone(),
                            self.terminals.clone(),
                            self.active_drag.clone(),
                            self.action_dispatcher.clone(),
                        )
                    })
                })
                .clone();

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
                .child(AnyView::from(container).cached(
                    StyleRefinement::default().size_full()
                ))
                .into_any_element();

            elements.push(child_element);
        }

        div()
            .id(ElementId::Name(format!("split-container-{}-{:?}", project_id, layout_path).into()))
            .child(canvas(
                {
                    let container_bounds_ref = container_bounds_ref.clone();
                    move |bounds, _window, _cx| {
                        *container_bounds_ref.borrow_mut() = bounds;
                    }
                },
                |_bounds, _prepaint, _window, _cx| {},
            ).absolute().size_full())
            .flex()
            .when(is_horizontal, |d| d.flex_col())
            .flex_nowrap()
            .size_full()
            .min_h_0()
            .min_w_0()
            .children(elements)
    }
}

impl<D: ActionDispatch + Send + Sync> Render for LayoutContainer<D> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);
        let layout = self.get_layout(workspace).cloned();

        match &layout {
            Some(LayoutNode::Terminal { .. }) => {
                if !self.child_containers.is_empty() {
                    self.child_containers.clear();
                }
            }
            Some(LayoutNode::Split { .. }) | Some(LayoutNode::Tabs { .. }) => {
                if self.terminal_pane.is_some() {
                    self.terminal_pane = None;
                }
            }
            None => {
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
