//! Per-file git history data and provider interface.

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileHistoryEntry {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub author_email: String,
    pub timestamp: i64,
    pub summary: String,
    pub path: String,
}

/// Source of commit history and historical text for the file viewer.
///
/// Calls run on a background thread, so implementations may block on I/O.
pub trait FileHistoryProvider: Send + Sync + 'static {
    fn get_file_history(
        &self,
        relative_path: &str,
        limit: usize,
    ) -> Result<Vec<FileHistoryEntry>, String>;

    fn get_file_at_revision(
        &self,
        relative_path: &str,
        revision: &str,
    ) -> Result<Option<String>, String>;
}
