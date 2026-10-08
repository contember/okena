//! Helpers for opening `gix` repositories. Centralizes discovery so each
//! call site doesn't need to think about the `ThreadSafeRepository` dance.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gix::bstr::BString;
use okena_core::process::{CommandSpec, Lane};
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

/// Drop `process` from each `[filter "x"]` section that also defines `clean`, in
/// memory only: gix never reaps a long-running filter, but waits on `clean`.
/// A `process`-only driver keeps it, or status would compare unfiltered content.
fn drop_long_running_filter_processes(repo: &mut gix::Repository) {
    let ids: Vec<_> = repo
        .config_snapshot()
        .plumbing()
        .sections_and_ids_by_name("filter")
        .into_iter()
        .flatten()
        // git itself prefers `process`, so a stale `clean`, or one with side
        // effects (git-annex's ingests the file), now runs on stat-dirty files.
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

/// How often one repo may have git refresh its index.
const INDEX_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

struct IndexRefresh {
    started: Instant,
    running: bool,
}

/// Index refreshes per worktree: at most one running, one started per interval.
static INDEX_REFRESHES: Mutex<Option<HashMap<PathBuf, IndexRefresh>>> = Mutex::new(None);

/// Have git rewrite the index in the background after a status walk read
/// unchanged entries in full: gix never writes it back, so every poll would run
/// their `clean` filter again. Repos with no filtered path are left alone.
pub(crate) fn refresh_filtered_index(
    repo: &gix::Repository,
    outcome: Option<&gix::status::Outcome>,
) {
    if outcome.is_none_or(|outcome| {
        outcome
            .index_worktree
            .tracked_file_modification
            .entries_to_update
            == 0
    }) {
        return;
    }
    let drivers = clean_filter_drivers(repo);
    if drivers.is_empty() {
        return;
    }
    let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
        return;
    };
    if !claim_index_refresh(&workdir) {
        return;
    }
    let repo = repo.clone();
    let spawned = std::thread::Builder::new()
        .name("okena-git-index-refresh".into())
        .spawn({
            let workdir = workdir.clone();
            move || {
                if index_uses_filter(&repo, &drivers) {
                    run_index_refresh(&workdir);
                }
                release_index_refresh(&workdir);
            }
        });
    if let Err(err) = spawned {
        log::warn!(
            "failed to spawn index refresh for {}: {err}",
            workdir.display()
        );
        release_index_refresh(&workdir);
    }
}

/// Names of the filter drivers that define `clean`.
fn clean_filter_drivers(repo: &gix::Repository) -> HashSet<BString> {
    repo.config_snapshot()
        .plumbing()
        .sections_by_name("filter")
        .into_iter()
        .flatten()
        .filter(|section| section.value("clean").is_some())
        .filter_map(|section| section.header().subsection_name().map(ToOwned::to_owned))
        .collect()
}

/// Whether the `filter` attribute of any index entry names one of `drivers`.
fn index_uses_filter(repo: &gix::Repository, drivers: &HashSet<BString>) -> bool {
    let Ok(index) = repo.index_or_empty() else {
        return false;
    };
    let Ok(mut attributes) = repo.attributes_only(
        &index,
        gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
    ) else {
        return false;
    };
    let mut matches = attributes.selected_attribute_matches(["filter"]);
    index.entries().iter().any(|entry| {
        attributes
            .at_entry(entry.path(&index), Some(entry.mode))
            .is_ok_and(|platform| platform.matching_attributes(&mut matches))
            && matches.iter_selected().any(|filter| {
                matches!(
                    filter.assignment.state,
                    gix::attrs::StateRef::Value(driver) if drivers.contains(driver.as_bstr())
                )
            })
    })
}

/// Claim a refresh unless one is running or started within the interval.
fn claim_index_refresh(workdir: &Path) -> bool {
    let mut guard = INDEX_REFRESHES.lock();
    let refreshes = guard.get_or_insert_with(HashMap::new);
    refreshes
        .retain(|_, refresh| refresh.running || refresh.started.elapsed() < INDEX_REFRESH_INTERVAL);
    if refreshes.contains_key(workdir) {
        return false;
    }
    refreshes.insert(
        workdir.to_path_buf(),
        IndexRefresh {
            started: Instant::now(),
            running: true,
        },
    );
    true
}

fn release_index_refresh(workdir: &Path) {
    if let Some(refresh) = INDEX_REFRESHES
        .lock()
        .as_mut()
        .and_then(|refreshes| refreshes.get_mut(workdir))
    {
        refresh.running = false;
    }
}

