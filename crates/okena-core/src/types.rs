use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffMode {
    #[default]
    WorkingTree,
    Staged,
}

impl DiffMode {
    /// Get the display name for this mode.
    pub fn display_name(&self) -> &'static str {
        match self {
            DiffMode::WorkingTree => "Unstaged",
            DiffMode::Staged => "Staged",
        }
    }

    /// Toggle to the other mode.
    pub fn toggle(&self) -> Self {
        match self {
            DiffMode::WorkingTree => DiffMode::Staged,
            DiffMode::Staged => DiffMode::WorkingTree,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_mode_serde_round_trip() {
        for mode in [DiffMode::WorkingTree, DiffMode::Staged] {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: DiffMode = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, mode);
        }
        // Check exact JSON values
        assert_eq!(serde_json::to_string(&DiffMode::WorkingTree).unwrap(), "\"working_tree\"");
        assert_eq!(serde_json::to_string(&DiffMode::Staged).unwrap(), "\"staged\"");
    }
}
