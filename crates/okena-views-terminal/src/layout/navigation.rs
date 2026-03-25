//! Spatial navigation for terminal panes
//!
//! This module provides arrow key navigation between terminal panes using
//! a spatial map of pane bounds. Navigation finds the nearest pane in the
//! requested direction using center-point distance calculation.

use gpui::*;

/// Direction for spatial navigation
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Information about a terminal pane's position
#[derive(Clone)]
pub struct PaneBounds {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub bounds: Bounds<Pixels>,
    /// Enables direct focus transfer, bypassing multi-frame delay from nested cached views.
    pub focus_handle: Option<FocusHandle>,
}

impl std::fmt::Debug for PaneBounds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneBounds")
            .field("project_id", &self.project_id)
            .field("layout_path", &self.layout_path)
            .field("bounds", &self.bounds)
            .field("focus_handle", &self.focus_handle.as_ref().map(|_| "..."))
            .finish()
    }
}

/// Spatial map of all visible terminal panes
#[derive(Default, Clone)]
pub struct PaneMap {
    panes: Vec<PaneBounds>,
}

impl PaneMap {
    pub fn new() -> Self {
        Self { panes: Vec::new() }
    }

    /// Register (or update) a pane's bounds.
    /// Uses upsert semantics so cached views that skip prepaint keep their entry.
    pub fn register(&mut self, project_id: String, layout_path: Vec<usize>, bounds: Bounds<Pixels>, focus_handle: Option<FocusHandle>) {
        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
            return;
        }

        if let Some(existing) = self.panes.iter_mut().find(|p| {
            p.project_id == project_id && p.layout_path == layout_path
        }) {
            existing.bounds = bounds;
            if focus_handle.is_some() {
                existing.focus_handle = focus_handle;
            }
        } else {
            self.panes.push(PaneBounds {
                project_id,
                layout_path,
                bounds,
                focus_handle,
            });
        }
    }

    /// Remove a pane from the map (e.g. when the terminal pane is dropped).
    pub fn deregister(&mut self, project_id: &str, layout_path: &[usize]) {
        self.panes.retain(|p| {
            !(p.project_id == project_id && p.layout_path == layout_path)
        });
    }

    /// Find the pane at the given project_id and layout_path
    pub fn find_pane(&self, project_id: &str, layout_path: &[usize]) -> Option<&PaneBounds> {
        self.panes.iter().find(|p| {
            p.project_id == project_id && p.layout_path == layout_path
        })
    }

    /// Find the nearest pane in the given direction from the source pane
    pub fn find_nearest_in_direction(
        &self,
        source: &PaneBounds,
        direction: NavigationDirection,
    ) -> Option<&PaneBounds> {
        let source_center = source.bounds.center();

        self.panes.iter()
            .filter(|p| {
                if p.project_id == source.project_id && p.layout_path == source.layout_path {
                    return false;
                }

                let candidate_center = p.bounds.center();

                match direction {
                    NavigationDirection::Left => candidate_center.x < source_center.x,
                    NavigationDirection::Right => candidate_center.x > source_center.x,
                    NavigationDirection::Up => candidate_center.y < source_center.y,
                    NavigationDirection::Down => candidate_center.y > source_center.y,
                }
            })
            .min_by(|a, b| {
                let dist_a = weighted_distance(&source_center, &a.bounds.center(), direction);
                let dist_b = weighted_distance(&source_center, &b.bounds.center(), direction);
                dist_a.partial_cmp(&dist_b).unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Find the next pane in sequential order (cycles through all panes)
    pub fn find_next_pane(&self, source: &PaneBounds) -> Option<&PaneBounds> {
        if self.panes.len() <= 1 {
            return None;
        }

        let current_idx = self.panes.iter().position(|p| {
            p.project_id == source.project_id && p.layout_path == source.layout_path
        })?;

        let next_idx = (current_idx + 1) % self.panes.len();
        self.panes.get(next_idx)
    }

    /// Get all registered panes
    pub fn panes(&self) -> &[PaneBounds] {
        &self.panes
    }

    /// Return panes sorted by reading order: top-to-bottom, then left-to-right.
    #[allow(dead_code)]
    pub fn sorted_by_reading_order(&self) -> Vec<&PaneBounds> {
        let mut sorted: Vec<&PaneBounds> = self.panes.iter().collect();
        sorted.sort_by(|a, b| {
            let ay = f32::from(a.bounds.center().y);
            let by = f32::from(b.bounds.center().y);
            let ax = f32::from(a.bounds.center().x);
            let bx = f32::from(b.bounds.center().x);
            ay.partial_cmp(&by)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal))
        });
        sorted
    }

    /// Find the previous pane in sequential order (cycles through all panes)
    pub fn find_prev_pane(&self, source: &PaneBounds) -> Option<&PaneBounds> {
        if self.panes.len() <= 1 {
            return None;
        }

        let current_idx = self.panes.iter().position(|p| {
            p.project_id == source.project_id && p.layout_path == source.layout_path
        })?;

        let prev_idx = if current_idx == 0 {
            self.panes.len() - 1
        } else {
            current_idx - 1
        };
        self.panes.get(prev_idx)
    }
}

