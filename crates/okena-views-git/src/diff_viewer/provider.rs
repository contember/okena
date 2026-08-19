//! GitProvider trait and the remote-server (HTTP) implementation.

use okena_git::{BranchList, CommitLogEntry, DiffMode, DiffResult, FileDiffSummary};
use serde::de::DeserializeOwned;

/// Provides git data from either local git commands or a remote server.
pub trait GitProvider: Send + Sync + 'static {
    fn is_git_repo(&self) -> bool;
    /// True for providers that perform mutations on the local filesystem.
    /// Used by UI to gate destructive actions (e.g. branch switching is
    /// disabled when this is false).
    fn supports_mutations(&self) -> bool {
        true
    }
    fn get_diff(&self, mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String>;
    fn get_file_contents(
        &self,
        file_path: &str,
        mode: DiffMode,
    ) -> Result<(Option<String>, Option<String>), String>;
    fn get_diff_file_summary(&self) -> Result<Vec<FileDiffSummary>, String>;
    fn get_commit_graph(
        &self,
        count: usize,
        branch: Option<&str>,
    ) -> Result<Vec<CommitLogEntry>, String>;
    fn list_branches(&self) -> Result<Vec<String>, String>;
    /// List branches split into local/remote with the current branch name.
    /// Default implementation falls back to [`list_branches`] and classifies
    /// remote refs as anything containing a `/`.
    fn list_branches_classified(&self) -> Result<BranchList, String> {
        let all = self.list_branches()?;
        let (remote, local): (Vec<String>, Vec<String>) =
            all.into_iter().partition(|n| n.contains('/'));
        Ok(BranchList {
            local,
            remote,
            current: None,
            // The name-only fallback carries no per-branch metadata; the
            // picker degrades to plain names.
            details: Default::default(),
        })
    }

    // ── Mutations (Phase 1: per-file) ──────────────────────────────────────
    fn stage_file(&self, file_path: &str) -> Result<(), String>;
    fn unstage_file(&self, file_path: &str) -> Result<(), String>;
    fn discard_file(&self, file_path: &str) -> Result<(), String>;
    fn delete_file(&self, file_path: &str) -> Result<(), String>;

    // ── Mutations (Phase 2: branches) ──────────────────────────────────────
    fn checkout_local_branch(&self, _branch: &str) -> Result<(), String> {
        Err("Branch checkout is not supported by this provider".to_string())
    }
    fn checkout_remote_branch(&self, _remote_branch: &str) -> Result<(), String> {
        Err("Branch checkout is not supported by this provider".to_string())
    }
    fn create_and_checkout_branch(
        &self,
        _new_name: &str,
        _start_point: Option<&str>,
    ) -> Result<(), String> {
        Err("Branch creation is not supported by this provider".to_string())
    }

    /// Absolute path of a file in the working tree, used for copy-absolute-path.
    /// Returns None when the provider can't resolve it (e.g. remote without
    /// a sensible local absolute path).
    fn absolute_file_path(&self, file_path: &str) -> Option<String>;
}

/// Remote git provider — fetches git data via HTTP from a remote server.
pub struct RemoteGitProvider {
    client: okena_transport::remote_action::RemoteActionClient,
    project_id: String,
    root: String,
}

impl RemoteGitProvider {
    pub fn new(
        client: okena_transport::remote_action::RemoteActionClient,
        project_id: String,
        root: String,
    ) -> Self {
        Self {
            client,
            project_id,
            root,
        }
    }

    fn post_action(
        &self,
        action: okena_core::api::ActionRequest,
    ) -> Result<Option<serde_json::Value>, String> {
        self.client.post_action(action)
    }

    fn post_json<T>(&self, action: okena_core::api::ActionRequest, label: &str) -> Result<T, String>
    where
        T: DeserializeOwned,
    {
        let value = self
            .post_action(action)?
            .ok_or_else(|| format!("Missing {label} response"))?;
        serde_json::from_value(value).map_err(|error| format!("Invalid {label} response: {error}"))
    }

