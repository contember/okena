use crate::ActionDispatch;
use crate::elements::resize_handle::ResizeHandle;
use gpui::*;
use okena_files::theme::theme;
use okena_workspace::state::{SplitDirection, WindowId, Workspace};
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
        left_child: usize,
        right_child: usize,
        direction: SplitDirection,
        container_bounds: Bounds<Pixels>,
        initial_mouse_pos: Point<Pixels>,
        initial_sizes: Vec<f32>,
        visible_sizes_sum: f32,
        action_dispatcher: Option<Box<dyn ActionDispatchClone>>,
    },
    /// Resizing project columns (or rows, when `vertical`)
    ProjectColumn {
        divider_index: usize,
        project_ids: Vec<String>,
        /// Available space along the resize axis (width for columns, height
        /// for rows), minus divider thickness.
        available_size: f32,
        /// Sum of the raw widths for the currently visible projects.
        visible_widths_sum: f32,
        /// When true the projects are stacked as rows, so the drag tracks the
        /// vertical axis instead of the horizontal one.
        vertical: bool,
        initial_mouse_pos: Point<Pixels>,
        initial_widths: HashMap<String, f32>,
        min_col_width: f32,
    },
    /// Resizing sidebar width
    Sidebar,
    /// Resizing per-project service panel height
    ServicePanel {
        project_id: String,
        initial_mouse_y: f32,
        initial_height: f32,
    },
    /// Resizing per-project hook panel height
    HookPanel {
        project_id: String,
        initial_mouse_y: f32,
        initial_height: f32,
    },
}

/// Trait object wrapper for ActionDispatch in DragState (needs Clone).
pub trait ActionDispatchClone: Send + Sync {
    fn dispatch_action(&self, action: okena_core::api::ActionRequest, cx: &mut gpui::App);
    fn clone_box(&self) -> Box<dyn ActionDispatchClone>;
}

