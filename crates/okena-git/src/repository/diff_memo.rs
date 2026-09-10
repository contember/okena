//! Per-file memo of diff counts across status walks.
//!
//! The poller re-walks every repository every few seconds and most changed
//! files are the same ones as last time. A file's counts are reused while
//! neither its HEAD blob nor its worktree stat moved since the previous walk.
//! Used by [`super::status::worktree_diff`] for tracked files and
//! [`super::status::untracked_line_count`] for untracked ones.

use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

use gix::bstr::{BStr, BString};
use parking_lot::Mutex;

/// Racy-write guard: an mtime this close to now may still be overwritten in the
/// same timestamp tick, so a same-size rewrite would be invisible to `stat`.
/// Must exceed the coarsest filesystem tick we run on (FAT rounds to 2 s).
const RACY_WINDOW: Duration = Duration::from_secs(2);

/// Memos of paths not walked for this long are dropped.
const IDLE_TTL: Duration = Duration::from_secs(600);

/// Upper bound on remembered files across all paths.
const MAX_ENTRIES: usize = 20_000;

/// What `stat` shows of a worktree path — every diff input besides the HEAD blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorktreeInput {
    File {
        len: u64,
        mtime: SystemTime,
        ctime: Option<SystemTime>,
        /// `(device, inode)`; zero on platforms that expose neither.
        identity: (u64, u64),
    },
    Missing,
}

/// A [`WorktreeInput`] plus whether it is old enough to be remembered.
#[derive(Clone, Copy, Debug)]
pub(super) struct Observed {
    pub input: WorktreeInput,
    pub trusted: bool,
}

/// Stat `path` for memoization. `None` when the stat failed for any reason
/// other than absence; such files are still diffed, just never remembered.
pub(super) fn observe(path: &Path, now: SystemTime) -> Option<Observed> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Some(Observed {
                input: WorktreeInput::Missing,
                trusted: true,
            });
        }
        Err(_) => return None,
    };
    let mtime = meta.modified().ok()?;
    let trusted = now
        .duration_since(mtime)
        .is_ok_and(|age| age >= RACY_WINDOW);
    Some(Observed {
        input: WorktreeInput::File {
            len: meta.len(),
            mtime,
            ctime: change_time(&meta),
            identity: identity(&meta),
        },
        trusted,
    })
}

/// Whether a read outcome is a pure function of the observed input and so safe
/// to remember: content read, undecodable content, or a confirmed absence.
/// Anything else may be a transient error that the next walk should retry.
pub(super) fn read_settled<T>(input: &WorktreeInput, read: &std::io::Result<T>) -> bool {
    use std::io::ErrorKind;
    match (input, read) {
        (WorktreeInput::File { .. }, Ok(_)) => true,
        (WorktreeInput::File { .. }, Err(e)) => e.kind() == ErrorKind::InvalidData,
        (WorktreeInput::Missing, Err(e)) => e.kind() == ErrorKind::NotFound,
        (WorktreeInput::Missing, Ok(_)) => false,
    }
}

#[cfg(unix)]
fn change_time(meta: &Metadata) -> Option<SystemTime> {
    use std::os::unix::fs::MetadataExt;
    let secs = u64::try_from(meta.ctime()).ok()?;
    let nanos = u32::try_from(meta.ctime_nsec()).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(secs, nanos))
}

#[cfg(not(unix))]
fn change_time(meta: &Metadata) -> Option<SystemTime> {
    meta.created().ok()
}

#[cfg(unix)]
fn identity(meta: &Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn identity(_meta: &Metadata) -> (u64, u64) {
    (0, 0)
}

/// HEAD-side and worktree-side inputs of one tracked path's diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TrackedInputs {
    /// Blob id at this path in HEAD; `None` when HEAD has no regular blob
    /// there (unborn HEAD, new file, submodule), which diffs as empty.
    pub head: Option<gix::ObjectId>,
    pub worktree: WorktreeInput,
}

struct TrackedEntry {
    inputs: TrackedInputs,
    counts: (usize, usize),
}

struct UntrackedEntry {
    input: WorktreeInput,
    lines: usize,
}

