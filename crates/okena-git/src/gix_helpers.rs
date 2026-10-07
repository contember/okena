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

    let mut local = gix::ThreadSafeRepository::discover(path)
        .ok()?
        .to_thread_local();
    drop_long_running_filter_processes(&mut local);
    let repo = local.clone().into_sync();

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

/// Make `gix` use a filter driver's one-shot `clean`/`smudge` command instead
/// of its long-running `process` (e.g. `filter.lfs.process = git-lfs
/// filter-process`).
///
/// gix-filter only waits for a long-running filter when the caller invokes
/// `driver::State::shutdown`; its `State` has no `Drop`. A status walk builds
/// that state internally and drops it, so the filter exits on the closed pipe
/// and is left a zombie. Under the 5s poll in an LFS repo that is one zombie
/// per repo per poll, until the process limit is hit. The one-shot commands
/// are waited on after every file, so falling back to them leaks nothing.
///
/// Only a section that also defines `clean` loses its `process`. gix builds
/// one driver per `[filter "x"]` section and uses the first one that matches,
/// so the fallback has to live in the same section; and status only ever
/// cleans, so `smudge` alone is no fallback. Without one, removing `process`
/// would compare unfiltered content and report LFS pointers as modified.
/// Drivers defined by `process` alone are left as they are and still leak.
///
/// The key is removed from the in-memory config only, never written back.
/// Setting it to an empty value would not work: gix would try to spawn `""`
/// rather than fall back.
fn drop_long_running_filter_processes(repo: &mut gix::Repository) {
    let ids: Vec<_> = repo
        .config_snapshot()
        .plumbing()
        .sections_and_ids_by_name("filter")
        .into_iter()
        .flatten()
        .filter(|(section, _)| {
            section.header().subsection_name().is_some()
                && section.value("process").is_some()
                && section.value("clean").is_some()
        })
        .map(|(_, id)| id)
        .collect();
    if ids.is_empty() {
        return;
    }

    let mut config = repo.config_snapshot_mut();
    for id in ids {
        if let Some(mut section) = config.section_mut_by_id(id) {
            while section.remove("process").is_some() {}
        }
    }
    if let Err(err) = config.commit() {
        log::warn!("failed to drop long-running filter processes from gix config: {err}");
    }
}

/// How often one repo may have its racy index entries refreshed by git.
const RACY_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Last racy refresh per worktree, bounding it to one git run per interval.
static RACY_REFRESHED: Mutex<Option<HashMap<PathBuf, Instant>>> = Mutex::new(None);