impl<T: ActionDispatch + Send + Sync> ActionDispatchClone for T {
    fn dispatch_action(&self, action: okena_core::api::ActionRequest, cx: &mut gpui::App) {
        self.dispatch(action, cx);
    }
    fn clone_box(&self) -> Box<dyn ActionDispatchClone> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn ActionDispatchClone> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

pub type ActiveDrag = Rc<RefCell<Option<DragState>>>;

/// Create a new active drag handle.
pub fn new_active_drag() -> ActiveDrag {
    Rc::new(RefCell::new(None))
}

fn resize_project_pair(
    left_initial: f32,
    right_initial: f32,
    visible_widths_sum: f32,
    delta_px: f32,
    container_size: f32,
    min_col_width: f32,
) -> (f32, f32) {
    let scale = if visible_widths_sum > 0.0 {
        visible_widths_sum
    } else {
        100.0
    };
    let delta_width = delta_px / container_size * scale;
    let min_width = (min_col_width / container_size * scale).max(scale * 0.05);
    let combined_width = (left_initial + right_initial).max(2.0 * min_width);
    let left_width = (left_initial + delta_width).clamp(min_width, combined_width - min_width);

    (left_width, combined_width - left_width)
}

/// Helper to compute and apply resize based on mouse position.
///
/// The `window_id` parameter selects which window's `project_widths` slot
/// receives the dragged column widths in the `DragState::ProjectColumn`
/// arm. Mirrors `render_project_divider`'s parameter-threaded shape: the
/// caller (today `WindowView`'s mouse-move listener) passes its own
/// `WindowView::window_id` so a drag in window N writes back to window
/// N's per-column widths.
pub fn compute_resize(
    window_id: WindowId,
    mouse_pos: Point<Pixels>,
    drag_state: &DragState,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    match drag_state {
        DragState::Split {
            project_id,
            layout_path,
            left_child,
            right_child,
            direction,
            container_bounds,
            initial_mouse_pos,
            initial_sizes,
            visible_sizes_sum,
            action_dispatcher,
        } => {
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

            let combined_size = initial_sizes[left_child] + initial_sizes[right_child];

            let delta = if is_horizontal {
                f32::from(mouse_pos.y) - f32::from(initial_mouse_pos.y)
            } else {
                f32::from(mouse_pos.x) - f32::from(initial_mouse_pos.x)
            };
            let scale = if *visible_sizes_sum > 0.0 {
                *visible_sizes_sum
            } else {
                100.0
            };
            let delta_percent = delta / container_size * scale;

            let min_size = scale * 0.05;
            let combined_size = combined_size.max(2.0 * min_size);
            let max_size = combined_size - min_size;
            let left_size = (initial_sizes[left_child] + delta_percent).clamp(min_size, max_size);
            let right_size = combined_size - left_size;

            let mut new_sizes = initial_sizes.clone();
            new_sizes[left_child] = left_size;
            new_sizes[right_child] = right_size;

            let project_id = project_id.clone();
            let layout_path = layout_path.clone();

            if let Some(dispatcher) = action_dispatcher {
                dispatcher.dispatch_action(
                    okena_core::api::ActionRequest::UpdateSplitSizes {
                        project_id,
                        path: layout_path,
                        sizes: new_sizes,
                    },
                    cx,
                );
            } else {
                // Use UI-only notify during drag to avoid auto-save spam;
                // final sizes are persisted on mouse-up via notify_data.
                workspace.update(cx, |ws, cx| {
                    ws.update_split_sizes_ui_only(&project_id, &layout_path, new_sizes, cx);
                });
            }
        }
        DragState::ProjectColumn {
            divider_index,
            project_ids,
            available_size,
            visible_widths_sum,
            vertical,
            initial_mouse_pos,
            initial_widths,
            min_col_width,
        } => {
            let container_size = *available_size;
            if container_size <= 0.0 {
                return;
            }

            let divider_index = *divider_index;
            let left_id = &project_ids[divider_index];
            let right_id = &project_ids[divider_index + 1];

            let num_projects = project_ids.len();
            let default_width = 100.0 / num_projects as f32;
            let left_initial = initial_widths
                .get(left_id)
                .copied()
                .unwrap_or(default_width);
            let right_initial = initial_widths
                .get(right_id)
                .copied()
                .unwrap_or(default_width);

            let delta_px = if *vertical {
                f32::from(mouse_pos.y) - f32::from(initial_mouse_pos.y)
            } else {
                f32::from(mouse_pos.x) - f32::from(initial_mouse_pos.x)
            };
            let (left_new, right_new) = resize_project_pair(
                left_initial,
                right_initial,
                *visible_widths_sum,
                delta_px,
                container_size,
                *min_col_width,
            );

            let mut new_widths = initial_widths.clone();
            new_widths.insert(left_id.clone(), left_new);
            new_widths.insert(right_id.clone(), right_new);

            workspace.update(cx, |ws, cx| {
                ws.update_project_widths(window_id, new_widths, cx);
            });
        }
        DragState::Sidebar | DragState::ServicePanel { .. } | DragState::HookPanel { .. } => {
            // Handled directly in WindowView's on_mouse_move
        }
    }
}

/// Render an inline split divider handle element
// GPUI render helper: params are layout/render inputs, not a cohesive group.
#[allow(clippy::too_many_arguments)]
pub fn render_split_divider<D: ActionDispatch + Send + Sync>(
    workspace: Entity<Workspace>,
    project_id: String,
    left_child_idx: usize,
    right_child_idx: usize,
    direction: SplitDirection,
    layout_path: Vec<usize>,
    container_bounds: Rc<RefCell<Bounds<Pixels>>>,
    active_drag: &ActiveDrag,
    action_dispatcher: Option<D>,
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

            let (initial_sizes, visible_sizes_sum) = workspace
                .read(cx)
                .project(&project_id)
                .and_then(|p| p.layout.as_ref()?.get_at_path(&layout_path))
                .and_then(|node| {
                    if let okena_workspace::state::LayoutNode::Split {
                        sizes, children, ..
                    } = node
                    {
                        let visible_sum: f32 = children
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| !c.is_all_hidden())
                            .map(|(i, _)| sizes.get(i).copied().unwrap_or(0.0))
                            .sum();
                        Some((sizes.clone(), visible_sum))
                    } else {
                        None
                    }
                })
                .unwrap_or((vec![], 100.0));

            let boxed_dispatcher: Option<Box<dyn ActionDispatchClone>> = action_dispatcher
                .as_ref()
                .map(|d| Box::new(d.clone()) as Box<dyn ActionDispatchClone>);

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
                action_dispatcher: boxed_dispatcher,
            });
        },
    )
}

