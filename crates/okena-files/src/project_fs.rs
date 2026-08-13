//! Filesystem scope trait and daemon-backed implementations.

use crate::content_search::{ContentSearchConfig, FileSearchResult, SearchMode};
use crate::file_scan::FileEntry;
use crate::list_directory::DirEntry;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectFileMetadata {
    pub size: u64,
    pub modified_at_millis: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileSourceAction {
    OpenExternally,
    Download,
}

/// Provides browser operations inside a movable daemon filesystem scope.
pub trait ProjectFs: Send + Sync + 'static {
    /// List files in the current scope (for file search dialog).
    fn list_files(&self, show_ignored: bool) -> Result<Vec<FileEntry>, String>;

    /// List immediate children of a scope-relative directory (for the lazy
    /// file viewer tree). `relative_path = ""` lists the scope root.
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

    /// Scope display name (directory name).
    fn project_name(&self) -> String;

    /// Unique scope identifier (used for caching and client-side paths).
    fn project_id(&self) -> String;

    /// Daemon-side absolute path for a scope-relative path, for display/copy
    /// (e.g. the "Copy Absolute Path" context-menu action). The path lives on
    /// the daemon's filesystem. Returns `None` when the daemon root is unknown.
    fn absolute_path(&self, relative_path: &str) -> Option<String>;

    /// Origin shown in the viewer header, including the daemon that owns it.
    fn source_label(&self, relative_path: &str) -> String;

    /// Action that can move the viewed file out of the integrated viewer.
    fn source_action(&self) -> FileSourceAction;

    /// Stream the complete file into a local writer.
    fn download_file(
        &self,
        relative_path: &str,
        writer: &mut dyn std::io::Write,
    ) -> Result<(), String>;

    /// Absolute daemon path that anchors relative operations in this browser.
    fn scope_path(&self) -> String;

    /// Resolve an absolute daemon path for navigation and breadcrumbs.
    fn resolve_path(&self, path: &str) -> Result<okena_core::api::ResolvedPath, String>;

    /// Create an equivalent provider rooted at a resolved directory.
    fn scoped_to(&self, scope: okena_core::api::ResolvedPath) -> Arc<dyn ProjectFs>;

    /// Convert a daemon absolute path into this scope's relative identifier.
    fn relative_path(&self, absolute_path: &str) -> Option<String>;

    /// Daemon identity shown before the filesystem breadcrumb.
    fn owner_label(&self) -> String;
}

fn owner_label(client: &okena_transport::remote_action::RemoteActionClient) -> String {
    if client.is_local_daemon() {
        "local".to_string()
    } else {
        client.connection_name().to_string()
    }
}

pub(crate) fn join_daemon_path(root: &str, relative_path: &str) -> String {
    if relative_path.is_empty() {
        return root.to_string();
    }
    let separator = if root.contains('\\') && !root.contains('/') {
        '\\'
    } else {
        '/'
    };
    let relative_path = relative_path.replace(['/', '\\'], &separator.to_string());
    format!(
        "{}{}{}",
        root.trim_end_matches(['/', '\\']),
        separator,
        relative_path
    )
}

fn relative_daemon_path(root: &str, absolute_path: &str) -> Option<String> {
    let normalize = |value: &str| value.replace('\\', "/");
    let root = normalize(root);
    let absolute = normalize(absolute_path);
    let root = if root == "/" {
        root
    } else {
        root.trim_end_matches('/').to_string()
    };
    if absolute == root {
        return Some(String::new());
    }
    absolute
        .strip_prefix(&root)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .map(str::to_string)
}

/// Filesystem provider rooted at an arbitrary daemon directory.
#[derive(Clone)]
pub struct RemotePathFs {
    client: okena_transport::remote_action::RemoteActionClient,
    scope: okena_core::api::ResolvedPath,
}

impl RemotePathFs {
    pub fn new(
        client: okena_transport::remote_action::RemoteActionClient,
        scope: okena_core::api::ResolvedPath,
    ) -> Self {
        Self { client, scope }
    }

    pub fn new_unresolved(
        client: okena_transport::remote_action::RemoteActionClient,
        name: String,
        root: String,
    ) -> Self {
        Self::new(
            client,
            okena_core::api::ResolvedPath {
                canonical_path: root,
                name,
                kind: okena_core::api::ResolvedPathKind::Directory,
                size: 0,
                modified_at_millis: None,
                project_id: None,
                relative_path: None,
                breadcrumbs: Vec::new(),
            },
        )
    }

    fn post_action(
        &self,
        action: okena_core::api::ActionRequest,
    ) -> Result<Option<serde_json::Value>, String> {
        self.client.post_action(action)
    }
}

impl ProjectFs for RemotePathFs {
    fn list_files(&self, show_ignored: bool) -> Result<Vec<FileEntry>, String> {
        let value = self
            .post_action(okena_core::api::ActionRequest::ListPathFiles {
                root: self.scope.canonical_path.clone(),
                show_ignored,
            })?
            .ok_or_else(|| "Missing file list response".to_string())?;
        serde_json::from_value(value)
            .map_err(|error| format!("Invalid file list response: {error}"))
    }

    fn list_directory(
        &self,
        relative_path: &str,
        show_ignored: bool,
    ) -> Result<Vec<DirEntry>, String> {
        let value = self
            .post_action(okena_core::api::ActionRequest::ListPathDirectory {
                root: self.scope.canonical_path.clone(),
                relative_path: relative_path.to_string(),
                show_ignored,
            })?
            .ok_or_else(|| "Missing directory list response".to_string())?;
        serde_json::from_value(value)
            .map_err(|error| format!("Invalid directory list response: {error}"))
    }

