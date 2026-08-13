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
        /// Pixels represented by one stored project-width unit.
        width_scale: f32,
        /// Rendered size of the full project strip when the drag started.
        initial_content_size: f32,
        /// Resize only the project before the divider.
        resize_leading_only: bool,
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

fn resize_project_pair_px(
    left_initial: f32,
    right_initial: f32,
    delta_px: f32,
    min_col_width: f32,
    initial_content_size: f32,
    available_size: f32,
    resize_leading_only: bool,
) -> (f32, f32) {
    let left_capacity = (left_initial - min_col_width).max(0.0);

    if resize_leading_only {
        return ((left_initial + delta_px).max(min_col_width), right_initial);
    }

    if initial_content_size > available_size + 0.5 {
        if delta_px >= 0.0 {
            return (left_initial + delta_px, right_initial);
        }

        let requested = -delta_px;
        let overflow = initial_content_size - available_size;
        let strip_shrink = requested.min(overflow).min(left_capacity);
        let left_after_shrink = left_initial - strip_shrink;
        let transfer = (requested - strip_shrink).min((left_after_shrink - min_col_width).max(0.0));

        return (left_after_shrink - transfer, right_initial + transfer);
    }

    if delta_px >= 0.0 {
        let transfer = delta_px.min((right_initial - min_col_width).max(0.0));
        (left_initial + delta_px, right_initial - transfer)
    } else {
        let transfer = (-delta_px).min(left_capacity);
        (left_initial - transfer, right_initial + transfer)
    }
}

/// Resolve the stable pixel scale for a set of project-size weights.
pub fn project_width_scale(
    widths: &[f32],
    available_size: f32,
    persisted_scale: Option<f32>,
) -> f32 {
    let widths_sum: f32 = widths.iter().copied().filter(|width| *width > 0.0).sum();
    if widths_sum <= 0.0 || available_size <= 0.0 {
        return 0.0;
    }

    persisted_scale
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(available_size / widths_sum)
}

