use crate::theme::theme;
use crate::workspace::state::{SplitDirection, Workspace};
use gpui::*;
use gpui::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Unified drag state for all resize operations
#[derive(Clone)]
pub enum DragState {
    /// Resizing a split pane within a project
    Split {
        project_id: String,
        layout_path: Vec<usize>,
        child_index: usize,
        direction: SplitDirection,
        container_bounds: Bounds<Pixels>,
        /// Mouse position at drag start (for delta-based resize)
        initial_mouse_pos: Point<Pixels>,
        /// Sizes snapshot at drag start
        initial_sizes: Vec<f32>,
    },
    /// Resizing project columns
    ProjectColumn {
        divider_index: usize,
        project_ids: Vec<String>,
        container_bounds: Bounds<Pixels>,
    },
    /// Resizing sidebar width
    Sidebar,
}

pub type ActiveDrag = Rc<RefCell<Option<DragState>>>;

/// Create a new active drag handle.
pub fn new_active_drag() -> ActiveDrag {
    Rc::new(RefCell::new(None))
}

/// Helper to compute and apply resize based on mouse position
pub fn compute_resize(
    mouse_pos: Point<Pixels>,
    drag_state: &DragState,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    match drag_state {
        DragState::Split { project_id, layout_path, child_index, direction, container_bounds, initial_mouse_pos, initial_sizes } => {
            let bounds = *container_bounds;
            let is_horizontal = *direction == SplitDirection::Horizontal;
            let divider_index = *child_index;

            let container_size = if is_horizontal {
                f32::from(bounds.size.height)
            } else {
                f32::from(bounds.size.width)
            };

            if container_size <= 0.0 {
                return;
            }

            let num_children = initial_sizes.len();
            if num_children < 2 {
                return;
            }

            // Divider N is between child N and child N+1
            let left_child = divider_index;
            let right_child = divider_index + 1;

            if right_child >= num_children {
                return;
            }

            // Combined size of the two adjacent children
            let combined_size = initial_sizes[left_child] + initial_sizes[right_child];

            // Delta-based resize: compute mouse movement since drag start
            let delta = if is_horizontal {
                f32::from(mouse_pos.y) - f32::from(initial_mouse_pos.y)
            } else {
                f32::from(mouse_pos.x) - f32::from(initial_mouse_pos.x)
            };
            let delta_percent = delta / container_size * 100.0;

            let left_size = (initial_sizes[left_child] + delta_percent).clamp(5.0, combined_size - 5.0);
            let right_size = combined_size - left_size;

            // Build new sizes: keep all others unchanged, update only the two adjacent
            let mut new_sizes = initial_sizes.clone();
            new_sizes[left_child] = left_size;
            new_sizes[right_child] = right_size;

            let project_id = project_id.clone();
            let layout_path = layout_path.clone();

            workspace.update(cx, |ws, cx| {
                ws.update_split_sizes(&project_id, &layout_path, new_sizes, cx);
            });
        }
        DragState::ProjectColumn { divider_index, project_ids, container_bounds } => {
            let bounds = *container_bounds;
            let container_width = f32::from(bounds.size.width);
            if container_width <= 0.0 {
                return;
            }

            let relative_x = f32::from(mouse_pos.x) - f32::from(bounds.origin.x);
            let divider_pos_percent = (relative_x / container_width * 100.0).clamp(10.0, 90.0);

            let num_projects = project_ids.len();
            let divider_index = *divider_index;

            let mut new_widths: HashMap<String, f32> = HashMap::new();

            if num_projects == 2 {
                new_widths.insert(project_ids[0].clone(), divider_pos_percent);
                new_widths.insert(project_ids[1].clone(), 100.0 - divider_pos_percent);
            } else {
                let before_count = divider_index + 1;
                let after_count = num_projects - before_count;

                let before_width = divider_pos_percent / before_count as f32;
                let after_width = (100.0 - divider_pos_percent) / after_count as f32;

                for (i, project_id) in project_ids.iter().enumerate() {
                    if i <= divider_index {
                        new_widths.insert(project_id.clone(), before_width);
                    } else {
                        new_widths.insert(project_id.clone(), after_width);
                    }
                }
            }

            workspace.update(cx, |ws, cx| {
                ws.update_project_widths(new_widths, cx);
            });
        }
        DragState::Sidebar => {
            // Sidebar resize is handled directly in RootView
        }
    }
}