    fn read_file(&self, relative_path: &str) -> Result<String, String> {
        let value = self
            .post_action(okena_core::api::ActionRequest::ReadPathFile {
                root: self.scope.canonical_path.clone(),
                relative_path: relative_path.to_string(),
            })?
            .ok_or_else(|| "Missing file response".to_string())?;
        value
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "Missing content in response".to_string())
    }

    fn read_file_bytes(&self, relative_path: &str) -> Result<Vec<u8>, String> {
        use base64::Engine as _;
        let value = self
            .post_action(okena_core::api::ActionRequest::ReadPathFileBytes {
                root: self.scope.canonical_path.clone(),
                relative_path: relative_path.to_string(),
            })?
            .ok_or_else(|| "Missing file response".to_string())?;
        let encoded = value
            .get("content_b64")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Missing content_b64 in response".to_string())?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| format!("Invalid base64 in response: {error}"))
    }

    fn file_metadata(&self, relative_path: &str) -> Result<ProjectFileMetadata, String> {
        let value = self
            .post_action(okena_core::api::ActionRequest::PathFileSize {
                root: self.scope.canonical_path.clone(),
                relative_path: relative_path.to_string(),
            })?
            .ok_or_else(|| "Missing file metadata response".to_string())?;
        Ok(ProjectFileMetadata {
            size: value
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| "Missing size in response".to_string())?,
            modified_at_millis: value
                .get("modified_at_millis")
                .and_then(serde_json::Value::as_u64),
        })
    }

    fn rename_file(&self, relative_path: &str, new_name: &str) -> Result<(), String> {
        self.post_action(okena_core::api::ActionRequest::RenamePath {
            root: self.scope.canonical_path.clone(),
            relative_path: relative_path.to_string(),
            new_name: new_name.to_string(),
        })
        .map(|_| ())
    }

    fn delete_file(&self, relative_path: &str) -> Result<(), String> {
        self.post_action(okena_core::api::ActionRequest::DeletePath {
            root: self.scope.canonical_path.clone(),
            relative_path: relative_path.to_string(),
        })
        .map(|_| ())
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
        let action = okena_core::api::ActionRequest::SearchPathContent {
            root: self.scope.canonical_path.clone(),
            query: query.to_string(),
            case_sensitive: config.case_sensitive,
            mode: mode.to_string(),
            max_results: config.max_results,
            file_glob: config.file_glob.clone(),
            context_lines: config.context_lines,
            show_ignored: config.show_ignored,
        };
        let value = self
            .client
            .post_action_cancellable(action, cancelled)?
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
        self.scope.name.clone()
    }

    fn project_id(&self) -> String {
        format!(
            "path:{}:{}",
            owner_label(&self.client),
            self.scope.canonical_path
        )
    }

    fn absolute_path(&self, relative_path: &str) -> Option<String> {
        Some(join_daemon_path(&self.scope.canonical_path, relative_path))
    }

    fn source_label(&self, relative_path: &str) -> String {
        let path = self
            .absolute_path(relative_path)
            .unwrap_or_else(|| relative_path.to_string());
        format!("{}:{path}", owner_label(&self.client))
    }

    fn source_action(&self) -> FileSourceAction {
        if self.client.is_local_daemon() {
            FileSourceAction::OpenExternally
        } else {
            FileSourceAction::Download
        }
    }

    fn download_file(
        &self,
        relative_path: &str,
        writer: &mut dyn std::io::Write,
    ) -> Result<(), String> {
        self.client.download_file(
            &okena_core::api::FileDownloadRequest::Path {
                root: self.scope.canonical_path.clone(),
                relative_path: relative_path.to_string(),
            },
            writer,
        )
    }

    fn scope_path(&self) -> String {
        self.scope.canonical_path.clone()
    }

    fn resolve_path(&self, path: &str) -> Result<okena_core::api::ResolvedPath, String> {
        let value = self
            .post_action(okena_core::api::ActionRequest::ResolvePath {
                path: path.to_string(),
            })?
            .ok_or_else(|| "Missing resolved path response".to_string())?;
        serde_json::from_value(value)
            .map_err(|error| format!("Invalid resolved path response: {error}"))
    }

    fn scoped_to(&self, scope: okena_core::api::ResolvedPath) -> Arc<dyn ProjectFs> {
        Arc::new(Self::new(self.client.clone(), scope))
    }

    fn relative_path(&self, absolute_path: &str) -> Option<String> {
        relative_daemon_path(&self.scope.canonical_path, absolute_path)
    }

    fn owner_label(&self) -> String {
        owner_label(&self.client)
    }
}

#[cfg(test)]
mod tests {
    use super::{join_daemon_path, relative_daemon_path};

    #[test]
    fn daemon_paths_round_trip_with_unix_separators() {
        let absolute = join_daemon_path("/srv/apps", "demo/src/main.rs");
        assert_eq!(absolute, "/srv/apps/demo/src/main.rs");
        assert_eq!(
            relative_daemon_path("/srv/apps", &absolute).as_deref(),
            Some("demo/src/main.rs")
        );
    }

    #[test]
    fn daemon_paths_round_trip_with_windows_separators() {
        let absolute = join_daemon_path(r"C:\Users\dev", "demo/src/main.rs");
        assert_eq!(absolute, r"C:\Users\dev\demo\src\main.rs");
        assert_eq!(
            relative_daemon_path(r"C:\Users\dev", &absolute).as_deref(),
            Some("demo/src/main.rs")
        );
    }

    #[test]
    fn relative_path_rejects_sibling_prefixes() {
        assert_eq!(relative_daemon_path("/srv/app", "/srv/application/x"), None);
    }
}