/// No timeout: the bus SIGKILLs a command that outlives one, and git removes
/// `index.lock` only on a signal it can catch, so a kill would leave it behind.
fn run_index_refresh(workdir: &Path) {
    let spec = CommandSpec::new("git")
        .args(["update-index", "-q", "--refresh"])
        .current_dir(workdir)
        .lane(Lane::Long)
        .label("index refresh");
    // Exits non-zero when entries really changed, which is not a failure here.
    if let Err(err) = okena_core::process::run(spec) {
        log::debug!(
            "git update-index --refresh failed in {}: {err}",
            workdir.display()
        );
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
    use std::time::SystemTime;

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
        git_in(
            &repo,
            &["config", "filter.lfs.process", "git-lfs filter-process"],
        );
        git_in(
            &repo,
            &["config", "filter.lfs.clean", "git-lfs clean -- %f"],
        );

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
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "add filtered file",
            ],
        );
        // Records the name of whoever launched it. git may legitimately run it
        // (the poll has git refresh the index, and git reaps what it starts);
        // gix must not.
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
        wait_for_index_refresh(&repo);
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

    fn set_mtime(path: &Path, at: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    /// Block until the background index refresh for `repo`, if any, has finished.
    fn wait_for_index_refresh(repo: &Path) {
        let workdir = open(repo)
            .expect("open repo")
            .workdir()
            .expect("workdir")
            .to_path_buf();
        let deadline = Instant::now() + Duration::from_secs(30);
        while INDEX_REFRESHES
            .lock()
            .as_ref()
            .and_then(|refreshes| refreshes.get(&workdir))
            .is_some_and(|refresh| refresh.running)
        {
            assert!(
                Instant::now() < deadline,
                "the index refresh did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A repo whose `*.bin` files go through a `clean` filter that logs each run
    /// to the returned file. Every tracked file is dated two minutes back, so
    /// none of them is racy against the index.
    fn repo_with_logging_clean_filter() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let (tmp, repo) = init_temp_repo();
        let log = repo.join(".git").join("clean-runs");
        git_in(
            &repo,
            &[
                "config",
                "filter.test.clean",
                &format!("echo x >> {}; cat", log.display()),
            ],
        );
        std::fs::write(repo.join(".gitattributes"), "*.bin filter=test\n").unwrap();
        std::fs::write(repo.join("data.bin"), "data\n").unwrap();
        let past = SystemTime::now() - Duration::from_secs(120);
        for name in ["file.txt", ".gitattributes", "data.bin"] {
            set_mtime(&repo.join(name), past);
        }
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "add filtered file",
            ],
        );
        (tmp, repo, log)
    }

    fn clean_runs(log: &Path) -> usize {
        std::fs::read_to_string(log)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    /// How a status walk of `repo` judged its tracked files.
    fn tracked_file_stats(repo: &Path) -> gix::status::plumbing::index_as_worktree::Outcome {
        let repo = open(repo).expect("open repo");
        let mut iter = single_threaded(repo.status(gix::progress::Discard).expect("status"))
            .untracked_files(gix::status::UntrackedFiles::None)
            .into_iter(None)
            .expect("status iter");
        for item in iter.by_ref() {
            item.expect("status item");
        }
        iter.outcome_mut()
            .expect("finished walk")
            .index_worktree
            .tracked_file_modification
            .clone()
    }

    /// Poll, let the index refresh it triggers finish, and poll again: only the
    /// first poll may run the clean filter.
    fn assert_only_the_first_poll_cleans(repo: &Path, log: &Path) {
        let _ = crate::repository::get_status(repo);
        wait_for_index_refresh(repo);
        let after_first = clean_runs(log);
        let _ = crate::repository::get_status(repo);
        wait_for_index_refresh(repo);

        assert!(
            after_first > 0,
            "the first poll did not run the clean filter"
        );
        assert_eq!(
            clean_runs(log),
            after_first,
            "the second poll ran the clean filter again"
        );
    }

    /// A touched file (new mtime, same content) is read through `clean` on every
    /// walk until the index records its new stat.
    #[cfg(unix)]
    #[test]
    fn a_touched_filtered_file_is_cleaned_by_one_poll_only() {
        let (_tmp, repo, log) = repo_with_logging_clean_filter();
        set_mtime(
            &repo.join("data.bin"),
            SystemTime::now() - Duration::from_secs(60),
        );
        let stats = tracked_file_stats(&repo);
        assert_eq!((stats.racy_clean, stats.entries_to_update), (0, 1));

        assert_only_the_first_poll_cleans(&repo, &log);
    }

    /// A racy entry (mtime not older than the index file) is read through
    /// `clean` on every walk until the index is rewritten.
    #[cfg(unix)]
    #[test]
    fn a_racy_filtered_entry_is_cleaned_by_one_poll_only() {
        let (_tmp, repo, log) = repo_with_logging_clean_filter();
        let recorded = std::fs::metadata(repo.join("data.bin"))
            .unwrap()
            .modified()
            .unwrap();
        set_mtime(&repo.join(".git").join("index"), recorded);
        let stats = tracked_file_stats(&repo);
        assert_eq!((stats.racy_clean, stats.entries_to_update), (3, 3));

        assert_only_the_first_poll_cleans(&repo, &log);
    }

    /// Without a path routed to a `clean` filter, the poll never writes the index.
    #[test]
    fn a_repo_without_filtered_paths_gets_no_index_refresh() {
        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["config", "filter.test.clean", "cat"]);
        set_mtime(
            &repo.join("file.txt"),
            SystemTime::now() - Duration::from_secs(60),
        );
        let index = repo.join(".git").join("index");
        let before = std::fs::metadata(&index).unwrap().modified().unwrap();

        let _ = crate::repository::get_status(&repo);
        wait_for_index_refresh(&repo);

        assert_eq!(
            std::fs::metadata(&index).unwrap().modified().unwrap(),
            before
        );
    }
}
