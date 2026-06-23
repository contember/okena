//! Helpers for opening `gix` repositories. Centralizes discovery so each
//! call site doesn't need to think about the `ThreadSafeRepository` dance.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Cache of opened `ThreadSafeRepository` handles, keyed by the exact query
/// path passed to [`open`]. `ThreadSafeRepository::discover` walks the
/// directory tree upward and **re-parses the full git config** (system +
/// global + local) on every call — under the 5s status-poll loop that was the
/// single largest source of allocation churn (hundreds of MB/min through
/// `gix_config::parse`), which fragments the allocator and inflates RSS.
///
/// `ThreadSafeRepository` is `Send + Sync` and designed to be opened once and
/// shared; per-call we hand out a cheap `to_thread_local()` view. Status reads
/// the index and walks the worktree fresh each time, so a cached handle still
/// yields up-to-date status — only config is reused. A TTL bounds how long a
/// config change (e.g. a newly added remote) goes unnoticed and keeps the map
/// from pinning handles for repos that are no longer polled.
static REPO_CACHE: Mutex<Option<HashMap<PathBuf, (gix::ThreadSafeRepository, Instant)>>> =
    Mutex::new(None);

/// Re-discover (and re-parse config) at most this often per path.
const REPO_CACHE_TTL: Duration = Duration::from_secs(300);
/// Evict opportunistically above this many entries to bound memory.
const REPO_CACHE_MAX_ENTRIES: usize = 256;

/// Discover and open a `gix` repository starting from `path`, walking upward.
/// Returns `None` if no git repository is found or if opening fails for any
/// reason — mirrors the soft-fail semantics of the previous CLI-based callers.
///
/// Successful discoveries are cached (see [`REPO_CACHE`]); each call returns a
/// fresh thread-local view, so the result is indistinguishable from a direct
/// `discover().to_thread_local()` for callers.
pub(crate) fn open(path: &Path) -> Option<gix::Repository> {
    // Fast path: reuse a cached handle that's still within its TTL.
    {
        let guard = REPO_CACHE.lock();
        if let Some(cache) = guard.as_ref()
            && let Some((repo, ts)) = cache.get(path)
            && ts.elapsed() < REPO_CACHE_TTL
        {
            return Some(repo.to_thread_local());
        }
    }

    let repo = gix::ThreadSafeRepository::discover(path).ok()?;
    let local = repo.to_thread_local();

    {
        let mut guard = REPO_CACHE.lock();
        let cache = guard.get_or_insert_with(HashMap::new);
        cache.insert(path.to_path_buf(), (repo, Instant::now()));
        // Drop stale entries when the map grows; keeps unbounded project churn
        // (worktrees created/removed over a long session) from accumulating.
        if cache.len() > REPO_CACHE_MAX_ENTRIES {
            cache.retain(|_, (_, ts)| ts.elapsed() < REPO_CACHE_TTL);
        }
    }

    Some(local)
}

/// Cap a `gix` status walk to a single worker thread.
///
/// By default gix runs the index→worktree walk (directory walk + blob hashing)
/// across one thread per logical core. The status poller already fans out
/// across (nearly) every project in parallel, so that per-walk pool just churns
/// ~16 short-lived threads per walk — tens of thousands of thread spawns over a
/// session — for no throughput gain, while dominating CPU (the `gitoxide.in_par`
/// pool was the single largest cost in profiling). One thread per walk keeps
/// total concurrency bounded by the number of repos, not repos × cores.
pub(crate) fn single_threaded<'repo, P: gix::Progress>(
    platform: gix::status::Platform<'repo, P>,
) -> gix::status::Platform<'repo, P> {
    platform.index_worktree_options_mut(|opts| opts.thread_limit = Some(1))
}

/// List untracked files honoring `.gitignore`, with paths relative to
/// `query_path` (matches the previous `git -C path ls-files --others
/// --exclude-standard` behavior, including for monorepo subdirs).
///
/// Returns `None` on a transient failure (gix couldn't open the index, the
/// status walk init failed, or an iteration step errored). Callers that just
/// want a best-effort list can use `.unwrap_or_default()`; the polling hot
/// path uses `None` to keep the previous cached value instead of clobbering
/// it with a misleading empty list.
pub(crate) fn list_untracked_files(query_path: &Path) -> Option<Vec<String>> {
    let repo = open(query_path)?;
    let workdir = repo.workdir()?;

    // Compute the prefix from workdir to query_path. Empty when query_path
    // is the workdir itself.
    let canonical_query = query_path.canonicalize().unwrap_or_else(|_| query_path.to_path_buf());
    let canonical_workdir = workdir.canonicalize().unwrap_or_else(|_| workdir.to_path_buf());
    let prefix: String = canonical_query
        .strip_prefix(&canonical_workdir)
        .ok()
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            if s.is_empty() { String::new() } else { format!("{}/", s) }
        })
        .unwrap_or_default();

    let platform = match repo.status(gix::progress::Discard) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("gix status init failed for {}: {e}", query_path.display());
            return None;
        }
    };
    let iter = match single_threaded(platform)
        .untracked_files(gix::status::UntrackedFiles::Files)
        .into_iter(None)
    {
        Ok(i) => i,
        Err(e) => {
            log::warn!("gix status iter init failed for {}: {e}", query_path.display());
            return None;
        }
    };

    let mut result = Vec::new();
    for item_result in iter {
        let item = match item_result {
            Ok(i) => i,
            Err(e) => {
                log::warn!("gix status iteration failed for {}: {e}", query_path.display());
                return None;
            }
        };
        let gix::status::Item::IndexWorktree(
            gix::status::index_worktree::Item::DirectoryContents { entry, .. },
        ) = item
        else {
            continue;
        };
        if !matches!(entry.status, gix::dir::entry::Status::Untracked) {
            continue;
        }
        let rela = entry.rela_path.to_string();
        if prefix.is_empty() {
            result.push(rela);
        } else if let Some(stripped) = rela.strip_prefix(&prefix) {
            result.push(stripped.to_string());
        }
    }
    Some(result)
}
