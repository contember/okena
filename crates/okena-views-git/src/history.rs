//! Remote file-history provider for the file viewer.

use okena_files::history::{FileHistoryEntry, FileHistoryProvider};

pub struct RemoteFileHistoryProvider {
    client: okena_transport::remote_action::RemoteActionClient,
    project_id: String,
}

impl RemoteFileHistoryProvider {
    pub fn new(
        client: okena_transport::remote_action::RemoteActionClient,
        project_id: String,
    ) -> Self {
        Self { client, project_id }
    }
}

impl FileHistoryProvider for RemoteFileHistoryProvider {
    fn get_file_history(
        &self,
        relative_path: &str,
        limit: usize,
    ) -> Result<Vec<FileHistoryEntry>, String> {
        let value = self
            .client
            .post_action(okena_core::api::ActionRequest::GitFileHistory {
                project_id: self.project_id.clone(),
                relative_path: relative_path.to_string(),
                count: limit,
            })?
            .ok_or_else(|| "Missing file history response".to_string())?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    fn get_file_at_revision(
        &self,
        relative_path: &str,
        revision: &str,
    ) -> Result<Option<String>, String> {
        let value = self
            .client
            .post_action(okena_core::api::ActionRequest::GitFileContents {
                project_id: self.project_id.clone(),
                file_path: relative_path.to_string(),
                mode: okena_core::types::DiffMode::Commit(revision.to_string()),
            })?
            .ok_or_else(|| "Missing file contents response".to_string())?;
        Ok(value
            .get("new_content")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }

    fn get_file_at_source(
        &self,
        relative_path: &str,
        source: &okena_files::file_viewer::FileSource,
    ) -> Result<Option<String>, String> {
        let (mode, content_key) = match source {
            okena_files::file_viewer::FileSource::WorkingTree => {
                (okena_core::types::DiffMode::WorkingTree, "new_content")
            }
            okena_files::file_viewer::FileSource::GitRevision(revision) => (
                okena_core::types::DiffMode::Commit(revision.clone()),
                "new_content",
            ),
            okena_files::file_viewer::FileSource::Index => {
                (okena_core::types::DiffMode::Staged, "new_content")
            }
            okena_files::file_viewer::FileSource::BranchMergeBase { base, head } => (
                okena_core::types::DiffMode::BranchCompare {
                    base: base.clone(),
                    head: head.clone(),
                },
                "old_content",
            ),
        };
        let value = self
            .client
            .post_action(okena_core::api::ActionRequest::GitFileContents {
                project_id: self.project_id.clone(),
                file_path: relative_path.to_string(),
                mode,
            })?
            .ok_or_else(|| "Missing file contents response".to_string())?;
        Ok(value
            .get(content_key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }
}
