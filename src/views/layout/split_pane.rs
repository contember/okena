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
    /// Resizing a grid row divider
    GridRow {
        project_id: String,
        layout_path: Vec<usize>,
        top_row: usize,
        bottom_row: usize,
        container_bounds: Bounds<Pixels>,
        initial_mouse_pos: Point<Pixels>,
        initial_row_sizes: Vec<f32>,
    },
    /// Resizing a grid column divider
    GridCol {
        project_id: String,
        layout_path: Vec<usize>,
        left_col: usize,
        right_col: usize,
        container_bounds: Bounds<Pixels>,
        initial_mouse_pos: Point<Pixels>,
        initial_col_sizes: Vec<f32>,
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

            let left_size = (initial_sizes[left_child] + delta_percent).clamp(5.0, combined_size - 5.0);
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
        DragState::GridRow { project_id, layout_path, top_row, bottom_row, container_bounds, initial_mouse_pos, initial_row_sizes } => {
            let bounds = *container_bounds;
            let container_height = f32::from(bounds.size.height);
            if container_height <= 0.0 { return; }
            let top = *top_row;
            let bottom = *bottom_row;
            if top >= initial_row_sizes.len() || bottom >= initial_row_sizes.len() { return; }
            let combined = initial_row_sizes[top] + initial_row_sizes[bottom];
            let total: f32 = initial_row_sizes.iter().sum();
            let scale = if total > 0.0 { total } else { 100.0 };
            let delta = f32::from(mouse_pos.y) - f32::from(initial_mouse_pos.y);
            let delta_percent = delta / container_height * scale;
            let top_size = (initial_row_sizes[top] + delta_percent).clamp(5.0, combined - 5.0);
            let bottom_size = combined - top_size;
            let mut new_sizes = initial_row_sizes.clone();
            new_sizes[top] = top_size;
            new_sizes[bottom] = bottom_size;
            let project_id = project_id.clone();
            let layout_path = layout_path.clone();
            workspace.update(cx, |ws, cx| {
                ws.update_grid_row_sizes(&project_id, &layout_path, new_sizes, cx);
            });
        }
        DragState::GridCol { project_id, layout_path, left_col, right_col, container_bounds, initial_mouse_pos, initial_col_sizes } => {
            let bounds = *container_bounds;
            let container_width = f32::from(bounds.size.width);
            if container_width <= 0.0 { return; }
            let left = *left_col;
            let right = *right_col;
            if left >= initial_col_sizes.len() || right >= initial_col_sizes.len() { return; }
            let combined = initial_col_sizes[left] + initial_col_sizes[right];
            let total: f32 = initial_col_sizes.iter().sum();
            let scale = if total > 0.0 { total } else { 100.0 };
            let delta = f32::from(mouse_pos.x) - f32::from(initial_mouse_pos.x);
            let delta_percent = delta / container_width * scale;
            let left_size = (initial_col_sizes[left] + delta_percent).clamp(5.0, combined - 5.0);
            let right_size = combined - left_size;
            let mut new_sizes = initial_col_sizes.clone();
            new_sizes[left] = left_size;
            new_sizes[right] = right_size;
            let project_id = project_id.clone();
            let layout_path = layout_path.clone();
            workspace.update(cx, |ws, cx| {
                ws.update_grid_col_sizes(&project_id, &layout_path, new_sizes, cx);
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
        move |_, _| {
            let bounds = *container_bounds.borrow();
            *active_drag.borrow_mut() = Some(DragState::ProjectColumn {
                divider_index,
                project_ids: project_ids.clone(),
                container_bounds: bounds,
            });
        },
    )
}

/// Render a grid row (horizontal) divider handle
pub fn render_grid_row_divider(
    workspace: Entity<Workspace>,
    project_id: String,
    top_row: usize,
    bottom_row: usize,
    layout_path: Vec<usize>,
    container_bounds: Rc<RefCell<Bounds<Pixels>>>,
    active_drag: &ActiveDrag,
    cx: &App,
) -> impl IntoElement {
    let t = theme(cx);
    let active_drag = active_drag.clone();

    ResizeHandle::new(
        true, // horizontal divider
        t.border,
        t.border_active,
        move |mouse_pos, cx| {
            let bounds = *container_bounds.borrow();

            let initial_row_sizes = workspace.read(cx).project(&project_id).and_then(|p| {
                p.layout.as_ref()?.get_at_path(&layout_path)
            }).and_then(|node| {
                if let crate::workspace::state::LayoutNode::Grid { row_sizes, .. } = node {
                    Some(row_sizes.clone())
                } else {
                    None
                }
            }).unwrap_or_default();

            *active_drag.borrow_mut() = Some(DragState::GridRow {
                project_id: project_id.clone(),
                layout_path: layout_path.clone(),
                top_row,
                bottom_row,
                container_bounds: bounds,
                initial_mouse_pos: mouse_pos,
                initial_row_sizes,
            });
        },
    )
}

/// Render a grid column (vertical) divider handle
pub fn render_grid_col_divider(
    workspace: Entity<Workspace>,
    project_id: String,
    left_col: usize,
    right_col: usize,
    layout_path: Vec<usize>,
    container_bounds: Rc<RefCell<Bounds<Pixels>>>,
    active_drag: &ActiveDrag,
    cx: &App,
) -> impl IntoElement {
    let t = theme(cx);
    let active_drag = active_drag.clone();

    ResizeHandle::new(
        false, // vertical divider
        t.border,
        t.border_active,
        move |mouse_pos, cx| {
            let bounds = *container_bounds.borrow();

            let initial_col_sizes = workspace.read(cx).project(&project_id).and_then(|p| {
                p.layout.as_ref()?.get_at_path(&layout_path)
            }).and_then(|node| {
                if let crate::workspace::state::LayoutNode::Grid { col_sizes, .. } = node {
                    Some(col_sizes.clone())
                } else {
                    None
                }
            }).unwrap_or_default();

            *active_drag.borrow_mut() = Some(DragState::GridCol {
                project_id: project_id.clone(),
                layout_path: layout_path.clone(),
                left_col,
                right_col,
                container_bounds: bounds,
                initial_mouse_pos: mouse_pos,
                initial_col_sizes,
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