/// How many times the expensive path ran, for tests only.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Computed {
    pub tracked: usize,
    pub untracked: usize,
}

/// Remembered counts for one queried path.
#[derive(Default)]
pub(super) struct RepoDiffMemo {
    /// Keyed by repo-relative path, as reported by the status walk.
    tracked: HashMap<BString, TrackedEntry>,
    /// Keyed by the query-relative path listed in `WorktreeDiff::untracked`.
    untracked: HashMap<String, UntrackedEntry>,
    #[cfg(test)]
    pub computed: Computed,
}

impl RepoDiffMemo {
    /// Forget files that this walk no longer reports as changed or untracked.
    pub fn retain_walk(&mut self, changed: &HashSet<BString>, untracked: &[String]) {
        self.tracked.retain(|path, _| changed.contains(path));
        let keep: HashSet<&str> = untracked.iter().map(String::as_str).collect();
        self.untracked
            .retain(|path, _| keep.contains(path.as_str()));
    }

    pub fn tracked_counts(&self, path: &BStr, inputs: &TrackedInputs) -> Option<(usize, usize)> {
        let entry = self.tracked.get(path)?;
        (entry.inputs == *inputs).then_some(entry.counts)
    }

    pub fn remember_tracked(
        &mut self,
        path: BString,
        inputs: TrackedInputs,
        counts: (usize, usize),
    ) {
        self.tracked.insert(path, TrackedEntry { inputs, counts });
    }

    pub fn untracked_lines(&self, file: &str, input: &WorktreeInput) -> Option<usize> {
        let entry = self.untracked.get(file)?;
        (entry.input == *input).then_some(entry.lines)
    }

    pub fn remember_untracked(&mut self, file: String, input: WorktreeInput, lines: usize) {
        self.untracked.insert(file, UntrackedEntry { input, lines });
    }

    pub fn note_computed_tracked(&mut self) {
        #[cfg(test)]
        {
            self.computed.tracked += 1;
        }
    }

    pub fn note_computed_untracked(&mut self) {
        #[cfg(test)]
        {
            self.computed.untracked += 1;
        }
    }

    fn len(&self) -> usize {
        self.tracked.len() + self.untracked.len()
    }
}

type MemoTable = HashMap<PathBuf, (RepoDiffMemo, Instant)>;

static MEMOS: LazyLock<Mutex<MemoTable>> = LazyLock::new(Default::default);

/// Take the memo for `query` out of the table for the duration of a walk, so
/// the diff work runs without holding the lock. Pair with [`store`].
pub(super) fn take(query: &Path) -> RepoDiffMemo {
    MEMOS
        .lock()
        .remove(query)
        .map(|(memo, _)| memo)
        .unwrap_or_default()
}

/// Put a walked memo back, then drop idle paths and trim to [`MAX_ENTRIES`].
pub(super) fn store(query: &Path, memo: RepoDiffMemo) {
    let mut table = MEMOS.lock();
    store_in(&mut table, query, memo, Instant::now());
}

fn store_in(table: &mut MemoTable, query: &Path, memo: RepoDiffMemo, now: Instant) {
    table.retain(|_, (_, walked)| now.duration_since(*walked) < IDLE_TTL);
    table.insert(query.to_path_buf(), (memo, now));
    let mut total: usize = table.values().map(|(memo, _)| memo.len()).sum();
    while total > MAX_ENTRIES {
        let Some(oldest) = table
            .iter()
            .min_by_key(|(_, (_, walked))| *walked)
            .map(|(path, _)| path.clone())
        else {
            break;
        };
        if let Some((dropped, _)) = table.remove(&oldest) {
            total -= dropped.len();
        }
    }
}

/// Run `f` on the memo for `query`, if one exists. Untracked counting happens
/// after the walk stored its memo, so a missing memo means "nothing remembered".
pub(super) fn with<R>(query: &Path, f: impl FnOnce(&mut RepoDiffMemo) -> R) -> Option<R> {
    let mut table = MEMOS.lock();
    let (memo, _) = table.get_mut(query)?;
    Some(f(memo))
}

