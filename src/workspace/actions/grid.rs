//! Grid layout workspace actions
//!
//! Actions for creating and modifying grid layouts.

use crate::workspace::state::{LayoutNode, Workspace};
use gpui::*;

impl Workspace {
    /// Replace the node at `path` with a Grid layout.
    /// The original terminal becomes the first cell; the rest are new terminals.
    pub fn create_grid(
        &mut self,
        project_id: &str,
        path: &[usize],
        rows: usize,
        cols: usize,
        cx: &mut Context<Self>,
    ) {
        let rows = rows.max(1);
        let cols = cols.max(1);

        self.with_layout_node(project_id, path, cx, |node| {
            let old_node = node.clone();
            let total = rows * cols;
            let mut children = Vec::with_capacity(total);
            children.push(old_node);
            for _ in 1..total {
                children.push(LayoutNode::new_terminal());
            }

            *node = LayoutNode::Grid {
                rows,
                cols,
                row_sizes: vec![100.0 / rows as f32; rows],
                col_sizes: vec![100.0 / cols as f32; cols],
                children,
            };
            true
        });
    }

    /// Append a row to a grid at `path` (after the last row).
    pub fn add_grid_row(
        &mut self,
        project_id: &str,
        path: &[usize],
        cx: &mut Context<Self>,
    ) {
        let rows = self.project(project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.get_at_path(path))
            .and_then(|node| if let LayoutNode::Grid { rows, .. } = node { Some(*rows) } else { None });
        if let Some(r) = rows {
            self.add_grid_row_at(project_id, path, r - 1, cx);
        }
    }

    /// Insert a new row after `after_row` in a grid at `path`.
    /// Adds `cols` new terminals and redistributes row_sizes evenly.
    pub fn add_grid_row_at(
        &mut self,
        project_id: &str,
        path: &[usize],
        after_row: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Grid { rows, cols, row_sizes, children, .. } = node {
                let c = *cols;
                let insert_start = (after_row + 1) * c;
                for i in 0..c {
                    children.insert(insert_start + i, LayoutNode::new_terminal());
                }
                *rows += 1;
                *row_sizes = vec![100.0 / *rows as f32; *rows];
                true
            } else {
                false
            }
        });
    }

    /// Append a column to a grid at `path` (after the last column).
    pub fn add_grid_column(
        &mut self,
        project_id: &str,
        path: &[usize],
        cx: &mut Context<Self>,
    ) {
        let cols = self.project(project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.get_at_path(path))
            .and_then(|node| if let LayoutNode::Grid { cols, .. } = node { Some(*cols) } else { None });
        if let Some(c) = cols {
            self.add_grid_column_at(project_id, path, c - 1, cx);
        }
    }

    /// Insert a new column after `after_col` in a grid at `path`.
    /// Inserts a new terminal in each row and redistributes col_sizes evenly.
    pub fn add_grid_column_at(
        &mut self,
        project_id: &str,
        path: &[usize],
        after_col: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Grid { rows, cols, col_sizes, children, .. } = node {
                let r = *rows;
                let old_cols = *cols;
                *cols = old_cols + 1;
                // Insert new terminal after `after_col` in each row (in reverse to keep indices valid)
                for row in (0..r).rev() {
                    let insert_pos = row * old_cols + after_col + 1;
                    children.insert(insert_pos, LayoutNode::new_terminal());
                }
                *col_sizes = vec![100.0 / *cols as f32; *cols];
                true
            } else {
                false
            }
        });
    }

    /// Remove the last row from a grid at `path`.
    /// Returns the terminal IDs that were in the removed row.
    /// If the grid becomes 1×1, collapses to single child.
    pub fn remove_grid_row(
        &mut self,
        project_id: &str,
        path: &[usize],
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let rows = self.project(project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.get_at_path(path))
            .and_then(|node| if let LayoutNode::Grid { rows, .. } = node { Some(*rows) } else { None });
        match rows {
            Some(r) if r > 1 => self.remove_grid_row_at(project_id, path, r - 1, cx),
            _ => Vec::new(),
        }
    }

    /// Remove a specific row by index from a grid at `path`.
    /// Returns the terminal IDs that were in the removed row.
    /// If the grid becomes 1×1, collapses to single child.
    pub fn remove_grid_row_at(
        &mut self,
        project_id: &str,
        path: &[usize],
        row_index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let mut removed_ids = Vec::new();

        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Grid { rows, cols, row_sizes, children, .. } = node {
                if *rows <= 1 || row_index >= *rows {
                    return false;
                }
                let c = *cols;
                // Collect terminal IDs from the target row
                let start = row_index * c;
                for child in &children[start..start + c] {
                    for id in child.collect_terminal_ids() {
                        removed_ids.push(id);
                    }
                }
                // Remove the row's children
                children.drain(start..start + c);
                *rows -= 1;
                *row_sizes = vec![100.0 / *rows as f32; *rows];

                // Collapse if 1×1
                if *rows == 1 && *cols == 1 {
                    let remaining = children.remove(0);
                    *node = remaining;
                }
                true
            } else {
                false
            }
        });

        removed_ids
    }

    /// Remove the last column from a grid at `path`.
    /// Returns the terminal IDs that were in the removed column.
    /// If the grid becomes 1×1, collapses to single child.
    pub fn remove_grid_column(
        &mut self,
        project_id: &str,
        path: &[usize],
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let cols = self.project(project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.get_at_path(path))
            .and_then(|node| if let LayoutNode::Grid { cols, .. } = node { Some(*cols) } else { None });
        match cols {
            Some(c) if c > 1 => self.remove_grid_column_at(project_id, path, c - 1, cx),
            _ => Vec::new(),
        }
    }

    /// Remove a specific column by index from a grid at `path`.
    /// Returns the terminal IDs that were in the removed column.
    /// If the grid becomes 1×1, collapses to single child.
    pub fn remove_grid_column_at(
        &mut self,
        project_id: &str,
        path: &[usize],
        col_index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let mut removed_ids = Vec::new();

        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Grid { rows, cols, col_sizes, children, .. } = node {
                if *cols <= 1 || col_index >= *cols {
                    return false;
                }
                let r = *rows;
                let old_cols = *cols;
                // Remove the target column from each row (in reverse to keep indices valid)
                for row in (0..r).rev() {
                    let remove_pos = row * old_cols + col_index;
                    let removed = children.remove(remove_pos);
                    for id in removed.collect_terminal_ids() {
                        removed_ids.push(id);
                    }
                }
                *cols = old_cols - 1;
                *col_sizes = vec![100.0 / *cols as f32; *cols];

                // Collapse if 1×1
                if *rows == 1 && *cols == 1 {
                    let remaining = children.remove(0);
                    *node = remaining;
                }
                true
            } else {
                false
            }
        });

        removed_ids
    }

    /// Update row sizes for a grid at `path`.
    pub fn update_grid_row_sizes(
        &mut self,
        project_id: &str,
        path: &[usize],
        new_sizes: Vec<f32>,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Grid { row_sizes, .. } = node {
                *row_sizes = new_sizes;
                true
            } else {
                false
            }
        });
    }

    /// Update column sizes for a grid at `path`.
    pub fn update_grid_col_sizes(
        &mut self,
        project_id: &str,
        path: &[usize],
        new_sizes: Vec<f32>,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Grid { col_sizes, .. } = node {
                *col_sizes = new_sizes;
                true
            } else {
                false
            }
        });
    }
}
