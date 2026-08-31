//! Recursive layout container that renders terminal/split/tabs nodes

use crate::ActionDispatch;
use crate::layout::pane_drag::{DropZone, PaneDrag, PaneMoveState, is_move_target};
use crate::layout::split_pane::{ActiveDrag, render_split_divider};
use crate::layout::terminal_pane::TerminalPane;
use gpui::prelude::*;
use gpui::*;
use okena_core::api::ActionRequest;
use okena_files::theme::theme;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_ui::click_detector::ClickDetector;
use okena_ui::theme::with_alpha;
use okena_ui::tokens::ui_text_sm;
use okena_workspace::focus::FocusManager;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::{LayoutNode, SplitDirection, WindowId, Workspace};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

// Re-export rename state from okena-ui
pub use okena_ui::rename_state::*;

/// Trigger that opens the adaptive terminal menu for one pane.
///
/// Free-standing so the project header can render it without reaching into a
/// `LayoutContainer` entity (and inheriting its `ActionDispatch` generic).
#[allow(clippy::too_many_arguments)]
pub fn terminal_actions_button(
    action: okena_ui::header_buttons::HeaderAction,
    id_suffix: &str,
    project_id: String,
    request_broker: Entity<RequestBroker>,
    layout_path: Vec<usize>,
    terminal_id: Option<String>,
    can_export_buffer: bool,
    include_primary_actions: bool,
    cx: &App,
) -> Stateful<Div> {
    use okena_ui::header_buttons::{ButtonSize, header_button_base};

    let t = theme(cx);

    header_button_base(action, id_suffix, ButtonSize::COMPACT, &t, None, None).on_click(
        move |_, window, cx| {
            if let Some(terminal_id) = terminal_id.as_ref() {
                request_broker.update(cx, |broker, cx| {
                    broker.push_overlay_request(
                        okena_workspace::requests::OverlayRequest::Project(
                            okena_workspace::requests::ProjectOverlay {
                                project_id: project_id.clone(),
                                kind: okena_workspace::requests::ProjectOverlayKind::TerminalMenu {
                                    terminal_id: terminal_id.clone(),
                                    layout_path: layout_path.clone(),
                                    position: window.mouse_position(),
                                    can_export_buffer,
                                    invocation:
                                        okena_workspace::requests::TerminalMenuInvocation::Header {
                                            include_primary_actions,
                                        },
                                },
                            },
                        ),
                        cx,
                    );
                });
            }
            cx.stop_propagation();
        },
    )
}

/// Recursive layout container that renders terminal/split/tabs nodes
pub struct LayoutContainer<D: ActionDispatch> {
    pub(super) workspace: Entity<Workspace>,
    pub(super) focus_manager: Entity<FocusManager>,
    pub(super) request_broker: Entity<RequestBroker>,
    pub(super) window_id: WindowId,
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
    pub(super) pane_move: Entity<PaneMoveState>,
    pub(super) tab_click_detector: ClickDetector<usize>,
    pub(super) empty_area_click_detector: ClickDetector<()>,
    pub(super) tab_rename_state: Option<RenameState<String>>,
    pub(super) action_dispatcher: Option<D>,
    pub(super) tab_scroll_handle: ScrollHandle,
    pub(super) last_scrolled_to_tab: Option<usize>,
}

