//! DiffProvider trait and implementations for local and remote git diffs.

use crate::git::{DiffMode, DiffResult};

/// Provides git diff data from either local git commands or a remote server.
pub trait DiffProvider: Send + Sync + 'static {
    fn is_git_repo(&self) -> bool;
    fn get_diff(&self, mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String>;
    fn get_file_contents(&self, file_path: &str, mode: DiffMode) -> (Option<String>, Option<String>);
}

/// Local diff provider — wraps existing git functions.
pub struct LocalDiffProvider {
    path: String,
}

impl LocalDiffProvider {
    pub fn new(path: String) -> Self {
        Self { path }
    }
}

impl DiffProvider for LocalDiffProvider {
    fn is_git_repo(&self) -> bool {
        crate::git::is_git_repo(std::path::Path::new(&self.path))
    }

    fn get_diff(&self, mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String> {
        crate::git::get_diff_with_options(std::path::Path::new(&self.path), mode, ignore_whitespace)
    }

    fn get_file_contents(&self, file_path: &str, mode: DiffMode) -> (Option<String>, Option<String>) {
        crate::git::get_file_contents_for_diff(std::path::Path::new(&self.path), file_path, mode)
    }
}

/// Remote diff provider — fetches diff data via HTTP from a remote server.
pub struct RemoteDiffProvider {
    host: String,
    port: u16,
    token: String,
    project_id: String,
}

impl RemoteDiffProvider {
    pub fn new(host: String, port: u16, token: String, project_id: String) -> Self {
        Self { host, port, token, project_id }
    }

    fn post_action(&self, action: okena_core::api::ActionRequest) -> Result<Option<serde_json::Value>, String> {
        let url = format!("http://{}:{}/v1/actions", self.host, self.port);
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(&url)
            .bearer_auth(&self.token)
            .json(&action)
            .send()
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(format!("Server returned {}: {}", status, body));
        }

        let body: serde_json::Value = resp.json().map_err(|e| format!("Failed to parse response: {}", e))?;

        if let Some(error) = body.get("error").and_then(|e| e.as_str()) {
            return Err(error.to_string());
        }

        Ok(body.get("result").cloned())
    }
}

impl DiffProvider for RemoteDiffProvider {
    fn is_git_repo(&self) -> bool {
        // Remote projects with git status are always repos
        true
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
}