/// Convert project-size weights to rendered pixels without losing overflow.
pub fn project_pixel_widths(
    widths: &[f32],
    available_size: f32,
    min_col_width: f32,
    persisted_scale: Option<f32>,
) -> Vec<f32> {
    let scale = project_width_scale(widths, available_size, persisted_scale);

    widths
        .iter()
        .map(|width| (*width * scale).max(min_col_width))
        .collect()
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
            width_scale,
            initial_content_size,
            resize_leading_only,
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
            let (left_new_px, right_new_px) = resize_project_pair_px(
                left_initial * *width_scale,
                right_initial * *width_scale,
                delta_px,
                *min_col_width,
                *initial_content_size,
                container_size,
                *resize_leading_only,
            );
            let left_new = left_new_px / *width_scale;
            let right_new = right_new_px / *width_scale;

            let mut new_widths = initial_widths.clone();
            new_widths.insert(left_id.clone(), left_new);
            new_widths.insert(right_id.clone(), right_new);

            workspace.update(cx, |ws, cx| {
                ws.update_project_widths_with_scale(window_id, new_widths, *width_scale, cx);
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
    ResizeHandle::new_with_modifiers(
        is_rows,
        t.border,
        t.border_active,
        move |mouse_pos, modifiers, cx| {
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
            let raw_widths: HashMap<String, f32> = project_ids
                .iter()
                .map(|id| {
                    (
                        id.clone(),
                        ws.get_project_width(window_id, id, num_projects),
                    )
                })
                .collect();
            let raw_width_values: Vec<f32> = project_ids
                .iter()
                .filter_map(|id| raw_widths.get(id).copied())
                .collect();
            let width_scale = project_width_scale(
                &raw_width_values,
                available_size,
                ws.get_project_width_scale(window_id),
            );
            if width_scale <= 0.0 {
                return;
            }

            let initial_widths: HashMap<String, f32> = raw_widths
                .into_iter()
                .map(|(id, width)| {
                    let rendered = (width * width_scale).max(min_col_width);
                    (id, rendered / width_scale)
                })
                .collect();
            let initial_content_size = initial_widths.values().sum::<f32>() * width_scale;

            *active_drag.borrow_mut() = Some(DragState::ProjectColumn {
                divider_index,
                project_ids: project_ids.clone(),
                available_size,
                width_scale,
                initial_content_size,
                resize_leading_only: modifiers.shift,
                vertical: is_rows,
                initial_mouse_pos: mouse_pos,
                initial_widths,
                min_col_width,
            });
        },
    )
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
    use super::{project_pixel_widths, project_width_scale, resize_project_pair_px};

    #[test]
    fn project_width_scale_fits_legacy_weights_to_viewport() {
        assert_eq!(project_width_scale(&[20.0, 30.0], 1_000.0, None), 20.0);
    }

    #[test]
    fn persisted_project_width_scale_preserves_overflow() {
        let widths = project_pixel_widths(&[31.25, 25.0, 25.0], 1_000.0, 400.0, Some(16.0));
        assert_eq!(widths, vec![500.0, 400.0, 400.0]);
    }

    #[test]
    fn persisted_project_width_scale_preserves_underfill() {
        let widths = project_pixel_widths(&[40.0, 40.0], 1_600.0, 0.0, Some(10.0));
        assert_eq!(widths, vec![400.0, 400.0]);
    }

    #[test]
    fn equal_weights_refit_the_viewport_only_once_the_stale_scale_is_dropped() {
        // Dragged weights stop summing to 100, so the persisted scale is
        // pixels-per-unit for *that* sum. Equalize falls back to `100 / n`
        // weights: keeping the scale renders `100 * scale` (an overflowing
        // grid — worst in stacked rows, where it overflows the height),
        // dropping it refits them to the viewport.
        let viewport = 3_016.0;
        let equal = [100.0 / 3.0; 3];

        let stale: f32 = project_pixel_widths(&equal, viewport, 0.0, Some(35.4))
            .iter()
            .sum();
        assert!(stale > viewport + 500.0, "{stale} should overflow");

        let refitted: f32 = project_pixel_widths(&equal, viewport, 0.0, None)
            .iter()
            .sum();
        assert!((refitted - viewport).abs() < 0.01, "{refitted}");
    }

    #[test]
    fn fitting_resize_transfers_space_until_neighbor_reaches_minimum() {
        let (left, right) =
            resize_project_pair_px(500.0, 500.0, 300.0, 400.0, 1_000.0, 1_000.0, false);
        assert_eq!((left, right), (800.0, 400.0));
    }

    #[test]
    fn overflowing_resize_keeps_following_project_unchanged() {
        let (left, right) =
            resize_project_pair_px(400.0, 400.0, 100.0, 400.0, 3_200.0, 1_600.0, false);
        assert_eq!((left, right), (500.0, 400.0));
    }

    #[test]
    fn eight_project_resize_grows_scroll_strip_without_shrinking_neighbor() {
        let scale = project_width_scale(&[12.5; 8], 1_600.0, None);
        let initial = project_pixel_widths(&[12.5; 8], 1_600.0, 400.0, None);
        let mut resized: Vec<f32> = initial.iter().map(|width| width / scale).collect();
        let (left, right) = resize_project_pair_px(
            initial[0], initial[1], 100.0, 400.0, 3_200.0, 1_600.0, false,
        );

        resized[0] = left / scale;
        resized[1] = right / scale;

        assert_eq!(
            project_pixel_widths(&resized, 1_600.0, 400.0, Some(scale)),
            vec![500.0, 400.0, 400.0, 400.0, 400.0, 400.0, 400.0, 400.0]
        );
    }

    #[test]
    fn shrinking_overflow_transitions_back_to_pair_resize() {
        let (left, right) =
            resize_project_pair_px(700.0, 400.0, -200.0, 400.0, 1_100.0, 1_000.0, false);
        assert_eq!((left, right), (500.0, 500.0));
    }

    #[test]
    fn shift_resize_changes_only_leading_project() {
        assert_eq!(
            resize_project_pair_px(500.0, 500.0, 100.0, 400.0, 1_000.0, 1_000.0, true),
            (600.0, 500.0)
        );
        assert_eq!(
            resize_project_pair_px(500.0, 500.0, -100.0, 400.0, 1_000.0, 1_000.0, true),
            (400.0, 500.0)
        );
    }
}
