//! `BlameProvider` impls: local (gix-backed) and remote (HTTP).
//!
//! Mirrors the `GitProvider` split in `diff_viewer/provider.rs`. The trait
//! and data types live in `okena-files::blame` so the file viewer doesn't
//! depend on any git crate.

use std::collections::HashMap;
use std::sync::Arc;

use okena_files::blame::{BlameCommit, BlameError, BlameKind, BlameLine, BlameProvider};

/// Local provider — calls `okena_git::get_blame` and converts types.
pub struct LocalBlameProvider {
    path: String,
}

impl LocalBlameProvider {
    pub fn new(path: String) -> Self {
        Self { path }
    }
}

impl BlameProvider for LocalBlameProvider {
    fn get_blame(&self, relative_path: &str) -> Result<Vec<BlameLine>, BlameError> {
        let result = okena_git::get_blame(std::path::Path::new(&self.path), relative_path)
            .map_err(map_git_error)?;
        Ok(result.into_iter().map(convert_line).collect())
    }
}

fn map_git_error(e: okena_git::BlameError) -> BlameError {
    use okena_git::BlameError as E;
    match e {
        E::NotGitRepo => BlameError::NotGitRepo,
        E::NotTracked => BlameError::NotTracked,
        E::NoCommits => BlameError::NoCommits,
        E::Backend(s) | E::Io(s) => BlameError::Backend(s),
    }
}

fn convert_commit(c: &okena_git::BlameCommit) -> BlameCommit {
    BlameCommit {
        hash: c.hash.clone(),
        short_hash: c.short_hash.clone(),
        author: c.author.clone(),
        author_email: c.author_email.clone(),
        timestamp: c.timestamp,
        summary: c.summary.clone(),
    }
}

fn convert_line(l: okena_git::BlameLine) -> BlameLine {
    BlameLine {
        line_number: l.line_number,
        commit: Arc::new(convert_commit(&l.commit)),
        kind: match l.kind {
            okena_git::BlameKind::Committed => BlameKind::Committed,
            okena_git::BlameKind::Uncommitted => BlameKind::Uncommitted,
        },
    }
}

/// Remote provider — fetches blame from the remote server via the `GitBlame`
/// action. The wire format is `Vec<WireBlameLine>` (no `Arc` sharing); this
/// impl re-dedups commits client-side so the rendered gutter keeps one
/// `Arc<BlameCommit>` per unique hash.
pub struct RemoteBlameProvider {
    host: String,
    port: u16,
    token: String,
    project_id: String,
}

impl RemoteBlameProvider {
    pub fn new(host: String, port: u16, token: String, project_id: String) -> Self {
        Self { host, port, token, project_id }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WireBlameLine {
    line_number: usize,
    commit: BlameCommit,
    kind: BlameKind,
}

impl BlameProvider for RemoteBlameProvider {
    fn get_blame(&self, relative_path: &str) -> Result<Vec<BlameLine>, BlameError> {
        let action = okena_core::api::ActionRequest::GitBlame {
            project_id: self.project_id.clone(),
            relative_path: relative_path.to_string(),
        };
        let value = okena_transport::remote_action::post_action(
            &self.host,
            self.port,
            &self.token,
            action,
        )
        .map_err(BlameError::Backend)?;

        let Some(value) = value else {
            return Ok(Vec::new());
        };
        let wire: Vec<WireBlameLine> =
            serde_json::from_value(value).map_err(|e| BlameError::Backend(e.to_string()))?;

        // De-dupe commits into shared Arcs so rendering can compare commit
        // identity cheaply (Arc::ptr_eq) when grouping consecutive lines.
        let mut commit_cache: HashMap<String, Arc<BlameCommit>> = HashMap::new();
        let lines = wire
            .into_iter()
            .map(|w| {
                let commit = commit_cache
                    .entry(w.commit.hash.clone())
                    .or_insert_with(|| Arc::new(w.commit))
                    .clone();
                BlameLine {
                    line_number: w.line_number,
                    commit,
                    kind: w.kind,
                }
            })
            .collect();
        Ok(lines)
    }
}