/// Calculate weighted distance favoring the navigation direction axis
fn weighted_distance(
    from: &Point<Pixels>,
    to: &Point<Pixels>,
    direction: NavigationDirection,
) -> f32 {
    let dx = f32::from(to.x) - f32::from(from.x);
    let dy = f32::from(to.y) - f32::from(from.y);

    let (primary_weight, secondary_weight) = match direction {
        NavigationDirection::Left | NavigationDirection::Right => (1.0, 2.0),
        NavigationDirection::Up | NavigationDirection::Down => (2.0, 1.0),
    };

    let weighted_dx = dx * primary_weight;
    let weighted_dy = dy * secondary_weight;

    (weighted_dx * weighted_dx) + (weighted_dy * weighted_dy)
}

/// Global pane map storage for the main window
static PANE_MAP: std::sync::OnceLock<parking_lot::Mutex<PaneMap>> = std::sync::OnceLock::new();

fn pane_map_lock() -> &'static parking_lot::Mutex<PaneMap> {
    PANE_MAP.get_or_init(|| parking_lot::Mutex::new(PaneMap::new()))
}

/// Get the global pane map
pub fn get_pane_map() -> PaneMap {
    pane_map_lock().lock().clone()
}

/// Register a pane's bounds in the global map
pub fn register_pane_bounds(
    project_id: String,
    layout_path: Vec<usize>,
    bounds: Bounds<Pixels>,
    focus_handle: Option<FocusHandle>,
) {
    pane_map_lock().lock().register(project_id, layout_path, bounds, focus_handle);
}

/// Remove a pane from the global map (call when a terminal pane is dropped)
pub fn deregister_pane_bounds(project_id: &str, layout_path: &[usize]) {
    pane_map_lock().lock().deregister(project_id, layout_path);
}

#[cfg(test)]
mod tests {
    use super::{PaneMap, NavigationDirection};
    use gpui::{px, Bounds, Point, Size};

    fn make_bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<gpui::Pixels> {
        Bounds {
            origin: Point { x: px(x), y: px(y) },
            size: Size { width: px(w), height: px(h) },
        }
    }

    #[test]
    fn sorted_by_reading_order_horizontal_row() {
        let mut map = PaneMap::new();
        map.register("c".into(), vec![0], make_bounds(600.0, 0.0, 300.0, 400.0), None);
        map.register("a".into(), vec![0], make_bounds(0.0, 0.0, 300.0, 400.0), None);
        map.register("b".into(), vec![0], make_bounds(300.0, 0.0, 300.0, 400.0), None);

        let sorted = map.sorted_by_reading_order();
        assert_eq!(sorted[0].project_id, "a");
        assert_eq!(sorted[1].project_id, "b");
        assert_eq!(sorted[2].project_id, "c");
    }

    #[test]
    fn sorted_by_reading_order_2x2_grid() {
        let mut map = PaneMap::new();
        map.register("d".into(), vec![0], make_bounds(400.0, 300.0, 400.0, 300.0), None);
        map.register("a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 300.0), None);
        map.register("c".into(), vec![0], make_bounds(0.0, 300.0, 400.0, 300.0), None);
        map.register("b".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 300.0), None);

        let sorted = map.sorted_by_reading_order();
        assert_eq!(sorted[0].project_id, "a");
        assert_eq!(sorted[1].project_id, "b");
        assert_eq!(sorted[2].project_id, "c");
        assert_eq!(sorted[3].project_id, "d");
    }

    #[test]
    fn sorted_by_reading_order_single_pane() {
        let mut map = PaneMap::new();
        map.register("only".into(), vec![0], make_bounds(0.0, 0.0, 800.0, 600.0), None);

        let sorted = map.sorted_by_reading_order();
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].project_id, "only");
    }

    #[test]
    fn register_upserts_existing_entry() {
        let mut map = PaneMap::new();

        map.register("p".into(), vec![0, 1], make_bounds(0.0, 0.0, 400.0, 300.0), None);
        assert_eq!(map.panes().len(), 1);

        map.register("p".into(), vec![0, 1], make_bounds(100.0, 0.0, 500.0, 300.0), None);
        assert_eq!(map.panes().len(), 1);
        assert_eq!(f32::from(map.panes()[0].bounds.origin.x), 100.0);
    }

    #[test]
    fn register_inserts_different_paths() {
        let mut map = PaneMap::new();
        let bounds = make_bounds(0.0, 0.0, 400.0, 300.0);

        map.register("p".into(), vec![0], bounds, None);
        map.register("p".into(), vec![1], bounds, None);
        assert_eq!(map.panes().len(), 2);
    }

    #[test]
    fn deregister_removes_matching_entry() {
        let mut map = PaneMap::new();
        map.register("a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 300.0), None);
        map.register("b".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 300.0), None);
        assert_eq!(map.panes().len(), 2);

        map.deregister("a", &[0]);
        assert_eq!(map.panes().len(), 1);
        assert_eq!(map.panes()[0].project_id, "b");
    }

    #[test]
    fn deregister_noop_when_not_found() {
        let mut map = PaneMap::new();
        map.register("a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 300.0), None);

        map.deregister("nonexistent", &[0]);
        assert_eq!(map.panes().len(), 1);
    }

    #[test]
    fn navigation_works_after_upsert() {
        let mut map = PaneMap::new();
        map.register("a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 600.0), None);
        map.register("b".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 600.0), None);

        // Upsert pane "a" with same bounds (simulates cached re-register)
        map.register("a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 600.0), None);
        assert_eq!(map.panes().len(), 2);

        let source = map.find_pane("a", &[0]).unwrap();
        let target = map.find_nearest_in_direction(source, NavigationDirection::Right);
        assert!(target.is_some());
        assert_eq!(target.unwrap().project_id, "b");
    }
}
