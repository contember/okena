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
}