impl<D: ActionDispatch + Send + Sync> LayoutContainer<D> {
    // GPUI view constructor: each param is a distinct injected dependency.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        request_broker: Entity<RequestBroker>,
        window_id: WindowId,
        project_id: String,
        project_path: String,
        layout_path: Vec<usize>,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        active_drag: ActiveDrag,
        pane_move: Entity<PaneMoveState>,
        action_dispatcher: Option<D>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&pane_move, |_this, _state, cx| cx.notify())
            .detach();
        Self {
            workspace,
            focus_manager,
            request_broker,
            window_id,
            project_id,
            project_path,
            layout_path,
            backend,
            terminals,
            terminal_pane: None,
            child_containers: HashMap::new(),
            container_bounds_ref: Rc::new(RefCell::new(Bounds {
                origin: Point::default(),
                size: Size {
                    width: px(800.0),
                    height: px(600.0),
                },
            })),
            drop_animation: None,
            active_drag,
            pane_move,
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
            let window_id = self.window_id;
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
                    window_id,
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

    pub(super) fn deregister_resize_viewers(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = self.terminal_pane.clone() {
            pane.update(cx, |pane, cx| pane.deregister_resize_viewer(cx));
        }

        let children: Vec<_> = self.child_containers.values().cloned().collect();
        for child in children {
            child.update(cx, |child, cx| child.deregister_resize_viewers(cx));
        }
    }

    pub(super) fn deregister_child_resize_viewers_except(
        &mut self,
        visible_paths: &HashSet<Vec<usize>>,
        cx: &mut Context<Self>,
    ) {
        let hidden_children: Vec<_> = self
            .child_containers
            .iter()
            .filter(|(path, _)| !visible_paths.contains(*path))
            .map(|(_, child)| child.clone())
            .collect();

        for child in hidden_children {
            child.update(cx, |child, cx| child.deregister_resize_viewers(cx));
        }
    }

    fn is_in_tab_group(&self, cx: &Context<Self>) -> bool {
        self.workspace
            .read(cx)
            .project(&self.project_id)
            .and_then(|project| project.layout.as_ref())
            .is_some_and(|layout| layout.is_in_tab_group(&self.layout_path))
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
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn finish_tab_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((terminal_id, new_name)) = finish_rename(&mut self.tab_rename_state, cx)
            && let Some(ref dispatcher) = self.action_dispatcher
        {
            dispatcher.dispatch(
                ActionRequest::RenameTerminal {
                    project_id: self.project_id.clone(),
                    terminal_id,
                    name: new_name,
                },
                cx,
            );
        }
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn cancel_tab_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.tab_rename_state);
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
            cx.notify();
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
        let is_zoomed = terminal_id.as_ref().is_some_and(|tid| {
            let fm = self.focus_manager.read(cx);
            fm.is_terminal_fullscreened(&self.project_id, tid)
        });
        let mut container = div().size_full().min_h_0().flex().flex_col().relative();

        // Reading the settings is a serde round-trip, so only ask once the
        // cheap structural checks have not already ruled the header out.
        if !in_tab_group
            && !is_zoomed
            && !crate::terminal_view_settings(cx).auto_hide_single_terminal_header
        {
            container = container.child(self.render_standalone_tab_bar(window, cx));
        }

        container.child(
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .when_some(self.terminal_pane.clone(), |d, pane| {
                    d.child(AnyView::from(pane).cached(StyleRefinement::default().size_full()))
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
        let move_source = self.pane_move.read(cx).source().cloned();
        let pane_move = self.pane_move.clone();

        let make_zone =
            |zone: DropZone, id_suffix: &str, active_drag: &ActiveDrag| -> Stateful<Div> {
                let zone_id = format!("drop-zone-{}-{:?}", id_suffix, zone);
                let pid = project_id.clone();
                let this_tid = tid.clone();
                let active_drag_for_hover = active_drag.clone();
                let active_drag_for_drop = active_drag.clone();
                let dispatcher_for_drop = dispatcher.clone();
                let dispatcher_for_click = dispatcher.clone();
                let move_source = move_source.clone();
                let pane_move = pane_move.clone();

                let (zone_str, zone_label) = match zone {
                    DropZone::Top => ("top", "Above"),
                    DropZone::Bottom => ("bottom", "Below"),
                    DropZone::Left => ("left", "Left"),
                    DropZone::Right => ("right", "Right"),
                    DropZone::Center => ("center", "Tab"),
                };

                let element = div()
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
                            if let Some(ref target_id) = this_tid
                                && let Some(ref dispatcher) = dispatcher_for_drop
                            {
                                dispatcher.dispatch(
                                    ActionRequest::MovePaneTo {
                                        project_id: drag.project_id.clone(),
                                        terminal_id: drag.terminal_id.clone(),
                                        target_project_id: pid.clone(),
                                        target_terminal_id: target_id.clone(),
                                        zone: zone_str.to_string(),
                                    },
                                    cx,
                                );
                            }
                        }
                    }));

                if let Some(source) =
                    move_source.filter(|source| is_move_target(source, this_tid.as_deref()))
                {
                    element
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(with_alpha(t.border_active, 0.5))
                        .bg(with_alpha(t.border_active, 0.1))
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_primary))
                        .hover(|style| style.bg(highlight))
                        .child(zone_label)
                        .on_click(cx.listener(move |_this, _, _window, cx| {
                            if let Some(ref target_id) = this_tid
                                && let Some(ref dispatcher) = dispatcher_for_click
                            {
                                dispatcher.dispatch(
                                    ActionRequest::MovePaneTo {
                                        project_id: source.project_id.clone(),
                                        terminal_id: source.terminal_id.clone(),
                                        target_project_id: pid.clone(),
                                        target_terminal_id: target_id.clone(),
                                        zone: zone_str.to_string(),
                                    },
                                    cx,
                                );
                                pane_move.update(cx, |state, cx| state.cancel(cx));
                            }
                        }))
                } else {
                    element
                }
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

            let visible_paths = HashSet::from([child_path.clone()]);
            self.deregister_child_resize_viewers_except(&visible_paths, cx);

            let container = self
                .child_containers
                .entry(child_path.clone())
                .or_insert_with(|| {
                    cx.new(|cx| {
                        LayoutContainer::new(
                            self.workspace.clone(),
                            self.focus_manager.clone(),
                            self.request_broker.clone(),
                            self.window_id,
                            self.project_id.clone(),
                            self.project_path.clone(),
                            child_path.clone(),
                            self.backend.clone(),
                            self.terminals.clone(),
                            self.active_drag.clone(),
                            self.pane_move.clone(),
                            self.action_dispatcher.clone(),
                            cx,
                        )
                    })
                })
                .clone();

            return div()
                .id(ElementId::Name(
                    format!("split-container-{}-{:?}", project_id, layout_path).into(),
                ))
                .size_full()
                .min_h_0()
                .min_w_0()
                .child(AnyView::from(container).cached(StyleRefinement::default().size_full()));
        }

        let is_horizontal = direction == SplitDirection::Horizontal;

        let valid_paths: std::collections::HashSet<Vec<usize>> = (0..num_children)
            .map(|i| {
                let mut path = self.layout_path.clone();
                path.push(i);
                path
            })
            .collect();

        let mut visible_children_info: Vec<(usize, f32)> = Vec::new();
        for (i, child) in children.iter().enumerate() {
            if !child.is_all_hidden() {
                let size = sizes.get(i).copied().unwrap_or(100.0 / num_children as f32);
                visible_children_info.push((i, size));
            }
        }
        let visible_paths: HashSet<Vec<usize>> = visible_children_info
            .iter()
            .map(|(i, _)| {
                let mut path = self.layout_path.clone();
                path.push(*i);
                path
            })
            .collect();
        self.deregister_child_resize_viewers_except(&visible_paths, cx);
        self.child_containers
            .retain(|path, _| valid_paths.contains(path));

        let container_bounds_ref = self.container_bounds_ref.clone();

        let total_visible_size: f32 = visible_children_info.iter().map(|(_, s)| s).sum();
        let normalized_sizes: Vec<f32> = if total_visible_size > 0.0 {
            visible_children_info
                .iter()
                .map(|(_, s)| s / total_visible_size * 100.0)
                .collect()
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
                    cx.new(|cx| {
                        LayoutContainer::new(
                            self.workspace.clone(),
                            self.focus_manager.clone(),
                            self.request_broker.clone(),
                            self.window_id,
                            self.project_id.clone(),
                            self.project_path.clone(),
                            child_path.clone(),
                            self.backend.clone(),
                            self.terminals.clone(),
                            self.active_drag.clone(),
                            self.pane_move.clone(),
                            self.action_dispatcher.clone(),
                            cx,
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
                .child(AnyView::from(container).cached(StyleRefinement::default().size_full()))
                .into_any_element();

            elements.push(child_element);
        }

        div()
            .id(ElementId::Name(
                format!("split-container-{}-{:?}", project_id, layout_path).into(),
            ))
            .child(
                canvas(
                    {
                        let container_bounds_ref = container_bounds_ref.clone();
                        move |bounds, _window, _cx| {
                            *container_bounds_ref.borrow_mut() = bounds;
                        }
                    },
                    |_bounds, _prepaint, _window, _cx| {},
                )
                .absolute()
                .size_full(),
            )
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
        let mut layout = self.get_layout(workspace).cloned();
        if let Some(LayoutNode::Split { direction, .. }) = &mut layout {
            *direction = workspace
                .project_layout_mode(self.window_id)
                .presented_split_direction(*direction);
        }

        match &layout {
            Some(LayoutNode::Terminal { .. }) => {
                if !self.child_containers.is_empty() {
                    self.deregister_child_resize_viewers_except(&HashSet::new(), cx);
                    self.child_containers.clear();
                }
            }
            Some(LayoutNode::Split { .. }) | Some(LayoutNode::Tabs { .. }) => {
                if let Some(pane) = self.terminal_pane.take() {
                    pane.update(cx, |pane, cx| pane.deregister_resize_viewer(cx));
                }
            }
            None => {
                self.deregister_resize_viewers(cx);
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