/// Render an inline split divider handle element
pub fn render_split_divider(
    workspace: Entity<Workspace>,
    project_id: String,
    child_index: usize,
    direction: SplitDirection,
    layout_path: Vec<usize>,
    container_bounds: Rc<RefCell<Bounds<Pixels>>>,
    active_drag: &ActiveDrag,
    cx: &App,
) -> impl IntoElement {
    let t = theme(cx);
    let is_horizontal = direction == SplitDirection::Horizontal;
    let active_drag = active_drag.clone();

    div()
        .id(ElementId::Name(format!("split-handle-{}-{}", project_id, child_index).into()))
        .group("split-handle")
        .when(is_horizontal, |d| d.h(px(5.0)).w_full())
        .when(!is_horizontal, |d| d.w(px(5.0)).h_full())
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor(if is_horizontal {
            CursorStyle::ResizeUpDown
        } else {
            CursorStyle::ResizeLeftRight
        })
        .on_mouse_down(MouseButton::Left, {
            let active_drag = active_drag.clone();
            let project_id = project_id.clone();
            let layout_path = layout_path.clone();
            let container_bounds = container_bounds.clone();
            move |event, _window, cx| {
                let bounds = *container_bounds.borrow();

                // Snapshot current sizes for delta-based resize
                let initial_sizes = workspace.read(cx).project(&project_id).and_then(|p| {
                    p.layout.as_ref()?.get_at_path(&layout_path)
                }).and_then(|node| {
                    if let crate::workspace::state::LayoutNode::Split { sizes, .. } = node {
                        Some(sizes.clone())
                    } else {
                        None
                    }
                }).unwrap_or_default();

                *active_drag.borrow_mut() = Some(DragState::Split {
                    project_id: project_id.clone(),
                    layout_path: layout_path.clone(),
                    child_index,
                    direction,
                    container_bounds: bounds,
                    initial_mouse_pos: event.position,
                    initial_sizes,
                });
                cx.stop_propagation();
            }
        })
        .child(
            div()
                .when(is_horizontal, |d| d.h(px(1.0)).w_full())
                .when(!is_horizontal, |d| d.w(px(1.0)).h_full())
                .bg(rgb(t.border))
                .group_hover("split-handle", |s| s.bg(rgb(t.border_active))),
        )
}

/// Render a project column divider
pub fn render_project_divider(
    divider_index: usize,
    project_ids: Vec<String>,
    container_bounds: Rc<RefCell<Bounds<Pixels>>>,
    active_drag: &ActiveDrag,
    cx: &App,
) -> impl IntoElement {
    let t = theme(cx);
    let active_drag = active_drag.clone();

    div()
        .id(ElementId::Name(format!("project-divider-{}", divider_index).into()))
        .group("project-divider")
        .w(px(5.0))
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::ResizeLeftRight)
        .on_mouse_down(MouseButton::Left, {
            let active_drag = active_drag.clone();
            let project_ids = project_ids.clone();
            let container_bounds = container_bounds.clone();
            move |_event, _window, cx| {
                let bounds = *container_bounds.borrow();
                *active_drag.borrow_mut() = Some(DragState::ProjectColumn {
                    divider_index,
                    project_ids: project_ids.clone(),
                    container_bounds: bounds,
                });
                cx.stop_propagation();
            }
        })
        .child(
            div()
                .w(px(1.0))
                .h_full()
                .bg(rgb(t.border))
                .group_hover("project-divider", |s| s.bg(rgb(t.border_active))),
        )
}

/// Render the sidebar resize divider
pub fn render_sidebar_divider(active_drag: &ActiveDrag, cx: &App) -> impl IntoElement {
    let t = theme(cx);
    let active_drag = active_drag.clone();

    div()
        .id("sidebar-divider")
        .group("sidebar-divider")
        .w(px(5.0))
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::ResizeLeftRight)
        .on_mouse_down(MouseButton::Left, {
            let active_drag = active_drag.clone();
            move |_event, _window, cx| {
                *active_drag.borrow_mut() = Some(DragState::Sidebar);
                cx.stop_propagation();
            }
        })
        .child(
            div()
                .w(px(1.0))
                .h_full()
                .bg(rgb(t.border))
                .group_hover("sidebar-divider", |s| s.bg(rgb(t.border_active))),
        )
}
