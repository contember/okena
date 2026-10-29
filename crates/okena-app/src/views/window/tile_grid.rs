//! Geometry of the grid project layout: projects flow into equal tiles, as many
//! per row as fit the viewport width, rows as many as the projects need. Shared
//! by the grid render and the scroll-to-focused math. GPUI-free.

/// Gap between two adjacent tiles, on both axes.
pub(super) const TILE_GAP: f32 = 1.0;

/// One tile's rectangle, relative to the top-left of the grid content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TileRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct TileGrid {
    pub cols: usize,
    pub rows: usize,
    /// One rect per project, in project order (row by row).
    pub tiles: Vec<TileRect>,
    /// Height of all rows together; taller than the viewport when it scrolls.
    pub content_height: f32,
}

impl TileGrid {
    /// Lay out `count` tiles in a `width` × `height` viewport.
    ///
    /// Columns are as many as fit `min_tile_width`, at least one, and never
    /// more than there are tiles, so a lone project still fills the window.
    /// Tiles share the width evenly and share the height evenly down to
    /// `min_tile_height`; below that the content outgrows the viewport and the
    /// grid scrolls vertically.
    pub fn compute(
        width: f32,
        height: f32,
        count: usize,
        min_tile_width: f32,
        min_tile_height: f32,
    ) -> Self {
        let width = width.max(0.0);
        let height = height.max(0.0);
        // A degenerate minimum (≤ 0 from a hand-edited settings.json) must
        // not divide by zero; one pixel keeps the arithmetic finite.
        let min_tile_width = min_tile_width.max(1.0);
        let min_tile_height = min_tile_height.max(1.0);

        let fit = ((width + TILE_GAP) / (min_tile_width + TILE_GAP)).floor() as usize;
        let cols = fit.clamp(1, count.max(1));
        let rows = count.div_ceil(cols);

        let tile_width = ((width - TILE_GAP * (cols - 1) as f32) / cols as f32).max(0.0);
        let shared_height = if rows == 0 {
            0.0
        } else {
            (height - TILE_GAP * (rows - 1) as f32) / rows as f32
        };
        let tile_height = shared_height.max(min_tile_height);

        let tiles = (0..count)
            .map(|i| TileRect {
                x: (i % cols) as f32 * (tile_width + TILE_GAP),
                y: (i / cols) as f32 * (tile_height + TILE_GAP),
                width: tile_width,
                height: tile_height,
            })
            .collect();
        let content_height = if rows == 0 {
            0.0
        } else {
            rows as f32 * tile_height + (rows - 1) as f32 * TILE_GAP
        };

        Self {
            cols,
            rows,
            tiles,
            content_height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portrait_monitor_fits_two_columns_and_scrolls_past_five_rows() {
        // 1440 × 2560 portrait, 700 × 400 minimum: two tiles side by side,
        // five rows fit (2560 / 5 ≈ 512 ≥ 400); a sixth row has to scroll.
        let fits = TileGrid::compute(1440.0, 2560.0, 10, 700.0, 400.0);
        assert_eq!((fits.cols, fits.rows), (2, 5));
        assert!(fits.content_height <= 2560.0 + 0.01);
        assert!(fits.tiles.iter().all(|t| t.height >= 400.0));

        let scrolls = TileGrid::compute(1440.0, 2560.0, 14, 700.0, 400.0);
        assert_eq!((scrolls.cols, scrolls.rows), (2, 7));
        assert_eq!(scrolls.tiles[0].height, 400.0);
        assert_eq!(scrolls.content_height, 7.0 * 400.0 + 6.0 * TILE_GAP);
    }

    #[test]
    fn landscape_monitor_flows_tiles_row_by_row() {
        let grid = TileGrid::compute(2560.0, 1440.0, 5, 700.0, 300.0);
        assert_eq!((grid.cols, grid.rows), (3, 2));
        let w = (2560.0 - 2.0 * TILE_GAP) / 3.0;
        let h = (1440.0 - TILE_GAP) / 2.0;
        // Fourth tile wraps to the first column of the second row.
        assert_eq!(
            grid.tiles[3],
            TileRect {
                x: 0.0,
                y: h + TILE_GAP,
                width: w,
                height: h
            }
        );
        assert_eq!(grid.tiles[2].x, 2.0 * (w + TILE_GAP));
        // The last row keeps the column width instead of stretching.
        assert_eq!(grid.tiles[4].width, w);
    }

    #[test]
    fn columns_never_outnumber_projects() {
        let one = TileGrid::compute(3000.0, 1000.0, 1, 500.0, 300.0);
        assert_eq!((one.cols, one.rows), (1, 1));
        assert_eq!(one.tiles[0].width, 3000.0);
        assert_eq!(one.tiles[0].height, 1000.0);

        let two = TileGrid::compute(3000.0, 1000.0, 2, 500.0, 300.0);
        assert_eq!(two.cols, 2);
    }

    #[test]
    fn narrow_viewport_still_gets_one_column() {
        let grid = TileGrid::compute(300.0, 900.0, 3, 700.0, 200.0);
        assert_eq!((grid.cols, grid.rows), (1, 3));
        assert_eq!(grid.tiles[0].width, 300.0);
    }

    #[test]
    fn exact_fit_counts_the_gaps() {
        // Two 700px tiles plus one gap fit exactly; one pixel less does not.
        assert_eq!(TileGrid::compute(1401.0, 800.0, 4, 700.0, 100.0).cols, 2);
        assert_eq!(TileGrid::compute(1400.0, 800.0, 4, 700.0, 100.0).cols, 1);
    }

    #[test]
    fn empty_and_unmeasured_viewports_are_finite() {
        let empty = TileGrid::compute(1000.0, 800.0, 0, 600.0, 300.0);
        assert_eq!((empty.rows, empty.tiles.len()), (0, 0));
        assert_eq!(empty.content_height, 0.0);

        // Before the first layout pass the viewport measures 0 × 0.
        let unmeasured = TileGrid::compute(0.0, 0.0, 3, 600.0, 300.0);
        assert_eq!(unmeasured.cols, 1);
        assert!(unmeasured.tiles.iter().all(|t| t.height == 300.0));

        let zero_min = TileGrid::compute(1000.0, 800.0, 3, 0.0, 0.0);
        assert_eq!(zero_min.cols, 3);
        assert!(zero_min.content_height.is_finite());
    }
}
