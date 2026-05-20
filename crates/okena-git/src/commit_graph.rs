//! In-process commit log fetcher built on `gix`.
//!
//! Replaces the previous `git log --graph` shellout. We return raw commit
//! topology (each commit + its parent short hashes); lane/graph layout
//! happens on the consumer side from this DAG. No ASCII parsing.

use std::collections::HashMap;
use std::path::Path;

use gix::ObjectId;
use gix::revision::walk::Sorting;
use gix::traverse::commit::simple::CommitTimeOrder;

use crate::CommitLogEntry;

/// Number of hex chars used for the short hash. Matches `git log`'s default
/// abbreviation width for small/medium repos; collisions are tolerable
/// because consumers use this as a display key over a single response.
const SHORT_HASH_LEN: usize = 7;

fn short_hash(id: &ObjectId) -> String {
    let mut s = id.to_hex().to_string();
    s.truncate(SHORT_HASH_LEN);
    s
}

/// Build `commit-id -> [ref label]` map covering local branches, remote
/// branches, tags and HEAD. Labels follow `git log --decorate=short`
/// conventions so the same UI styling continues to work.
fn collect_refs(repo: &gix::Repository) -> HashMap<ObjectId, Vec<String>> {
    let mut map: HashMap<ObjectId, Vec<String>> = HashMap::new();

    // Track the branch HEAD currently points to so we can render
    // "HEAD -> main" instead of two separate labels.
    let head_branch: Option<String> = crate::repository::head_branch_short(repo);
    let head_id: Option<ObjectId> = repo.head_id().ok().map(|id| id.detach());

    if let Ok(platform) = repo.references() {
        // Local branches
        if let Ok(iter) = platform.local_branches() {
            for reference in iter.flatten() {
                let mut r = reference;
                let short = r.name().shorten().to_string();
                if let Ok(id) = r.peel_to_id() {
                    let oid = id.detach();
                    let label = if head_branch.as_deref() == Some(&short) {
                        format!("HEAD -> {short}")
                    } else {
                        short
                    };
                    map.entry(oid).or_default().push(label);
                }
            }
        }
        // Remote branches
        if let Ok(iter) = platform.remote_branches() {
            for reference in iter.flatten() {
                let mut r = reference;
                let short = r.name().shorten().to_string();
                if let Ok(id) = r.peel_to_id() {
                    map.entry(id.detach()).or_default().push(short);
                }
            }
        }
        // Tags — peel through tag objects to the underlying commit.
        if let Ok(iter) = platform.tags() {
            for reference in iter.flatten() {
                let mut r = reference;
                let short = r.name().shorten().to_string();
                if let Ok(id) = r.peel_to_id() {
                    map.entry(id.detach())
                        .or_default()
                        .push(format!("tag: {short}"));
                }
            }
        }
    }

    // Detached HEAD: emit a bare "HEAD" label if HEAD doesn't point to a branch.
    if head_branch.is_none()
        && let Some(oid) = head_id {
            map.entry(oid).or_default().insert(0, "HEAD".to_string());
        }

    map
}

/// Resolve the tip we should start the rev-walk from. `None` = HEAD;
/// otherwise interpret as a ref name / revspec.
fn resolve_tip(repo: &gix::Repository, branch: Option<&str>) -> Option<ObjectId> {
    match branch {
        None => repo.head_id().ok().map(|id| id.detach()),
        Some(name) => {
            // Try as a ref first (covers "main", "origin/main", "refs/heads/foo"),
            // then fall back to a full rev_parse for SHAs / advanced specs.
            if let Ok(Some(mut r)) = repo.try_find_reference(name)
                && let Ok(id) = r.peel_to_id() {
                    return Some(id.detach());
                }
            repo.rev_parse_single(name).ok().map(|id| id.detach())
        }
    }
}

/// Walk the commit history starting at `branch` (or HEAD), newest first,
/// returning up to `limit` entries with parent references for client-side
/// graph layout.
pub fn fetch_commit_log(path: &Path, limit: usize, branch: Option<&str>) -> Vec<CommitLogEntry> {
    if limit == 0 {
        return Vec::new();
    }
    let Some(repo) = crate::gix_helpers::open(path) else {
        return Vec::new();
    };
    let Some(tip) = resolve_tip(&repo, branch) else {
        return Vec::new();
    };

    let refs = collect_refs(&repo);

    let walk = match repo
        .rev_walk([tip])
        .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))
        .use_commit_graph(true)
        .all()
    {
        Ok(w) => w,
        Err(e) => {
            log::warn!("gix rev_walk failed: {e}");
            return Vec::new();
        }
    };

    let mut out = Vec::with_capacity(limit.min(256));
    for info in walk.take(limit) {
        let info = match info {
            Ok(i) => i,
            Err(e) => {
                log::warn!("gix rev_walk iteration failed: {e}");
                break;
            }
        };

        let id = info.id;
        let parents: Vec<String> = info.parent_ids.iter().map(short_hash).collect();
        let hash = short_hash(&id);

        let (message, author, timestamp) = match info.object() {
            Ok(commit) => {
                let msg = commit
                    .message()
                    .map(|m| m.title.to_string())
                    .unwrap_or_default();
                let author_name = commit
                    .author()
                    .map(|a| a.name.to_string())
                    .unwrap_or_default();
                let ts = commit.time().map(|t| t.seconds).unwrap_or(0);
                (msg, author_name, ts)
            }
            Err(e) => {
                log::warn!("gix object load failed for {hash}: {e}");
                (String::new(), String::new(), 0)
            }
        };

        let refs_for_commit = refs.get(&id).cloned().unwrap_or_default();

        out.push(CommitLogEntry {
            hash,
            parents,
            message,
            author,
            timestamp,
            refs: refs_for_commit,
        });
    }

    out
}
