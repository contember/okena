//! ProjectFs trait and the remote-server (HTTP) implementation.

use crate::content_search::{ContentSearchConfig, FileSearchResult, SearchMode};
use crate::file_scan::FileEntry;
use crate::list_directory::DirEntry;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

/// Provides file system operations from either local disk or a remote server.
pub trait ProjectFs: Send + Sync + 'static {
    /// List files in the project (for file search dialog).
    fn list_files(&self, show_ignored: bool) -> Vec<FileEntry>;

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

    /// Get file size in bytes.
    fn file_size(&self, relative_path: &str) -> Result<u64, String>;

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
    );

    /// Project display name (directory name).
    fn project_name(&self) -> String;

    /// Unique project identifier (used for caching).
    fn project_id(&self) -> String;

    /// Local absolute path to the project root, if available. Used to convert
    /// project-relative paths into absolute `PathBuf`s for filesystem
    /// operations (e.g. context-menu rename/delete). Remote projects return
    /// `None` because the root only exists on the remote machine.
    fn project_root(&self) -> Option<PathBuf>;

    /// Daemon-side absolute path for a project-relative path, for display/copy
    /// (e.g. the "Copy Absolute Path" context-menu action). The path lives on
    /// the daemon's filesystem. Returns `None` when the daemon root is unknown.
    fn absolute_path(&self, relative_path: &str) -> Option<String>;
}

/// Remote file system provider — fetches data via HTTP from a remote server.
pub struct RemoteProjectFs {
    host: String,
    port: u16,
    token: String,
    local_endpoint: Option<okena_transport::client::LocalEndpoint>,
    project_id: String,
    project_name: String,
    root: String,
}

impl RemoteProjectFs {
    pub fn new(
        host: String,
        port: u16,
        token: String,
        local_endpoint: Option<okena_transport::client::LocalEndpoint>,
        project_id: String,
        project_name: String,
        root: String,
    ) -> Self {
        Self { host, port, token, local_endpoint, project_id, project_name, root }
    }

    fn post_action(&self, action: okena_core::api::ActionRequest) -> Result<Option<serde_json::Value>, String> {
        okena_transport::remote_action::post_action_with_endpoint(
            &self.host,
            self.port,
            &self.token,
            self.local_endpoint.as_ref(),
            action,
        )
    }
}

impl ProjectFs for RemoteProjectFs {
    fn list_files(&self, show_ignored: bool) -> Vec<FileEntry> {
        let action = okena_core::api::ActionRequest::ListFiles {
            project_id: self.project_id.clone(),
            show_ignored,
        };
        match self.post_action(action) {
            Ok(Some(value)) => serde_json::from_value(value).unwrap_or_else(|e| {
                log::warn!("Failed to deserialize file list: {}", e);
                Vec::new()
            }),
            _ => Vec::new(),
        }
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

    fn file_size(&self, relative_path: &str) -> Result<u64, String> {
        let action = okena_core::api::ActionRequest::FileSize {
            project_id: self.project_id.clone(),
            relative_path: relative_path.to_string(),
        };
        match self.post_action(action)? {
            Some(value) => {
                value.get("size")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "Missing size in response".to_string())
            }
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
    ) {
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
        };
        if let Ok(Some(value)) = self.post_action(action) {
            let results: Vec<FileSearchResult> = serde_json::from_value(value).unwrap_or_else(|e| {
                log::warn!("Failed to deserialize search results: {}", e);
                Vec::new()
            });
            for result in results {
                if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                on_result(result);
            }
        }
    }

    fn project_name(&self) -> String {
        self.project_name.clone()
    }

    fn project_id(&self) -> String {
        self.project_id.clone()
    }

    fn project_root(&self) -> Option<PathBuf> {
        None
    }

    fn absolute_path(&self, relative_path: &str) -> Option<String> {
        if self.root.is_empty() {
            return None;
        }
        let base = self.root.trim_end_matches(['/', '\\']);
        Some(format!("{}/{}", base, relative_path))
    }
}