    fn post_unit(&self, action: okena_core::api::ActionRequest) -> Result<(), String> {
        self.post_action(action).map(|_| ())
    }
}

impl GitProvider for RemoteGitProvider {
    fn is_git_repo(&self) -> bool {
        true
    }

    fn supports_mutations(&self) -> bool {
        true
    }

    fn get_diff(&self, mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String> {
        let action = okena_core::api::ActionRequest::GitDiff {
            project_id: self.project_id.clone(),
            mode,
            ignore_whitespace,
        };
        self.post_json(action, "diff")
    }

    fn get_file_contents(
        &self,
        file_path: &str,
        mode: DiffMode,
    ) -> Result<(Option<String>, Option<String>), String> {
        let action = okena_core::api::ActionRequest::GitFileContents {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
            mode,
        };
        let value = self
            .post_action(action)?
            .ok_or_else(|| "Missing file contents response".to_string())?;
        let old = value
            .get("old_content")
            .and_then(|v| v.as_str())
            .map(String::from);
        let new = value
            .get("new_content")
            .and_then(|v| v.as_str())
            .map(String::from);
        Ok((old, new))
    }

    fn get_diff_file_summary(&self) -> Result<Vec<FileDiffSummary>, String> {
        let action = okena_core::api::ActionRequest::GitDiffSummary {
            project_id: self.project_id.clone(),
        };
        self.post_json(action, "diff summary")
    }

    fn get_commit_graph(
        &self,
        count: usize,
        branch: Option<&str>,
    ) -> Result<Vec<CommitLogEntry>, String> {
        let action = okena_core::api::ActionRequest::GitCommitGraph {
            project_id: self.project_id.clone(),
            count,
            branch: branch.map(String::from),
        };
        self.post_json(action, "commit graph")
    }

    fn list_branches(&self) -> Result<Vec<String>, String> {
        let action = okena_core::api::ActionRequest::GitListBranches {
            project_id: self.project_id.clone(),
        };
        self.post_json(action, "branch list")
    }

    fn list_branches_classified(&self) -> Result<BranchList, String> {
        let action = okena_core::api::ActionRequest::GitListBranchesClassified {
            project_id: self.project_id.clone(),
        };
        self.post_json(action, "classified branch list")
    }

    fn stage_file(&self, file_path: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::GitStageFile {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
        };
        self.post_unit(action)
    }

    fn unstage_file(&self, file_path: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::GitUnstageFile {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
        };
        self.post_unit(action)
    }

    fn discard_file(&self, file_path: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::GitDiscardFile {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
        };
        self.post_unit(action)
    }

    fn delete_file(&self, file_path: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::DeleteFile {
            project_id: self.project_id.clone(),
            relative_path: file_path.to_string(),
        };
        self.post_unit(action)
    }

    fn checkout_local_branch(&self, branch: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::GitCheckoutLocalBranch {
            project_id: self.project_id.clone(),
            branch: branch.to_string(),
        };
        self.post_unit(action)
    }

    fn checkout_remote_branch(&self, remote_branch: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::GitCheckoutRemoteBranch {
            project_id: self.project_id.clone(),
            remote_branch: remote_branch.to_string(),
        };
        self.post_unit(action)
    }

    fn create_and_checkout_branch(
        &self,
        new_name: &str,
        start_point: Option<&str>,
    ) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::GitCreateAndCheckoutBranch {
            project_id: self.project_id.clone(),
            new_name: new_name.to_string(),
            start_point: start_point.map(String::from),
        };
        self.post_unit(action)
    }

    fn absolute_file_path(&self, file_path: &str) -> Option<String> {
        if self.root.is_empty() {
            return None;
        }
        let base = self.root.trim_end_matches(['/', '\\']);
        Some(format!("{}/{}", base, file_path))
    }
}
