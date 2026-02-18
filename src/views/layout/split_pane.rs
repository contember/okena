use crate::elements::resize_handle::ResizeHandle;
use crate::theme::theme;
use crate::workspace::state::{SplitDirection, Workspace};
use gpui::*;
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
        /// Original index of the child to the left of the divider
        left_child: usize,
        /// Original index of the child to the right of the divider
        right_child: usize,
        direction: SplitDirection,
        container_bounds: Bounds<Pixels>,
        /// Mouse position at drag start (for delta-based resize)
        initial_mouse_pos: Point<Pixels>,
        /// Sizes snapshot at drag start (all children, including hidden)
        initial_sizes: Vec<f32>,
        /// Sum of visible children's sizes (for correct delta scaling)
        visible_sizes_sum: f32,
    },
    /// Resizing project columns
    ProjectColumn {
        divider_index: usize,
        project_ids: Vec<String>,
        container_bounds: Bounds<Pixels>,
        /// Mouse position at drag start (for delta-based resize)
        initial_mouse_pos: Point<Pixels>,
        /// Width snapshots at drag start (project_id -> width %)
        initial_widths: HashMap<String, f32>,
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
        DragState::Split { project_id, layout_path, left_child, right_child, direction, container_bounds, initial_mouse_pos, initial_sizes, visible_sizes_sum } => {
            let bounds = *container_bounds;
            let is_horizontal = *direction == SplitDirection::Horizontal;
            let left_child = *left_child;
            let right_child = *right_child;

            let container_size = if is_horizontal {
                f32::from(bounds.size.height)
            } else {
                f32::from(bounds.size.width)
            };

            if container_size <= 0.0 {
                return;
            }

            if left_child >= initial_sizes.len() || right_child >= initial_sizes.len() {
                return;
            }

            // Combined size of the two children being resized
            let combined_size = initial_sizes[left_child] + initial_sizes[right_child];

            // Delta-based resize: compute mouse movement since drag start
            // Scale by visible_sizes_sum so 1px of mouse movement = correct proportion
            // (when hidden children exist, visible sizes don't sum to 100)
            let delta = if is_horizontal {
                f32::from(mouse_pos.y) - f32::from(initial_mouse_pos.y)
            } else {
                f32::from(mouse_pos.x) - f32::from(initial_mouse_pos.x)
            };
            let scale = if *visible_sizes_sum > 0.0 { *visible_sizes_sum } else { 100.0 };
            let delta_percent = delta / container_size * scale;

            // Sizes are relative weights — min is 5% of total visible sum
            let min_size = scale * 0.05;
            // Ensure combined size is at least 2*min so both sides stay positive
            let combined_size = combined_size.max(2.0 * min_size);
            let max_size = combined_size - min_size;
            let left_size = (initial_sizes[left_child] + delta_percent).clamp(min_size, max_size);
            let right_size = combined_size - left_size;

            // Build new sizes: keep all others unchanged, update only the two children
            let mut new_sizes = initial_sizes.clone();
            new_sizes[left_child] = left_size;
            new_sizes[right_child] = right_size;

            let project_id = project_id.clone();
            let layout_path = layout_path.clone();

            workspace.update(cx, |ws, cx| {
                ws.update_split_sizes(&project_id, &layout_path, new_sizes, cx);
            });
        }
        DragState::ProjectColumn { divider_index, project_ids, container_bounds, initial_mouse_pos, initial_widths } => {
            let bounds = *container_bounds;
            let container_width = f32::from(bounds.size.width);
            if container_width <= 0.0 {
                return;
            }

            let divider_index = *divider_index;
            let left_id = &project_ids[divider_index];
            let right_id = &project_ids[divider_index + 1];

            let num_projects = project_ids.len();
            let default_width = 100.0 / num_projects as f32;
            let left_initial = initial_widths.get(left_id).copied().unwrap_or(default_width);
            let right_initial = initial_widths.get(right_id).copied().unwrap_or(default_width);
            let combined = left_initial + right_initial;

            // Delta-based: only adjust the two adjacent projects
            let delta_px = f32::from(mouse_pos.x) - f32::from(initial_mouse_pos.x);
            let delta_percent = delta_px / container_width * 100.0;

            let min_width = 5.0_f32;
            let max_width = (combined - min_width).max(min_width);
            let left_new = (left_initial + delta_percent).clamp(min_width, max_width);
            let right_new = combined - left_new;

            let mut new_widths = initial_widths.clone();
            new_widths.insert(left_id.clone(), left_new);
            new_widths.insert(right_id.clone(), right_new);

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
    left_child_idx: usize,
    right_child_idx: usize,
    direction: SplitDirection,
    layout_path: Vec<usize>,
    container_bounds: Rc<RefCell<Bounds<Pixels>>>,
    active_drag: &ActiveDrag,
    cx: &App,
) -> impl IntoElement {
    let t = theme(cx);
    let active_drag = active_drag.clone();

    ResizeHandle::new(
        direction == SplitDirection::Horizontal,
        t.border,
        t.border_active,
        move |mouse_pos, cx| {
            let bounds = *container_bounds.borrow();

            let (initial_sizes, visible_sizes_sum) = workspace.read(cx).project(&project_id).and_then(|p| {
                p.layout.as_ref()?.get_at_path(&layout_path)
            }).and_then(|node| {
                if let crate::workspace::state::LayoutNode::Split { sizes, children, .. } = node {
                    let visible_sum: f32 = children.iter().enumerate()
                        .filter(|(_, c)| !c.is_all_hidden())
                        .map(|(i, _)| sizes.get(i).copied().unwrap_or(0.0))
                        .sum();
                    Some((sizes.clone(), visible_sum))
                } else {
                    None
                }
            }).unwrap_or((vec![], 100.0));

            *active_drag.borrow_mut() = Some(DragState::Split {
                project_id: project_id.clone(),
                layout_path: layout_path.clone(),
                left_child: left_child_idx,
                right_child: right_child_idx,
                direction,
                container_bounds: bounds,
                initial_mouse_pos: mouse_pos,
                initial_sizes,
                visible_sizes_sum,
            });
        },
    )
}

/// Render a project column divider
pub fn render_project_divider(
    workspace: Entity<Workspace>,
    divider_index: usize,
    project_ids: Vec<String>,
    container_bounds: Rc<RefCell<Bounds<Pixels>>>,
    active_drag: &ActiveDrag,
    cx: &App,
) -> impl IntoElement {
    let t = theme(cx);
    let active_drag = active_drag.clone();

    ResizeHandle::new(
        false,
        t.border,
        t.border_active,
        move |mouse_pos, cx| {
            let bounds = *container_bounds.borrow();
            let num_projects = project_ids.len();

            // Snapshot current widths at drag start
            let ws = workspace.read(cx);
            let initial_widths: HashMap<String, f32> = project_ids.iter()
                .map(|id| (id.clone(), ws.get_project_width(id, num_projects)))
                .collect();

            *active_drag.borrow_mut() = Some(DragState::ProjectColumn {
                divider_index,
                project_ids: project_ids.clone(),
                container_bounds: bounds,
                initial_mouse_pos: mouse_pos,
                initial_widths,
            });
        },
    )
}

/// Render the sidebar resize divider
pub fn render_sidebar_divider(active_drag: &ActiveDrag, cx: &App) -> impl IntoElement {
    let t = theme(cx);
    let active_drag = active_drag.clone();

    ResizeHandle::new(
        false,
        t.border,
        t.border_active,
        move |_, _| {
            *active_drag.borrow_mut() = Some(DragState::Sidebar);
        },
    )
}