/// Render a project column divider.
///
/// The `window_id` parameter selects which window's `project_widths` slot supplies
/// the per-column starting widths for the drag. Today every caller passes
/// `WindowId::Main` because the runtime is single-window; once extras land
/// (slice 05) each caller will pass its own `WindowView::window_id` so that a
/// drag on column N starts from the same width the user sees in that window.
// Render helper: params are render inputs (geometry, theme, window slot, callbacks).
#[allow(clippy::too_many_arguments)]
pub fn render_project_divider(
    window_id: WindowId,
    workspace: Entity<Workspace>,
    divider_index: usize,
    project_ids: Vec<String>,
    container_bounds: Rc<RefCell<Bounds<Pixels>>>,
    active_drag: &ActiveDrag,
    min_col_width: f32,
    is_rows: bool,
    cx: &App,
) -> impl IntoElement {
    let t = theme(cx);
    let active_drag = active_drag.clone();

    // A rows grid needs a horizontal divider (full width, drag along Y); a
    // columns grid needs a vertical divider (full height, drag along X).
    ResizeHandle::new(is_rows, t.border, t.border_active, move |mouse_pos, cx| {
        let bounds = *container_bounds.borrow();
        let num_projects = project_ids.len();
        let num_dividers = num_projects.saturating_sub(1) as f32;

        let viewport_size = if is_rows {
            f32::from(bounds.size.height)
        } else {
            f32::from(bounds.size.width)
        };
        let available_size = (viewport_size - num_dividers * 1.0).max(0.0);

        let ws = workspace.read(cx);
        let initial_widths: HashMap<String, f32> = project_ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    ws.get_project_width(window_id, id, num_projects),
                )
            })
            .collect();
        let visible_widths_sum = initial_widths.values().sum();

        *active_drag.borrow_mut() = Some(DragState::ProjectColumn {
            divider_index,
            project_ids: project_ids.clone(),
            available_size,
            visible_widths_sum,
            vertical: is_rows,
            initial_mouse_pos: mouse_pos,
            initial_widths,
            min_col_width,
        });
    })
}

/// Render the sidebar resize divider
pub fn render_sidebar_divider(active_drag: &ActiveDrag, cx: &App) -> impl IntoElement {
    let t = theme(cx);
    let active_drag = active_drag.clone();

    ResizeHandle::new(false, t.border, t.border_active, move |_, _| {
        *active_drag.borrow_mut() = Some(DragState::Sidebar);
    })
}

#[cfg(test)]
mod tests {
    use super::resize_project_pair;

    fn rendered_width(weight: f32, total: f32, container_size: f32) -> f32 {
        weight / total * container_size
    }

    #[test]
    fn project_resize_tracks_mouse_when_visible_widths_sum_below_100() {
        let container_size = 1_000.0;
        let visible_sum = 50.0;
        let initial_left = 20.0;
        let initial_right = 30.0;

        let (left, right) = resize_project_pair(
            initial_left,
            initial_right,
            visible_sum,
            100.0,
            container_size,
            50.0,
        );

        let initial_px = rendered_width(initial_left, visible_sum, container_size);
        let resized_px = rendered_width(left, left + right, container_size);
        assert!((resized_px - initial_px - 100.0).abs() < 0.001);
    }

    #[test]
    fn project_resize_tracks_mouse_when_visible_widths_sum_above_100() {
        let container_size = 1_000.0;
        let visible_sum = 200.0;
        let initial_left = 80.0;
        let initial_right = 120.0;

        let (left, right) = resize_project_pair(
            initial_left,
            initial_right,
            visible_sum,
            100.0,
            container_size,
            50.0,
        );

        let initial_px = rendered_width(initial_left, visible_sum, container_size);
        let resized_px = rendered_width(left, left + right, container_size);
        assert!((resized_px - initial_px - 100.0).abs() < 0.001);
    }

    #[test]
    fn project_resize_preserves_pair_total_at_minimum_width() {
        let (left, right) = resize_project_pair(60.0, 40.0, 100.0, 500.0, 1_000.0, 200.0);

        assert_eq!(left, 80.0);
        assert_eq!(right, 20.0);
        assert_eq!(left + right, 100.0);
    }
}
