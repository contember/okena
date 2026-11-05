//! Order and chrome of the projects grid: the flat grid, or the status lanes.
//! Shared by the grid render and the scroll-to-focused math so both agree on
//! where every column sits. GPUI-free.

use okena_core::types::BoardColumn;
use okena_workspace::status_board::Lane;
use std::ops::Range;

/// Thickness of the divider between two adjacent columns of one span.
pub(super) const DIVIDER_SIZE: f32 = 1.0;
/// Width of the spine down the left of every lane; along the grid axis only
/// in a columns grid (in a rows grid it sits across the axis).
pub(super) const LANE_SPINE_WIDTH: f32 = 20.0;
/// Line between two lanes of a rows grid, where no spine separates them
/// along the axis.
pub(super) const LANE_SEPARATOR: f32 = 1.0;

/// A run of adjacent columns in [`GridLayout::order`] with dividers between
/// them. `caption` is `Some` for a lane (`Some(None)` = "No status").
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GridSpan {
    pub caption: Option<Option<BoardColumn>>,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GridLayout {
    /// Visible project ids in render order (lane by lane when lanes are on).
    pub order: Vec<String>,
    pub spans: Vec<GridSpan>,
}

impl GridLayout {
    /// The grid as it was before lanes: one caption-less span.
    pub fn flat(visible: Vec<String>) -> Self {
        let range = 0..visible.len();
        Self {
            order: visible,
            spans: vec![GridSpan {
                caption: None,
                range,
            }],
        }
    }

    /// `visible` regrouped lane by lane; `lanes` index into `visible`.
    pub fn with_lanes(visible: &[String], lanes: Vec<Lane>) -> Self {
        let mut order = Vec::with_capacity(visible.len());
        let mut spans = Vec::with_capacity(lanes.len());
        for lane in lanes {
            let start = order.len();
            order.extend(lane.indices.iter().map(|&i| visible[i].clone()));
            spans.push(GridSpan {
                caption: Some(lane.column),
                range: start..order.len(),
            });
        }
        Self { order, spans }
    }

    /// Everything along the grid axis that is not a column: dividers inside
    /// spans plus, per lane, what leads it (see [`Self::lane_lead`]).
    pub fn chrome(&self, is_rows: bool) -> f32 {
        self.spans
            .iter()
            .enumerate()
            .map(|(index, span)| {
                let dividers = span.range.len().saturating_sub(1) as f32 * DIVIDER_SIZE;
                dividers + Self::lane_lead(index, span, is_rows)
            })
            .sum()
    }

    /// The leading edge of every column in `order` along the grid axis, given
    /// each column's rendered size.
    pub fn leading_edges(&self, sizes: &[f32], is_rows: bool) -> Vec<f32> {
        let mut edges = vec![0.0; self.order.len()];
        let mut lead = 0.0;
        for (index, span) in self.spans.iter().enumerate() {
            lead += Self::lane_lead(index, span, is_rows);
            for i in span.range.clone() {
                edges[i] = lead;
                lead += sizes.get(i).copied().unwrap_or(0.0);
                if i + 1 < span.range.end {
                    lead += DIVIDER_SIZE;
                }
            }
        }
        edges
    }

    /// What stands before a lane along the grid axis: its spine in a columns
    /// grid, a separator from the lane above in a rows grid (none before the
    /// first). Nothing for the caption-less flat span.
    pub fn lane_lead(index: usize, span: &GridSpan, is_rows: bool) -> f32 {
        match (&span.caption, is_rows) {
            (None, _) => 0.0,
            (Some(_), false) => LANE_SPINE_WIDTH,
            (Some(_), true) if index == 0 => 0.0,
            (Some(_), true) => LANE_SEPARATOR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// Visible `a b c d`; lanes put `c` first, then `a d`, then `b` unassigned.
    fn three_lanes() -> GridLayout {
        let review = BoardColumn::new("review", "Review");
        let done = BoardColumn::new("done", "Done");
        GridLayout::with_lanes(
            &ids(&["a", "b", "c", "d"]),
            vec![
                Lane {
                    column: Some(review),
                    indices: vec![2],
                },
                Lane {
                    column: Some(done),
                    indices: vec![0, 3],
                },
                Lane {
                    column: None,
                    indices: vec![1],
                },
            ],
        )
    }

    #[test]
    fn flat_grid_has_a_divider_between_every_pair() {
        let layout = GridLayout::flat(ids(&["a", "b", "c"]));
        assert_eq!(layout.chrome(false), 2.0);
        assert_eq!(layout.chrome(true), 2.0);
        assert_eq!(
            layout.leading_edges(&[100.0, 200.0, 300.0], false),
            vec![0.0, 101.0, 302.0]
        );
    }

    #[test]
    fn lanes_reorder_and_keep_dividers_inside_a_lane_only() {
        let layout = three_lanes();
        assert_eq!(layout.order, ids(&["c", "a", "d", "b"]));
        let ranges: Vec<Range<usize>> = layout.spans.iter().map(|s| s.range.clone()).collect();
        assert_eq!(ranges, vec![0..1, 1..3, 3..4]);
        // One divider (inside the Done lane) plus three spines in columns,
        // two separators between three lanes in rows.
        assert_eq!(layout.chrome(false), 1.0 + 3.0 * LANE_SPINE_WIDTH);
        assert_eq!(layout.chrome(true), 1.0 + 2.0 * LANE_SEPARATOR);
    }

    #[test]
    fn lane_leading_edges_count_spines_in_columns_and_separators_in_rows() {
        let layout = three_lanes();
        let sizes = [100.0, 100.0, 100.0, 100.0];
        assert_eq!(
            layout.leading_edges(&sizes, false),
            vec![20.0, 140.0, 241.0, 361.0]
        );
        assert_eq!(
            layout.leading_edges(&sizes, true),
            vec![0.0, 101.0, 202.0, 303.0]
        );
    }

    #[test]
    fn sizes_plus_chrome_fill_the_budget_exactly() {
        let layout = three_lanes();
        let sizes = [120.0, 80.0, 50.0, 70.0];
        for is_rows in [false, true] {
            let edges = layout.leading_edges(&sizes, is_rows);
            let last = edges.len() - 1;
            let total = edges[last] + sizes[last];
            assert_eq!(total, sizes.iter().sum::<f32>() + layout.chrome(is_rows));
        }
    }
}
