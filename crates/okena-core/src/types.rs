use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

impl SplitDirection {
    /// Return the perpendicular direction (Horizontal <-> Vertical).
    pub fn flipped(self) -> Self {
        match self {
            SplitDirection::Horizontal => SplitDirection::Vertical,
            SplitDirection::Vertical => SplitDirection::Horizontal,
        }
    }
}

/// How a window arranges its projects.
///
/// `Columns` (default) lays projects out side by side, each with a width;
/// `Rows` stacks them vertically, each with a height; `Grid` flows them into
/// equal tiles, as many per row as fit the window's width. Stored per window,
/// so each window picks its own mode. The persisted `project_widths` weights
/// are axis-agnostic, so they survive every switch (grid ignores them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectLayoutMode {
    /// Projects laid out side by side (horizontal, width-resized). Default.
    #[default]
    Columns,
    /// Projects stacked vertically (height-resized).
    Rows,
    /// Projects flowed into a scrolling grid of equal tiles.
    Grid,
}

impl ProjectLayoutMode {
    /// The next mode in the toggle cycle: columns → rows → grid → columns.
    pub fn toggled(self) -> Self {
        match self {
            ProjectLayoutMode::Columns => ProjectLayoutMode::Rows,
            ProjectLayoutMode::Rows => ProjectLayoutMode::Grid,
            ProjectLayoutMode::Grid => ProjectLayoutMode::Columns,
        }
    }

    /// True when projects are stacked vertically.
    pub fn is_rows(self) -> bool {
        matches!(self, ProjectLayoutMode::Rows)
    }

    /// True when projects are flowed into tiles.
    pub fn is_grid(self) -> bool {
        matches!(self, ProjectLayoutMode::Grid)
    }

    /// Transform the daemon's canonical split direction for this window.
    pub fn presented_split_direction(self, direction: SplitDirection) -> SplitDirection {
        if self.is_rows() {
            direction.flipped()
        } else {
            direction
        }
    }
}

/// Display mode for the diff viewer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffViewMode {
    #[default]
    Unified,
    SideBySide,
}

impl DiffViewMode {
    /// Toggle between view modes.
    pub fn toggle(self) -> Self {
        match self {
            DiffViewMode::Unified => DiffViewMode::SideBySide,
            DiffViewMode::SideBySide => DiffViewMode::Unified,
        }
    }
}

/// How much text the status bar spells out.
///
/// `Detailed` (default) is the labelled bar: `CPU 13%`, `Claude Code OK`,
/// `5h 20%`. `Minimal` keeps the same widgets but drops the redundant numbers
/// and "OK" labels, leaving the graphs and bars to carry the value — the exact
/// figures move into tooltips. Anything abnormal (a degraded service) still
/// prints its label in both modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusBarStyle {
    #[default]
    Detailed,
    Minimal,
}

impl StatusBarStyle {
    pub fn display_name(self) -> &'static str {
        match self {
            StatusBarStyle::Detailed => "Detailed",
            StatusBarStyle::Minimal => "Minimal",
        }
    }

    pub fn all_variants() -> &'static [StatusBarStyle] {
        &[StatusBarStyle::Detailed, StatusBarStyle::Minimal]
    }

    pub fn is_minimal(self) -> bool {
        matches!(self, StatusBarStyle::Minimal)
    }
}

/// What the small ring before a worktree row in the sidebar shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorktreeMarker {
    /// The project's colour (inherited from the parent unless overridden).
    #[default]
    Project,
    /// The colour of the worktree's status column; the project colour when
    /// it has none.
    Status,
}

impl WorktreeMarker {
    pub fn display_name(self) -> &'static str {
        match self {
            WorktreeMarker::Project => "Project colour",
            WorktreeMarker::Status => "Status colour",
        }
    }

    pub fn all_variants() -> &'static [WorktreeMarker] {
        &[WorktreeMarker::Project, WorktreeMarker::Status]
    }
}

/// One column of the status board. `id` is the stable key that
/// `ProjectData::status` stores; `name` is what the user sees and
/// may rename without touching any project's status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardColumn {
    pub id: String,
    pub name: String,
    /// Accent for the column's caption and header; `Default` means none.
    #[serde(default)]
    pub color: crate::theme::FolderColor,
}

impl BoardColumn {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            color: crate::theme::FolderColor::Default,
        }
    }

    pub fn with_color(mut self, color: crate::theme::FolderColor) -> Self {
        self.color = color;
        self
    }
}

/// The columns a fresh install starts with.
pub fn default_board_columns() -> Vec<BoardColumn> {
    vec![
        BoardColumn::new("todo", "To do"),
        BoardColumn::new("in-progress", "In progress"),
        BoardColumn::new("review", "Review"),
        BoardColumn::new("done", "Done"),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffMode {
    #[default]
    WorkingTree,
    Staged,
    /// Show the diff introduced by a specific commit (hash or ref).
    Commit(String),
    /// Compare two branches (shows diff between base and head).
    BranchCompare {
        base: String,
        head: String,
    },
}

impl DiffMode {
    /// Get the display name for this mode.
    pub fn display_name(&self) -> String {
        match self {
            DiffMode::WorkingTree => "Unstaged".to_string(),
            DiffMode::Staged => "Staged".to_string(),
            DiffMode::Commit(hash) => {
                let short = if hash.len() > 7 { &hash[..7] } else { hash };
                format!("Commit {short}")
            }
            DiffMode::BranchCompare { base, head } => {
                format!("{base}...{head}")
            }
        }
    }

    /// Toggle to the other mode. Commit/BranchCompare toggle back to WorkingTree.
    pub fn toggle(&self) -> Self {
        match self {
            DiffMode::WorkingTree => DiffMode::Staged,
            DiffMode::Staged => DiffMode::WorkingTree,
            DiffMode::Commit(_) | DiffMode::BranchCompare { .. } => DiffMode::WorkingTree,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_rows_present_canonical_splits_on_the_opposite_axis() {
        use ProjectLayoutMode::*;
        use SplitDirection::*;
        assert_eq!(Columns.presented_split_direction(Horizontal), Horizontal);
        assert_eq!(Rows.presented_split_direction(Horizontal), Vertical);
        assert_eq!(Rows.presented_split_direction(Vertical), Horizontal);
        assert_eq!(Grid.presented_split_direction(Horizontal), Horizontal);
    }

    #[test]
    fn layout_toggle_cycles_through_every_mode() {
        let start = ProjectLayoutMode::Columns;
        assert_eq!(start.toggled(), ProjectLayoutMode::Rows);
        assert_eq!(start.toggled().toggled(), ProjectLayoutMode::Grid);
        assert_eq!(start.toggled().toggled().toggled(), start);
    }

    #[test]
    fn diff_mode_serde_round_trip() {
        for mode in [
            DiffMode::WorkingTree,
            DiffMode::Staged,
            DiffMode::Commit("abc1234".to_string()),
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: DiffMode = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, mode);
        }
        // Check exact JSON values
        assert_eq!(
            serde_json::to_string(&DiffMode::WorkingTree).unwrap(),
            "\"working_tree\""
        );
        assert_eq!(
            serde_json::to_string(&DiffMode::Staged).unwrap(),
            "\"staged\""
        );
    }
}