#[cfg(test)]
pub(super) fn computed(query: &Path) -> Computed {
    with(query, |memo| memo.computed).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memo_with_files(tracked: usize, untracked: usize) -> RepoDiffMemo {
        let mut memo = RepoDiffMemo::default();
        let inputs = TrackedInputs {
            head: None,
            worktree: WorktreeInput::Missing,
        };
        for i in 0..tracked {
            memo.remember_tracked(BString::from(format!("t{i}")), inputs, (0, 0));
        }
        for i in 0..untracked {
            memo.remember_untracked(format!("u{i}"), WorktreeInput::Missing, 0);
        }
        memo
    }

    #[test]
    fn retain_walk_keeps_only_files_the_walk_reported() {
        let mut memo = memo_with_files(3, 2);
        let changed: HashSet<BString> = [BString::from("t1")].into_iter().collect();
        memo.retain_walk(&changed, &["u0".to_string()]);
        let inputs = TrackedInputs {
            head: None,
            worktree: WorktreeInput::Missing,
        };
        assert_eq!(memo.tracked_counts(BStr::new("t1"), &inputs), Some((0, 0)));
        assert_eq!(memo.tracked_counts(BStr::new("t0"), &inputs), None);
        assert_eq!(memo.untracked_lines("u0", &WorktreeInput::Missing), Some(0));
        assert_eq!(memo.untracked_lines("u1", &WorktreeInput::Missing), None);
    }

    #[test]
    fn lookups_miss_when_inputs_differ() {
        let mut memo = RepoDiffMemo::default();
        let head = Some(gix::ObjectId::empty_blob(gix::hash::Kind::Sha1));
        let inputs = TrackedInputs {
            head,
            worktree: WorktreeInput::Missing,
        };
        memo.remember_tracked(BString::from("f"), inputs, (1, 2));
        assert_eq!(memo.tracked_counts(BStr::new("f"), &inputs), Some((1, 2)));
        let other_head = TrackedInputs {
            head: None,
            ..inputs
        };
        assert_eq!(memo.tracked_counts(BStr::new("f"), &other_head), None);
    }

    #[test]
    fn store_drops_idle_paths_and_trims_the_oldest_over_the_cap() {
        let mut table = MemoTable::new();
        let start = Instant::now();
        store_in(&mut table, Path::new("/idle"), memo_with_files(1, 0), start);
        store_in(
            &mut table,
            Path::new("/old"),
            memo_with_files(MAX_ENTRIES / 2, 0),
            start + IDLE_TTL / 2,
        );
        // `/idle` exceeded the TTL; `/old` plus this one exceed the cap.
        store_in(
            &mut table,
            Path::new("/new"),
            memo_with_files(MAX_ENTRIES / 2 + 1, 0),
            start + IDLE_TTL,
        );
        assert!(!table.contains_key(Path::new("/idle")));
        assert!(!table.contains_key(Path::new("/old")));
        assert!(table.contains_key(Path::new("/new")));

        // A single path over the cap on its own is dropped as well.
        store_in(
            &mut table,
            Path::new("/huge"),
            memo_with_files(MAX_ENTRIES + 1, 0),
            start + IDLE_TTL,
        );
        assert!(!table.contains_key(Path::new("/huge")));
    }

    #[test]
    fn read_settled_only_for_deterministic_outcomes() {
        use std::io::{Error, ErrorKind};
        let file = WorktreeInput::File {
            len: 0,
            mtime: SystemTime::UNIX_EPOCH,
            ctime: None,
            identity: (0, 0),
        };
        assert!(read_settled(&file, &Ok(())));
        assert!(read_settled::<()>(
            &file,
            &Err(Error::from(ErrorKind::InvalidData))
        ));
        assert!(!read_settled::<()>(
            &file,
            &Err(Error::from(ErrorKind::PermissionDenied))
        ));
        assert!(read_settled::<()>(
            &WorktreeInput::Missing,
            &Err(Error::from(ErrorKind::NotFound))
        ));
        assert!(!read_settled(&WorktreeInput::Missing, &Ok(())));
    }
}
