//! ProjectFs trait and the remote-server (HTTP) implementation.

use crate::content_search::{ContentSearchConfig, FileSearchResult, SearchMode};
use crate::file_scan::FileEntry;
use crate::list_directory::DirEntry;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectFileMetadata {
    pub size: u64,
    pub modified_at_millis: Option<u64>,
}

/// Provides file system operations from either local disk or a remote server.
pub trait ProjectFs: Send + Sync + 'static {
    /// List files in the project (for file search dialog).
    fn list_files(&self, show_ignored: bool) -> Result<Vec<FileEntry>, String>;

    /// List immediate children of a project-relative directory (for the lazy
    /// file viewer tree). `relative_path = ""` lists the project root.
    fn list_directory(
        &self,
        relative_path: &str,
        show_ignored: bool,
    ) -> Result<Vec<DirEntry>, String>;

    /// Read file content as UTF-8 string.
    fn read_file(&self, relative_path: &str) -> Result<String, String>;

    /// Read file content as raw bytes. Used for binary previews (images).
    fn read_file_bytes(&self, relative_path: &str) -> Result<Vec<u8>, String>;

    /// Get daemon-side file metadata used for size limits and freshness checks.
    fn file_metadata(&self, relative_path: &str) -> Result<ProjectFileMetadata, String>;

    /// Rename a file or folder (project-relative path) to `new_name`.
    fn rename_file(&self, relative_path: &str, new_name: &str) -> Result<(), String>;

    /// Delete a file or folder (project-relative path).
    fn delete_file(&self, relative_path: &str) -> Result<(), String>;

    /// Search content across project files.
    fn search_content(
        &self,
        query: &str,
        config: &ContentSearchConfig,
        cancelled: &AtomicBool,
        on_result: &mut (dyn FnMut(FileSearchResult) + Send),
    ) -> Result<(), String>;

    /// Project display name (directory name).
    fn project_name(&self) -> String;

    /// Unique project identifier (used for caching).
    fn project_id(&self) -> String;

    /// Daemon-side absolute path for a project-relative path, for display/copy
    /// (e.g. the "Copy Absolute Path" context-menu action). The path lives on
    /// the daemon's filesystem. Returns `None` when the daemon root is unknown.
    fn absolute_path(&self, relative_path: &str) -> Option<String>;
}

/// Remote file system provider — fetches data via HTTP from a remote server.
pub struct RemoteProjectFs {
    client: okena_transport::remote_action::RemoteActionClient,
    project_id: String,
    project_name: String,
    root: String,
}

impl RemoteProjectFs {
    pub fn new(
        client: okena_transport::remote_action::RemoteActionClient,
        project_id: String,
        project_name: String,
        root: String,
    ) -> Self {
        Self { client, project_id, project_name, root }
    }

    fn post_action(&self, action: okena_core::api::ActionRequest) -> Result<Option<serde_json::Value>, String> {
        self.client.post_action(action)
    }
}

impl ProjectFs for RemoteProjectFs {
    fn list_files(&self, show_ignored: bool) -> Result<Vec<FileEntry>, String> {
        let action = okena_core::api::ActionRequest::ListFiles {
            project_id: self.project_id.clone(),
            show_ignored,
        };
        let value = self
            .post_action(action)?
            .ok_or_else(|| "Missing file list response".to_string())?;
        serde_json::from_value(value)
            .map_err(|error| format!("Invalid file list response: {error}"))
    }

    fn list_directory(
        &self,
        relative_path: &str,
        show_ignored: bool,
    ) -> Result<Vec<DirEntry>, String> {
        let action = okena_core::api::ActionRequest::ListDirectory {
            project_id: self.project_id.clone(),
            relative_path: relative_path.to_string(),
            show_ignored,
        };
        match self.post_action(action)? {
            Some(value) => serde_json::from_value(value)
                .map_err(|e| format!("Failed to deserialize directory list: {}", e)),
            None => Err("Empty response".to_string()),
        }
    }

    fn read_file(&self, relative_path: &str) -> Result<String, String> {
        let action = okena_core::api::ActionRequest::ReadFile {
            project_id: self.project_id.clone(),
            relative_path: relative_path.to_string(),
        };
        match self.post_action(action)? {
            Some(value) => {
                value.get("content")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .ok_or_else(|| "Missing content in response".to_string())
            }
            None => Err("Empty response".to_string()),
        }
    }

    fn read_file_bytes(&self, relative_path: &str) -> Result<Vec<u8>, String> {
        use base64::Engine as _;
        let action = okena_core::api::ActionRequest::ReadFileBytes {
            project_id: self.project_id.clone(),
            relative_path: relative_path.to_string(),
        };
        match self.post_action(action)? {
            Some(value) => {
                let encoded = value
                    .get("content_b64")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing content_b64 in response".to_string())?;
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|e| format!("Invalid base64 in response: {}", e))
            }
            None => Err("Empty response".to_string()),
        }
    }

    fn file_metadata(&self, relative_path: &str) -> Result<ProjectFileMetadata, String> {
        let action = okena_core::api::ActionRequest::FileSize {
            project_id: self.project_id.clone(),
            relative_path: relative_path.to_string(),
        };
        match self.post_action(action)? {
            Some(value) => Ok(ProjectFileMetadata {
                size: value
                    .get("size")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "Missing size in response".to_string())?,
                modified_at_millis: value
                    .get("modified_at_millis")
                    .and_then(|v| v.as_u64()),
            }),
            None => Err("Empty response".to_string()),
        }
    }

    fn rename_file(&self, relative_path: &str, new_name: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::RenameFile {
            project_id: self.project_id.clone(),
            relative_path: relative_path.to_string(),
            new_name: new_name.to_string(),
        };
        self.post_action(action).map(|_| ())
    }

    fn delete_file(&self, relative_path: &str) -> Result<(), String> {
        let action = okena_core::api::ActionRequest::DeleteFile {
            project_id: self.project_id.clone(),
            relative_path: relative_path.to_string(),
        };
        self.post_action(action).map(|_| ())
    }

    fn search_content(
        &self,
        query: &str,
        config: &ContentSearchConfig,
        cancelled: &AtomicBool,
        on_result: &mut (dyn FnMut(FileSearchResult) + Send),
    ) -> Result<(), String> {
        let mode = match config.mode {
            SearchMode::Literal => "literal",
            SearchMode::Regex => "regex",
            SearchMode::Fuzzy => "fuzzy",
        };
        let action = okena_core::api::ActionRequest::SearchContent {
            project_id: self.project_id.clone(),
            query: query.to_string(),
            case_sensitive: config.case_sensitive,
            mode: mode.to_string(),
            max_results: config.max_results,
            file_glob: config.file_glob.clone(),
            context_lines: config.context_lines,
            show_ignored: config.show_ignored,
        };
        let value = self
            .post_action(action)?
            .ok_or_else(|| "Missing content search response".to_string())?;
        let results: Vec<FileSearchResult> = serde_json::from_value(value)
            .map_err(|error| format!("Invalid content search response: {error}"))?;
        for result in results {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            on_result(result);
        }
        Ok(())
    }

    fn project_name(&self) -> String {
        self.project_name.clone()
    }

    fn project_id(&self) -> String {
        self.project_id.clone()
    }

    fn absolute_path(&self, relative_path: &str) -> Option<String> {
        if self.root.is_empty() {
            return None;
        }
        let base = self.root.trim_end_matches(['/', '\\']);
        Some(format!("{}/{}", base, relative_path))
    }
}
