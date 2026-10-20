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

/// One column of the terminal status board. `id` is the stable key that
/// `ProjectData::terminal_statuses` stores; `name` is what the user sees and
/// may rename without touching any terminal.
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

/// A stable id for a new column named `name`, unique among `existing`:
/// the lower-cased name with every run of non-alphanumerics folded to one
/// `-` (`"column"` when nothing is left), then `-2`, `-3`, … while taken.
pub fn board_column_id(name: &str, existing: &[BoardColumn]) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.extend(ch.to_lowercase());
        } else {
            pending_dash = true;
        }
    }
    if slug.is_empty() {
        slug = "column".to_string();
    }
    let taken = |id: &str| existing.iter().any(|c| c.id == id);
    if !taken(&slug) {
        return slug;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{slug}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod board_column_tests {
    use super::*;

    #[test]
    fn board_column_id_slugs_and_dedups() {
        let existing = vec![BoardColumn::new("review", "Review")];
        assert_eq!(board_column_id("In Progress", &existing), "in-progress");
        assert_eq!(board_column_id("  Needs  QA! ", &existing), "needs-qa");
        assert_eq!(board_column_id("Review", &existing), "review-2");
        assert_eq!(board_column_id("???", &existing), "column");
        let mut more = existing.clone();
        more.push(BoardColumn::new("review-2", "Review"));
        assert_eq!(board_column_id("review", &more), "review-3");
    }
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
