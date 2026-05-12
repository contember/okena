//! Spatial navigation for terminal panes
//!
//! This module provides arrow key navigation between terminal panes using
//! a spatial map of pane bounds. Navigation finds the nearest pane in the
//! requested direction using center-point distance calculation.

use gpui::*;
use okena_workspace::state::WindowId;

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
    pub window_id: WindowId,
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub bounds: Bounds<Pixels>,
    /// Enables direct focus transfer, bypassing multi-frame delay from nested cached views.
    pub focus_handle: Option<FocusHandle>,
}

impl std::fmt::Debug for PaneBounds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneBounds")
            .field("window_id", &self.window_id)
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
    pub fn register(&mut self, window_id: WindowId, project_id: String, layout_path: Vec<usize>, bounds: Bounds<Pixels>, focus_handle: Option<FocusHandle>) {
        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
            return;
        }

        if let Some(existing) = self.panes.iter_mut().find(|p| {
            p.window_id == window_id && p.project_id == project_id && p.layout_path == layout_path
        }) {
            existing.bounds = bounds;
            if focus_handle.is_some() {
                existing.focus_handle = focus_handle;
            }
        } else {
            self.panes.push(PaneBounds {
                window_id,
                project_id,
                layout_path,
                bounds,
                focus_handle,
            });
        }
    }

    /// Remove a pane from the map (e.g. when the terminal pane is dropped).
    pub fn deregister(&mut self, window_id: WindowId, project_id: &str, layout_path: &[usize]) {
        self.panes.retain(|p| {
            !(p.window_id == window_id && p.project_id == project_id && p.layout_path == layout_path)
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
                if p.window_id == source.window_id && p.project_id == source.project_id && p.layout_path == source.layout_path {
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

    /// Find the next pane in reading order (top-to-bottom, left-to-right, cycles)
    pub fn find_next_pane(&self, source: &PaneBounds) -> Option<PaneBounds> {
        let sorted = self.sorted_by_reading_order();
        if sorted.len() <= 1 {
            return None;
        }

        let current_idx = sorted.iter().position(|p| {
            p.project_id == source.project_id && p.layout_path == source.layout_path
        })?;

        let next_idx = (current_idx + 1) % sorted.len();
        Some(sorted[next_idx].clone())
    }

    /// Remove panes whose project_id is not in the given set.
    /// Called during render to evict stale entries from hidden projects
    /// (e.g. worktree columns that are retained but not currently visible).
    pub fn retain_projects(&mut self, visible_ids: &std::collections::HashSet<&str>) {
        self.panes.retain(|p| visible_ids.contains(p.project_id.as_str()));
    }

    /// Retain visible projects for one window while leaving other windows' panes untouched.
    pub fn retain_window_projects(&mut self, window_id: WindowId, visible_ids: &std::collections::HashSet<&str>) {
        self.panes.retain(|p| p.window_id != window_id || visible_ids.contains(p.project_id.as_str()));
    }

    /// Get all registered panes
    pub fn panes(&self) -> &[PaneBounds] {
        &self.panes
    }

    /// Return a copy containing only panes for one window.
    pub fn for_window(&self, window_id: WindowId) -> Self {
        Self {
            panes: self.panes.iter().filter(|p| p.window_id == window_id).cloned().collect(),
        }
    }

    /// Return panes sorted by reading order: column-first (left-to-right by project),
    /// then top-to-bottom within each column.
    ///
    /// Panes are grouped by `project_id` (each project is a visual column).
    /// Groups are ordered by the leftmost X edge of any pane in the group.
    /// Within each group, panes are sorted by center Y then center X.
    pub fn sorted_by_reading_order(&self) -> Vec<&PaneBounds> {
        use std::collections::HashMap;

        // Group panes by project_id
        let mut groups: HashMap<&str, Vec<&PaneBounds>> = HashMap::new();
        for pane in &self.panes {
            groups.entry(&pane.project_id).or_default().push(pane);
        }

        // Sort each group internally by center Y then center X
        for group in groups.values_mut() {
            group.sort_by(|a, b| {
                let ay = f32::from(a.bounds.center().y);
                let by = f32::from(b.bounds.center().y);
                let ax = f32::from(a.bounds.center().x);
                let bx = f32::from(b.bounds.center().x);
                ay.partial_cmp(&by)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal))
            });
        }

        // Sort groups by minimum origin.x, then project_id as tiebreaker
        let mut group_entries: Vec<(&str, Vec<&PaneBounds>)> = groups.into_iter().collect();
        group_entries.sort_by(|(id_a, panes_a), (id_b, panes_b)| {
            let min_x_a = panes_a.iter().map(|p| f32::from(p.bounds.origin.x)).fold(f32::INFINITY, f32::min);
            let min_x_b = panes_b.iter().map(|p| f32::from(p.bounds.origin.x)).fold(f32::INFINITY, f32::min);
            min_x_a.partial_cmp(&min_x_b)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| id_a.cmp(id_b))
        });

        // Flatten
        group_entries.into_iter().flat_map(|(_, panes)| panes).collect()
    }

    /// Find the previous pane in reading order (top-to-bottom, left-to-right, cycles)
    pub fn find_prev_pane(&self, source: &PaneBounds) -> Option<PaneBounds> {
        let sorted = self.sorted_by_reading_order();
        if sorted.len() <= 1 {
            return None;
        }

        let current_idx = sorted.iter().position(|p| {
            p.project_id == source.project_id && p.layout_path == source.layout_path
        })?;

        let prev_idx = if current_idx == 0 {
            sorted.len() - 1
        } else {
            current_idx - 1
        };
        Some(sorted[prev_idx].clone())
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

/// Get the pane map for one window.
pub fn get_pane_map(window_id: WindowId) -> PaneMap {
    pane_map_lock().lock().for_window(window_id)
}

/// Register a pane's bounds in the global map
pub fn register_pane_bounds(
    window_id: WindowId,
    project_id: String,
    layout_path: Vec<usize>,
    bounds: Bounds<Pixels>,
    focus_handle: Option<FocusHandle>,
) {
    pane_map_lock().lock().register(window_id, project_id, layout_path, bounds, focus_handle);
}

/// Remove a pane from the global map (call when a terminal pane is dropped)
pub fn deregister_pane_bounds(window_id: WindowId, project_id: &str, layout_path: &[usize]) {
    pane_map_lock().lock().deregister(window_id, project_id, layout_path);
}

/// Remove pane entries for projects not in the visible set.
/// Prevents stale entries from hidden columns (e.g. worktree projects with
/// show_in_overview=false) from blocking spatial navigation.
pub fn prune_pane_map(window_id: WindowId, visible_project_ids: &std::collections::HashSet<&str>) {
    pane_map_lock().lock().retain_window_projects(window_id, visible_project_ids);
}

#[cfg(test)]
mod tests {
    use super::{PaneMap, NavigationDirection};
    use gpui::{px, Bounds, Point, Size};
    use okena_workspace::state::WindowId;

    fn make_bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<gpui::Pixels> {
        Bounds {
            origin: Point { x: px(x), y: px(y) },
            size: Size { width: px(w), height: px(h) },
        }
    }

    #[test]
    fn sorted_by_reading_order_horizontal_row() {
        let mut map = PaneMap::new();
        map.register(WindowId::Main, "c".into(), vec![0], make_bounds(600.0, 0.0, 300.0, 400.0), None);
        map.register(WindowId::Main, "a".into(), vec![0], make_bounds(0.0, 0.0, 300.0, 400.0), None);
        map.register(WindowId::Main, "b".into(), vec![0], make_bounds(300.0, 0.0, 300.0, 400.0), None);

        let sorted = map.sorted_by_reading_order();
        assert_eq!(sorted[0].project_id, "a");
        assert_eq!(sorted[1].project_id, "b");
        assert_eq!(sorted[2].project_id, "c");
    }

    #[test]
    fn sorted_by_reading_order_2x2_grid() {
        let mut map = PaneMap::new();
        // Left column (project "left") — two stacked panes
        map.register(WindowId::Main, "left".into(), vec![1], make_bounds(0.0, 300.0, 400.0, 300.0), None);
        map.register(WindowId::Main, "left".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 300.0), None);
        // Right column (project "right") — two stacked panes
        map.register(WindowId::Main, "right".into(), vec![1], make_bounds(400.0, 300.0, 400.0, 300.0), None);
        map.register(WindowId::Main, "right".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 300.0), None);

        let sorted = map.sorted_by_reading_order();
        // Left column first (top then bottom), then right column
        assert_eq!(sorted[0].project_id, "left");
        assert_eq!(sorted[0].layout_path, vec![0]);
        assert_eq!(sorted[1].project_id, "left");
        assert_eq!(sorted[1].layout_path, vec![1]);
        assert_eq!(sorted[2].project_id, "right");
        assert_eq!(sorted[2].layout_path, vec![0]);
        assert_eq!(sorted[3].project_id, "right");
        assert_eq!(sorted[3].layout_path, vec![1]);
    }

    #[test]
    fn sorted_by_reading_order_multi_column_different_heights() {
        let mut map = PaneMap::new();
        // Column A (left): one full-height pane, center Y=300
        map.register(WindowId::Main, "col_a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 600.0), None);
        // Column B (right): two stacked panes, center Y=150 and Y=450
        map.register(WindowId::Main, "col_b".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 300.0), None);
        map.register(WindowId::Main, "col_b".into(), vec![1], make_bounds(400.0, 300.0, 400.0, 300.0), None);

        let sorted = map.sorted_by_reading_order();
        // Column A first (leftmost), then column B top-to-bottom
        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0].project_id, "col_a");
        assert_eq!(sorted[1].project_id, "col_b");
        assert_eq!(sorted[1].layout_path, vec![0]);
        assert_eq!(sorted[2].project_id, "col_b");
        assert_eq!(sorted[2].layout_path, vec![1]);
    }

    #[test]
    fn sorted_by_reading_order_single_pane() {
        let mut map = PaneMap::new();
        map.register(WindowId::Main, "only".into(), vec![0], make_bounds(0.0, 0.0, 800.0, 600.0), None);

        let sorted = map.sorted_by_reading_order();
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].project_id, "only");
    }

    #[test]
    fn register_upserts_existing_entry() {
        let mut map = PaneMap::new();

        map.register(WindowId::Main, "p".into(), vec![0, 1], make_bounds(0.0, 0.0, 400.0, 300.0), None);
        assert_eq!(map.panes().len(), 1);

        map.register(WindowId::Main, "p".into(), vec![0, 1], make_bounds(100.0, 0.0, 500.0, 300.0), None);
        assert_eq!(map.panes().len(), 1);
        assert_eq!(f32::from(map.panes()[0].bounds.origin.x), 100.0);
    }

    #[test]
    fn register_keeps_same_pane_key_separate_per_window() {
        let mut map = PaneMap::new();
        let extra = WindowId::Extra(okena_workspace::state::WindowState::default().id);

        map.register(WindowId::Main, "p".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 300.0), None);
        map.register(extra, "p".into(), vec![0], make_bounds(500.0, 0.0, 400.0, 300.0), None);

        assert_eq!(map.panes().len(), 2);
        assert_eq!(map.for_window(WindowId::Main).panes().len(), 1);
        assert_eq!(map.for_window(extra).panes().len(), 1);
    }

    #[test]
    fn register_inserts_different_paths() {
        let mut map = PaneMap::new();
        let bounds = make_bounds(0.0, 0.0, 400.0, 300.0);

        map.register(WindowId::Main, "p".into(), vec![0], bounds, None);
        map.register(WindowId::Main, "p".into(), vec![1], bounds, None);
        assert_eq!(map.panes().len(), 2);
    }

    #[test]
    fn deregister_removes_matching_entry() {
        let mut map = PaneMap::new();
        map.register(WindowId::Main, "a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 300.0), None);
        map.register(WindowId::Main, "b".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 300.0), None);
        assert_eq!(map.panes().len(), 2);

        map.deregister(WindowId::Main, "a", &[0]);
        assert_eq!(map.panes().len(), 1);
        assert_eq!(map.panes()[0].project_id, "b");
    }

    #[test]
    fn deregister_noop_when_not_found() {
        let mut map = PaneMap::new();
        map.register(WindowId::Main, "a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 300.0), None);

        map.deregister(WindowId::Main, "nonexistent", &[0]);
        assert_eq!(map.panes().len(), 1);
    }

    #[test]
    fn retain_projects_removes_hidden() {
        let mut map = PaneMap::new();
        map.register(WindowId::Main, "parent".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 600.0), None);
        map.register(WindowId::Main, "worktree".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 600.0), None);
        map.register(WindowId::Main, "other".into(), vec![0], make_bounds(800.0, 0.0, 400.0, 600.0), None);
        assert_eq!(map.panes().len(), 3);

        // Only parent and other are visible (worktree hidden in overview)
        let visible: std::collections::HashSet<&str> = ["parent", "other"].into_iter().collect();
        map.retain_projects(&visible);
        assert_eq!(map.panes().len(), 2);
        assert!(map.find_pane("parent", &[0]).is_some());
        assert!(map.find_pane("worktree", &[0]).is_none());
        assert!(map.find_pane("other", &[0]).is_some());
    }

    #[test]
    fn retain_projects_allows_navigation_past_hidden() {
        let mut map = PaneMap::new();
        map.register(WindowId::Main, "a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 600.0), None);
        map.register(WindowId::Main, "hidden_wt".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 600.0), None);
        map.register(WindowId::Main, "b".into(), vec![0], make_bounds(800.0, 0.0, 400.0, 600.0), None);

        // Prune hidden worktree
        let visible: std::collections::HashSet<&str> = ["a", "b"].into_iter().collect();
        map.retain_projects(&visible);

        // Navigation from a should reach b (not get stuck on hidden_wt)
        let source = map.find_pane("a", &[0]).unwrap();
        let target = map.find_nearest_in_direction(source, NavigationDirection::Right);
        assert!(target.is_some());
        assert_eq!(target.unwrap().project_id, "b");
    }

    #[test]
    fn retain_window_projects_does_not_prune_other_windows() {
        let mut map = PaneMap::new();
        let extra = WindowId::Extra(okena_workspace::state::WindowState::default().id);
        map.register(WindowId::Main, "visible".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 600.0), None);
        map.register(WindowId::Main, "hidden".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 600.0), None);
        map.register(extra, "hidden".into(), vec![0], make_bounds(800.0, 0.0, 400.0, 600.0), None);

        let visible: std::collections::HashSet<&str> = ["visible"].into_iter().collect();
        map.retain_window_projects(WindowId::Main, &visible);

        assert!(map.for_window(WindowId::Main).find_pane("hidden", &[0]).is_none());
        assert!(map.for_window(extra).find_pane("hidden", &[0]).is_some());
    }

    #[test]
    fn navigation_works_after_upsert() {
        let mut map = PaneMap::new();
        map.register(WindowId::Main, "a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 600.0), None);
        map.register(WindowId::Main, "b".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 600.0), None);

        // Upsert pane "a" with same bounds (simulates cached re-register)
        map.register(WindowId::Main, "a".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 600.0), None);
        assert_eq!(map.panes().len(), 2);

        let source = map.find_pane("a", &[0]).unwrap();
        let target = map.find_nearest_in_direction(source, NavigationDirection::Right);
        assert!(target.is_some());
        assert_eq!(target.unwrap().project_id, "b");
    }

    #[test]
    fn sequential_cycling_uses_reading_order_not_insertion_order() {
        let mut map = PaneMap::new();
        // Register in non-visual order to prove insertion order is ignored
        // Left column: one full-height pane; Right column: two stacked panes
        map.register(WindowId::Main, "right".into(), vec![1], make_bounds(400.0, 300.0, 400.0, 300.0), None);
        map.register(WindowId::Main, "left".into(), vec![0], make_bounds(0.0, 0.0, 400.0, 600.0), None);
        map.register(WindowId::Main, "right".into(), vec![0], make_bounds(400.0, 0.0, 400.0, 300.0), None);

        // From left (first in reading order), next should be right[0] (top of right column)
        let source_left = map.find_pane("left", &[0]).unwrap().clone();
        let next = map.find_next_pane(&source_left).unwrap();
        assert_eq!(next.project_id, "right");
        assert_eq!(next.layout_path, vec![0]);

        // From right[0], next should be right[1]
        let source_rt = map.find_pane("right", &[0]).unwrap().clone();
        let next = map.find_next_pane(&source_rt).unwrap();
        assert_eq!(next.project_id, "right");
        assert_eq!(next.layout_path, vec![1]);

        // From right[1] (last), wraps to left
        let source_rb = map.find_pane("right", &[1]).unwrap().clone();
        let next = map.find_next_pane(&source_rb).unwrap();
        assert_eq!(next.project_id, "left");

        // Prev from left wraps to right[1]
        let prev = map.find_prev_pane(&source_left).unwrap();
        assert_eq!(prev.project_id, "right");
        assert_eq!(prev.layout_path, vec![1]);
    }
}