/// Have git refresh the index when a finished status walk met racy entries.
///
/// An entry is racy when its file's mtime is not older than the index file
/// itself: stat cannot prove it unchanged, so every walk hashes its content,
/// through the clean filter (one `git-lfs clean` per LFS file). git clears
/// this by rewriting the index, which gives it a newer timestamp. gix status
/// never writes the index, so without this the same entries stay racy and get
/// re-filtered on every poll: a checkout that wrote files and index within
/// the same second cost ~150 `git-lfs clean` runs a minute.
///
/// git, not gix's `Outcome::write_changes`, because that writes back the copy
/// read before the walk and would undo a `git add` that landed meanwhile;
/// `update-index` re-reads under `index.lock`. The interval bounds entries a
/// rewrite cannot fix (an mtime in the future) to one git run per minute.
pub(crate) fn refresh_racy_index(workdir: &Path, outcome: Option<&gix::status::Outcome>) {
    if outcome.is_none_or(|outcome| outcome.index_worktree.tracked_file_modification.racy_clean == 0) {
        return;
    }
    {
        let mut guard = RACY_REFRESHED.lock();
        let refreshed = guard.get_or_insert_with(HashMap::new);
        if refreshed
            .get(workdir)
            .is_some_and(|at| at.elapsed() < RACY_REFRESH_INTERVAL)
        {
            return;
        }
        refreshed.retain(|_, at| at.elapsed() < RACY_REFRESH_INTERVAL);
        refreshed.insert(workdir.to_path_buf(), Instant::now());
    }
    // Exits non-zero when entries really changed, which is not a failure here.
    if let Err(err) = okena_core::process::safe_output_with_timeout(
        okena_core::process::command("git")
            .arg("-C")
            .arg(workdir)
            .args(["update-index", "-q", "--refresh"]),
        Duration::from_secs(30),
    ) {
        log::debug!("git update-index --refresh failed in {}: {err}", workdir.display());
    }
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

/// List untracked files honoring `.gitignore`, scoped to the subtree at
/// `query_path` but named the way git names them: **relative to the worktree
/// root**, so a monorepo subdir project's untracked paths share one base with
/// its tracked `git diff` paths.
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
    let canonical_query = query_path
        .canonicalize()
        .unwrap_or_else(|_| query_path.to_path_buf());
    let canonical_workdir = workdir
        .canonicalize()
        .unwrap_or_else(|_| workdir.to_path_buf());
    let prefix: String = canonical_query
        .strip_prefix(&canonical_workdir)
        .ok()
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            if s.is_empty() {
                String::new()
            } else {
                format!("{}/", s)
            }
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
            log::warn!(
                "gix status iter init failed for {}: {e}",
                query_path.display()
            );
            return None;
        }
    };

    let mut result = Vec::new();
    for item_result in iter {
        let item = match item_result {
            Ok(i) => i,
            Err(e) => {
                log::warn!(
                    "gix status iteration failed for {}: {e}",
                    query_path.display()
                );
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
        if prefix.is_empty() || rela.starts_with(&prefix) {
            result.push(rela);
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::{git_in, init_temp_repo};

    fn filter_value(repo: &gix::Repository, driver: &str, key: &str) -> Option<String> {
        repo.config_snapshot()
            .string(format!("filter.{driver}.{key}").as_str())
            .map(|v| v.to_string())
    }

    #[test]
    fn open_drops_process_for_a_driver_with_a_clean_fallback() {
        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["config", "filter.lfs.process", "git-lfs filter-process"]);
        git_in(&repo, &["config", "filter.lfs.clean", "git-lfs clean -- %f"]);

        let opened = open(&repo).expect("open repo");

        assert_eq!(filter_value(&opened, "lfs", "process"), None);
        assert_eq!(
            filter_value(&opened, "lfs", "clean").as_deref(),
            Some("git-lfs clean -- %f")
        );
    }

    #[test]
    fn open_keeps_process_for_a_driver_without_a_fallback() {
        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["config", "filter.only.process", "only-process"]);

        let opened = open(&repo).expect("open repo");

        assert_eq!(
            filter_value(&opened, "only", "process").as_deref(),
            Some("only-process")
        );
    }

    /// gix uses the first `[filter "x"]` section, so a `clean` in a later
    /// section is no fallback for the one holding `process`.
    #[test]
    fn open_keeps_process_when_clean_lives_in_another_section() {
        let (_tmp, repo) = init_temp_repo();
        let config = repo.join(".git").join("config");
        let mut contents = std::fs::read_to_string(&config).unwrap();
        contents.push_str(
            "[filter \"lfs\"]\n\tprocess = git-lfs filter-process\n\
             [filter \"lfs\"]\n\tclean = git-lfs clean -- %f\n",
        );
        std::fs::write(&config, contents).unwrap();

        let opened = open(&repo).expect("open repo");

        assert_eq!(
            filter_value(&opened, "lfs", "process").as_deref(),
            Some("git-lfs filter-process")
        );
    }

    /// Status only cleans, so `smudge` alone is no fallback.
    #[test]
    fn open_keeps_process_for_a_driver_with_only_smudge() {
        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["config", "filter.x.process", "x-filter"]);
        git_in(&repo, &["config", "filter.x.smudge", "x smudge"]);

        let opened = open(&repo).expect("open repo");

        assert_eq!(
            filter_value(&opened, "x", "process").as_deref(),
            Some("x-filter")
        );
    }

    /// Worktree paths whose content differs from the index, per gix status.
    fn changed_paths(repo: &Path) -> Vec<String> {
        let repo = open(repo).expect("open repo");
        single_threaded(repo.status(gix::progress::Discard).expect("status"))
            .untracked_files(gix::status::UntrackedFiles::None)
            .into_iter(None)
            .expect("status iter")
            .filter_map(|item| match item.expect("status item") {
                gix::status::Item::IndexWorktree(
                    gix::status::index_worktree::Item::Modification {
                        rela_path,
                        status: gix::status::plumbing::index_as_worktree::EntryStatus::Change(_),
                        ..
                    },
                ) => Some(rela_path.to_string()),
                _ => None,
            })
            .collect()
    }

    /// The status poll must not launch a long-running filter (gix never reaps
    /// it, so every poll would leave a zombie behind), yet must still filter
    /// through the one-shot `clean`.
    #[cfg(unix)]
    #[test]
    fn status_cleans_without_launching_a_long_running_filter() {
        let (_tmp, repo) = init_temp_repo();
        let data = repo.join("data.bin");
        let set_mtime = |ago: u64| {
            std::fs::File::options()
                .write(true)
                .open(&data)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(ago))
                .unwrap();
        };
        git_in(&repo, &["config", "filter.test.clean", "tr -d ' '"]);
        git_in(&repo, &["config", "filter.test.smudge", "cat"]);
        std::fs::write(repo.join(".gitattributes"), "*.bin filter=test\n").unwrap();
        std::fs::write(&data, "o ne\n").unwrap();
        set_mtime(120);
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "add filtered file"],
        );
        // Records the name of whoever launched it. git may legitimately run it
        // (the poll has git refresh racy index entries, and git reaps what it
        // starts); gix must not.
        let marker = repo.join(".git").join("process-launched-by");
        git_in(
            &repo,
            &[
                "config",
                "filter.test.process",
                &format!("ps -o comm= -p $PPID >> {}", marker.display()),
            ],
        );
        // Same size (gix calls a size change modified without filtering) and a
        // different, still older mtime: the stat check fails without the entry
        // being racy, so gix compares content. It cleans back to the committed
        // blob only if `clean` runs.
        std::fs::write(&data, "on e\n").unwrap();
        set_mtime(60);

        let _ = crate::repository::get_status(&repo);
        let _ = list_untracked_files(&repo);
        let changed = changed_paths(&repo);

        let launchers = std::fs::read_to_string(&marker).unwrap_or_default();
        let by_gix: Vec<&str> = launchers
            .lines()
            .filter(|comm| !comm.trim().ends_with("git"))
            .collect();
        assert!(
            by_gix.is_empty(),
            "status launched the long-running filter process: {by_gix:?}"
        );
        assert!(
            changed.is_empty(),
            "the clean filter did not run: {changed:?} reported as modified"
        );
    }

    /// A racy entry (file mtime not older than the index) is hashed through
    /// the clean filter on every walk until the index is rewritten. The poll
    /// has git rewrite it, so the next poll filters nothing.
    #[cfg(unix)]
    #[test]
    fn a_racy_entry_is_filtered_once_not_on_every_poll() {
        let (_tmp, repo) = init_temp_repo();
        let counter = repo.join(".git").join("clean-runs");
        git_in(
            &repo,
            &[
                "config",
                "filter.test.clean",
                &format!("echo x >> {}; cat", counter.display()),
            ],
        );
        std::fs::write(repo.join(".gitattributes"), "*.bin filter=test\n").unwrap();
        std::fs::write(repo.join("data.bin"), "data\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "add filtered file"],
        );
        // Date the index and the file to the same past instant: racy.
        let past = std::time::SystemTime::now() - Duration::from_secs(60);
        for path in [repo.join(".git").join("index"), repo.join("data.bin")] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(past)
                .unwrap();
        }
        let runs = || {
            std::fs::read_to_string(&counter)
                .map(|s| s.lines().count())
                .unwrap_or(0)
        };

        let _ = crate::repository::get_status(&repo);
        let after_first = runs();
        let _ = crate::repository::get_status(&repo);

        assert!(after_first > 0, "the racy entry was never filtered");
        assert_eq!(runs(), after_first, "the second poll filtered again");
    }
}
