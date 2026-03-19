use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
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
    BranchCompare { base: String, head: String },
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
        for mode in [DiffMode::WorkingTree, DiffMode::Staged, DiffMode::Commit("abc1234".to_string())] {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: DiffMode = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, mode);
        }
        // Check exact JSON values
        assert_eq!(serde_json::to_string(&DiffMode::WorkingTree).unwrap(), "\"working_tree\"");
        assert_eq!(serde_json::to_string(&DiffMode::Staged).unwrap(), "\"staged\"");
    }
}
