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
#[derive(Clone, Debug)]
pub struct PaneBounds {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub bounds: Bounds<Pixels>,
}

impl PaneBounds {
    /// Get the center point of this pane
    pub fn center(&self) -> Point<Pixels> {
        Point {
            x: self.bounds.origin.x + self.bounds.size.width / 2.0,
            y: self.bounds.origin.y + self.bounds.size.height / 2.0,
        }
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

    /// Register a pane's bounds
    pub fn register(&mut self, project_id: String, layout_path: Vec<usize>, bounds: Bounds<Pixels>) {
        // Skip invalid bounds (zero or negative size)
        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
            return;
        }

        self.panes.push(PaneBounds {
            project_id,
            layout_path,
            bounds,
        });
    }

    /// Clear all registered panes
    pub fn clear(&mut self) {
        self.panes.clear();
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
        let source_center = source.center();

        // Filter panes that are in the correct direction
        let candidates: Vec<_> = self.panes.iter()
            .filter(|p| {
                // Don't consider the source pane itself
                if p.project_id == source.project_id && p.layout_path == source.layout_path {
                    return false;
                }

                let candidate_center = p.center();

                match direction {
                    NavigationDirection::Left => {
                        // Candidate must be to the left (its right edge before source's left edge,
                        // or center is to the left)
                        candidate_center.x < source_center.x
                    }
                    NavigationDirection::Right => {
                        // Candidate must be to the right
                        candidate_center.x > source_center.x
                    }
                    NavigationDirection::Up => {
                        // Candidate must be above
                        candidate_center.y < source_center.y
                    }
                    NavigationDirection::Down => {
                        // Candidate must be below
                        candidate_center.y > source_center.y
                    }
                }
            })
            .collect();

        if candidates.is_empty() {
            return None;
        }

        // Find the nearest candidate using weighted distance
        // Weight the primary axis more heavily to prefer panes that are directly
        // in the navigation direction rather than diagonally
        candidates.into_iter().min_by(|a, b| {
            let dist_a = weighted_distance(&source_center, &a.center(), direction);
            let dist_b = weighted_distance(&source_center, &b.center(), direction);
            dist_a.partial_cmp(&dist_b).unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Get all registered panes (for debugging)
    #[allow(dead_code)]
    pub fn panes(&self) -> &[PaneBounds] {
        &self.panes
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

    // For horizontal navigation, weight vertical distance more heavily (penalty for being off-axis)
    // For vertical navigation, weight horizontal distance more heavily
    let (primary_weight, secondary_weight) = match direction {
        NavigationDirection::Left | NavigationDirection::Right => (1.0, 2.0),
        NavigationDirection::Up | NavigationDirection::Down => (2.0, 1.0),
    };

    let weighted_dx = dx * primary_weight;
    let weighted_dy = dy * secondary_weight;

    // Use squared distance to avoid sqrt (we only need relative comparison)
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

/// Clear the global pane map
pub fn clear_pane_map() {
    pane_map_lock().lock().clear();
}

/// Register a pane's bounds in the global map
pub fn register_pane_bounds(
    project_id: String,
    layout_path: Vec<usize>,
    bounds: Bounds<Pixels>,
) {
    pane_map_lock().lock().register(project_id, layout_path, bounds);
}

// Tests removed due to GPUI recursion limit issues with #[test] macro
// Navigation functionality should be tested manually
