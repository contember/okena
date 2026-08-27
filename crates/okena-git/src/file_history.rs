//! Rename-aware commit history for a single repository-relative file.

use std::path::Path;

use okena_core::process::{command, safe_output};

use crate::{GitError, GitResult};

const SHORT_HASH_LEN: usize = 7;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileHistoryEntry {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub author_email: String,
    pub timestamp: i64,
    pub summary: String,
    /// Repository-relative path used by this version of the file.
    pub path: String,
}

/// Return the commits that touched `relative_path`, newest first.
///
/// Git's `--follow` path walk supplies the commit ids because it preserves
/// history across renames. Commit metadata is then decoded through gix so no
/// delimiter-based parsing is needed for author names or commit subjects.
pub fn get_file_history(
    repo_path: &Path,
    relative_path: &str,
    limit: usize,
) -> GitResult<Vec<FileHistoryEntry>> {
    if limit == 0 || relative_path.is_empty() {
        return Ok(Vec::new());
    }

    let repo_path_str = repo_path
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(repo_path.to_path_buf()))?;
    let limit_arg = format!("-{limit}");
    let output = safe_output(command("git").args([
        "-C",
        repo_path_str,
        "--literal-pathspecs",
        "log",
        "--follow",
        "--format=%x1e%H",
        "--name-only",
        "-z",
        limit_arg.as_str(),
        "--",
        relative_path,
    ]))?;
    if !output.status.success() {
        return Err(GitError::GitExitError {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let repo = crate::gix_helpers::open(repo_path)
        .ok_or_else(|| GitError::ParseError("not a git repository".to_string()))?;

    parse_records(&output.stdout)?
        .into_iter()
        .map(|(hash, path)| load_entry(&repo, &hash, path))
        .collect()
}

fn parse_records(output: &[u8]) -> GitResult<Vec<(String, String)>> {
    output
        .split(|byte| *byte == 0x1e)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let mut fields = record.split(|byte| *byte == 0);
            let hash = fields
                .next()
                .ok_or_else(|| GitError::ParseError("missing file-history hash".to_string()))?;
            let path = fields
                .find(|field| !field.is_empty())
                .ok_or_else(|| GitError::ParseError("missing file-history path".to_string()))?;
            let path = path.strip_prefix(b"\n").unwrap_or(path);
            Ok((
                String::from_utf8(hash.to_vec())
                    .map_err(|error| GitError::ParseError(error.to_string()))?,
                String::from_utf8(path.to_vec())
                    .map_err(|error| GitError::ParseError(error.to_string()))?,
            ))
        })
        .collect()
}

fn load_entry(repo: &gix::Repository, hash: &str, path: String) -> GitResult<FileHistoryEntry> {
    let id = repo
        .rev_parse_single(hash)
        .map_err(|error| GitError::ParseError(error.to_string()))?
        .detach();
    let commit = repo
        .find_commit(id)
        .map_err(|error| GitError::ParseError(error.to_string()))?;
    let author = commit
        .author()
        .map_err(|error| GitError::ParseError(error.to_string()))?;
    let message = commit
        .message()
        .map_err(|error| GitError::ParseError(error.to_string()))?;

    let hash = id.to_hex().to_string();
    Ok(FileHistoryEntry {
        short_hash: hash.chars().take(SHORT_HASH_LEN).collect(),
        hash,
        author: author.name.to_string(),
        author_email: author.email.to_string(),
        timestamp: author.seconds(),
        summary: message.summary().to_string(),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::get_file_history;
    use crate::repository::test_support::{git_in, init_temp_repo};

    #[test]
    fn follows_file_history_across_renames() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("file.txt"), "second").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "edit file"],
        );
        git_in(&repo, &["mv", "file.txt", "renamed.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "rename file"],
        );

        let entries = get_file_history(&repo, "renamed.txt", 10).unwrap();
        let summaries: Vec<_> = entries.iter().map(|entry| entry.summary.as_str()).collect();

        assert_eq!(summaries, ["rename file", "edit file", "init"]);
        assert!(entries.iter().all(|entry| entry.hash.len() == 40));
        assert!(entries.iter().all(|entry| entry.short_hash.len() == 7));
        assert_eq!(entries[0].path, "renamed.txt");
        assert_eq!(entries[1].path, "file.txt");
    }

    #[test]
    fn respects_the_requested_limit() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("file.txt"), "second").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "edit file"],
        );

        let entries = get_file_history(&repo, "file.txt", 1).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].summary, "edit file");
    }
}
