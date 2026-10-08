//! Memo of commit-range counts across status polls.
//!
//! Ahead/behind and unpushed counts walk every commit between two tips. A
//! branch thousands of commits behind its base takes hundreds of milliseconds
//! per walk, and the poller asks again every few seconds although neither tip
//! moved. A commit id fixes its whole ancestry, so a count is walked once per
//! pair of tips and remembered; nothing needs invalidating.
//!
//! The ancestry is fixed only when the repository rewrites no parents: a
//! shallow boundary (moved by `fetch --deepen`/`--unshallow`) or a replace ref
//! changes the walk for the same ids. Such repositories are walked every time.
//! The key also names the object database, so a damaged clone cannot answer
//! for a healthy one.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use gix::ObjectId;
use parking_lot::Mutex;

/// Counts not looked up for this long are dropped.
const IDLE_TTL: Duration = Duration::from_secs(600);

/// Upper bound on remembered counts across all repositories.
const MAX_ENTRIES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RangeKey {
    /// Common git dir: linked worktrees of one repository share their counts.
    common_dir: PathBuf,
    hidden: ObjectId,
    tip: ObjectId,
}

struct Entry {
    count: usize,
    used: Instant,
}

type MemoTable = HashMap<RangeKey, Entry>;

static MEMO: LazyLock<Mutex<MemoTable>> = LazyLock::new(Default::default);

/// Commits reachable from `tip` but not from `hidden` — what
/// `git rev-list --count hidden..tip` prints. `None` when the walk cannot start.
pub(super) fn count_range(
    repo: &gix::Repository,
    hidden: ObjectId,
    tip: ObjectId,
) -> Option<usize> {
    if !ancestry_is_fixed(repo) {
        return walk(repo, hidden, tip).map(|walked| walked.count);
    }
    let key = RangeKey {
        common_dir: repo.common_dir().to_path_buf(),
        hidden,
        tip,
    };
    if let Some(count) = lookup(&mut MEMO.lock(), &key, Instant::now()) {
        return Some(count);
    }
    let walked = walk(repo, hidden, tip)?;
    // A walk that hit an unreadable commit (a pack swapped out by a concurrent
    // `gc`, say) undercounts; the next poll should walk again.
    if walked.complete {
        store_in(&mut MEMO.lock(), key, walked.count, Instant::now());
    }
    Some(walked.count)
}

fn ancestry_is_fixed(repo: &gix::Repository) -> bool {
    !repo.is_shallow() && repo.objects.store_ref().replacements().next().is_none()
}

struct Walked {
    count: usize,
    complete: bool,
}

fn walk(repo: &gix::Repository, hidden: ObjectId, tip: ObjectId) -> Option<Walked> {
    #[cfg(test)]
    note_walk(repo);
    let walk = repo.rev_walk([tip]).with_hidden([hidden]).all().ok()?;
    let mut walked = Walked {
        count: 0,
        complete: true,
    };
    for step in walk {
        match step {
            Ok(_) => walked.count += 1,
            Err(_) => walked.complete = false,
        }
    }
    Some(walked)
}

fn lookup(table: &mut MemoTable, key: &RangeKey, now: Instant) -> Option<usize> {
    let entry = table.get_mut(key)?;
    entry.used = now;
    Some(entry.count)
}

/// Remember a count, then drop idle entries and the least recently used ones
/// over [`MAX_ENTRIES`].
fn store_in(table: &mut MemoTable, key: RangeKey, count: usize, now: Instant) {
    table.retain(|_, entry| now.duration_since(entry.used) < IDLE_TTL);
    table.insert(key, Entry { count, used: now });
    while table.len() > MAX_ENTRIES {
        let Some(oldest) = table
            .iter()
            .min_by_key(|(_, entry)| entry.used)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        table.remove(&oldest);
    }
}

/// Walks run per common git dir, for tests only. Keyed by repository so tests
/// running in parallel do not see each other's walks.
#[cfg(test)]
static WALKS: LazyLock<Mutex<HashMap<PathBuf, usize>>> = LazyLock::new(Default::default);

#[cfg(test)]
fn note_walk(repo: &gix::Repository) {
    *WALKS
        .lock()
        .entry(repo.common_dir().to_path_buf())
        .or_default() += 1;
}

/// How many range walks ran for the repository containing `path`.
#[cfg(test)]
pub(super) fn walks(path: &std::path::Path) -> usize {
    let repo = crate::gix_helpers::open(path).expect("open test repository");
    WALKS
        .lock()
        .get(repo.common_dir())
        .copied()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: usize) -> RangeKey {
        let mut tip = [0u8; 20];
        tip[..8].copy_from_slice(&n.to_le_bytes());
        RangeKey {
            common_dir: PathBuf::from("/repo/.git"),
            hidden: ObjectId::null(gix::hash::Kind::Sha1),
            tip: ObjectId::from_bytes_or_panic(&tip),
        }
    }

    #[test]
    fn lookup_keeps_the_entry_it_hits_alive() {
        let mut table = MemoTable::new();
        let start = Instant::now();
        store_in(&mut table, key(0), 7, start);
        assert_eq!(lookup(&mut table, &key(0), start + IDLE_TTL / 2), Some(7));
        assert_eq!(lookup(&mut table, &key(1), start), None);

        store_in(&mut table, key(1), 1, start + IDLE_TTL);
        assert_eq!(lookup(&mut table, &key(0), start + IDLE_TTL), Some(7));
    }

    #[test]
    fn store_drops_idle_entries() {
        let mut table = MemoTable::new();
        let start = Instant::now();
        store_in(&mut table, key(0), 0, start);
        store_in(&mut table, key(1), 1, start + IDLE_TTL);
        assert!(!table.contains_key(&key(0)));
        assert!(table.contains_key(&key(1)));
    }

    #[test]
    fn store_evicts_the_least_recently_used_over_the_cap() {
        let mut table = MemoTable::new();
        let start = Instant::now();
        for n in 0..=MAX_ENTRIES {
            store_in(
                &mut table,
                key(n),
                n,
                start + Duration::from_millis(n as u64),
            );
        }
        assert_eq!(table.len(), MAX_ENTRIES);
        assert!(!table.contains_key(&key(0)));
        assert!(table.contains_key(&key(1)));
        assert!(table.contains_key(&key(MAX_ENTRIES)));
    }
}
