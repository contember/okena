//! GitProvider trait and implementations for local and remote git operations.

use okena_git::{BranchList, DiffMode, DiffResult, FileDiffSummary, GraphRow};

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
    fn get_file_contents(&self, file_path: &str, mode: DiffMode) -> (Option<String>, Option<String>);
    fn get_diff_file_summary(&self) -> Vec<FileDiffSummary>;
    fn get_commit_graph(&self, count: usize, branch: Option<&str>) -> Vec<GraphRow>;
    fn list_branches(&self) -> Vec<String>;
    /// List branches split into local/remote with the current branch name.
    /// Default implementation falls back to [`list_branches`] and classifies
    /// remote refs as anything containing a `/`.
    fn list_branches_classified(&self) -> BranchList {
        let all = self.list_branches();
        let (remote, local): (Vec<String>, Vec<String>) =
            all.into_iter().partition(|n| n.contains('/'));
        BranchList { local, remote, current: None }
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

/// Local git provider — wraps existing git functions.
pub struct LocalGitProvider {
    path: String,
}

impl LocalGitProvider {
    pub fn new(path: String) -> Self {
        Self { path }
    }
}

impl GitProvider for LocalGitProvider {
    fn is_git_repo(&self) -> bool {
        okena_git::is_git_repo(std::path::Path::new(&self.path))
    }

    fn get_diff(&self, mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String> {
        okena_git::get_diff_with_options(std::path::Path::new(&self.path), mode, ignore_whitespace)
            .map_err(|e| e.to_string())
    }

    fn get_file_contents(&self, file_path: &str, mode: DiffMode) -> (Option<String>, Option<String>) {
        okena_git::get_file_contents_for_diff(std::path::Path::new(&self.path), file_path, mode)
    }

    fn get_diff_file_summary(&self) -> Vec<FileDiffSummary> {
        okena_git::get_diff_file_summary(std::path::Path::new(&self.path))
    }

    fn get_commit_graph(&self, count: usize, branch: Option<&str>) -> Vec<GraphRow> {
        okena_git::get_commit_graph(std::path::Path::new(&self.path), count, branch)
    }

    fn list_branches(&self) -> Vec<String> {
        okena_git::list_branches(std::path::Path::new(&self.path))
    }

    fn list_branches_classified(&self) -> BranchList {
        okena_git::list_branches_classified(std::path::Path::new(&self.path))
    }

    fn checkout_local_branch(&self, branch: &str) -> Result<(), String> {
        okena_git::checkout_local_branch(std::path::Path::new(&self.path), branch)
            .map_err(|e| e.to_string())
    }

    fn checkout_remote_branch(&self, remote_branch: &str) -> Result<(), String> {
        okena_git::checkout_remote_branch(std::path::Path::new(&self.path), remote_branch)
            .map_err(|e| e.to_string())
    }

    fn create_and_checkout_branch(
        &self,
        new_name: &str,
        start_point: Option<&str>,
    ) -> Result<(), String> {
        okena_git::create_and_checkout_branch(
            std::path::Path::new(&self.path),
            new_name,
            start_point,
        )
        .map_err(|e| e.to_string())
    }

    fn stage_file(&self, file_path: &str) -> Result<(), String> {
        okena_git::stage_file(std::path::Path::new(&self.path), file_path)
            .map_err(|e| e.to_string())
    }

    fn unstage_file(&self, file_path: &str) -> Result<(), String> {
        okena_git::unstage_file(std::path::Path::new(&self.path), file_path)
            .map_err(|e| e.to_string())
    }

    fn discard_file(&self, file_path: &str) -> Result<(), String> {
        okena_git::discard_file_changes(std::path::Path::new(&self.path), file_path)
            .map_err(|e| e.to_string())
    }

    fn delete_file(&self, file_path: &str) -> Result<(), String> {
        let abs = std::path::Path::new(&self.path).join(file_path);
        std::fs::remove_file(&abs)
            .map_err(|e| format!("Failed to delete file: {}", e))
    }

    fn absolute_file_path(&self, file_path: &str) -> Option<String> {
        Some(
            std::path::Path::new(&self.path)
                .join(file_path)
                .to_string_lossy()
                .to_string(),
        )
    }
}

/// Remote git provider — fetches git data via HTTP from a remote server.
pub struct RemoteGitProvider {
    host: String,
    port: u16,
    token: String,
    project_id: String,
}

impl RemoteGitProvider {
    pub fn new(host: String, port: u16, token: String, project_id: String) -> Self {
        Self { host, port, token, project_id }
    }

    fn post_action(&self, action: okena_core::api::ActionRequest) -> Result<Option<serde_json::Value>, String> {
        okena_core::remote_action::post_action(&self.host, self.port, &self.token, action)
    }
}

impl GitProvider for RemoteGitProvider {
    fn is_git_repo(&self) -> bool {
        true
    }

    fn supports_mutations(&self) -> bool {
        false
    }

    fn get_diff(&self, mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String> {
        let action = okena_core::api::ActionRequest::GitDiff {
            project_id: self.project_id.clone(),
            mode,
            ignore_whitespace,
        };
        let result = self.post_action(action)?;
        match result {
            Some(value) => serde_json::from_value(value).map_err(|e| format!("Failed to deserialize DiffResult: {}", e)),
            None => Ok(DiffResult::default()),
        }
    }

    fn get_file_contents(&self, file_path: &str, mode: DiffMode) -> (Option<String>, Option<String>) {
        let action = okena_core::api::ActionRequest::GitFileContents {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
            mode,
        };
        match self.post_action(action) {
            Ok(Some(value)) => {
                let old = value.get("old_content").and_then(|v| v.as_str()).map(String::from);
                let new = value.get("new_content").and_then(|v| v.as_str()).map(String::from);
                (old, new)
            }
            _ => (None, None),
        }
    }

    fn get_diff_file_summary(&self) -> Vec<FileDiffSummary> {
        let action = okena_core::api::ActionRequest::GitDiffSummary {
            project_id: self.project_id.clone(),
        };
        match self.post_action(action) {
            Ok(Some(value)) => serde_json::from_value(value).unwrap_or_else(|e| {
                log::warn!("Failed to deserialize diff summary: {}", e);
                Vec::new()
            }),
            _ => Vec::new(),
        }
    }

    fn get_commit_graph(&self, count: usize, branch: Option<&str>) -> Vec<GraphRow> {
        let action = okena_core::api::ActionRequest::GitCommitGraph {
            project_id: self.project_id.clone(),
            count,
            branch: branch.map(String::from),
        };
        match self.post_action(action) {
            Ok(Some(value)) => serde_json::from_value(value).unwrap_or_else(|e| {
                log::warn!("Failed to deserialize commit graph: {}", e);
                Vec::new()
            }),
            _ => Vec::new(),
        }
    }

    fn list_branches(&self) -> Vec<String> {
        let action = okena_core::api::ActionRequest::GitListBranches {
            project_id: self.project_id.clone(),
        };
        match self.post_action(action) {
            Ok(Some(value)) => serde_json::from_value(value).unwrap_or_else(|e| {
                log::warn!("Failed to deserialize branch list: {}", e);
                Vec::new()
            }),
            _ => Vec::new(),
        }
    }

    fn stage_file(&self, file_path: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::GitStageFile {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
        };
        self.post_action(action).map(|_| ())
    }

    fn unstage_file(&self, file_path: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::GitUnstageFile {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
        };
        self.post_action(action).map(|_| ())
    }

    fn discard_file(&self, file_path: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::GitDiscardFile {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
        };
        self.post_action(action).map(|_| ())
    }

    fn delete_file(&self, file_path: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::DeleteFile {
            project_id: self.project_id.clone(),
            relative_path: file_path.to_string(),
        };
        self.post_action(action).map(|_| ())
    }

    fn absolute_file_path(&self, _file_path: &str) -> Option<String> {
        None
    }
}
