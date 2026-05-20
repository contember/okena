//! Compute railway lane layout from a commit DAG.
//!
//! Input: a topologically-ordered `Vec<CommitLogEntry>` (newest first), each
//! carrying parent short hashes. Output: one `LaneRow` per commit. Every row
//! is self-contained — its rails are split into an **upper half** (top edge →
//! vertical middle, where the dot sits) and a **lower half** (middle → bottom
//! edge). Merges slant inward in the upper half; forks slant outward in the
//! lower half; lanes that just pass through draw a full-height vertical. This
//! keeps rails continuous across rows (a lane leaves row *i* at column X's
//! bottom edge and enters row *i+1* at column X's top edge) without needing
//! separate connector rows.
//!
//! ## Algorithm
//!
//! `active: Vec<Option<Lane>>` tracks, per display column, which commit that
//! lane is waiting to draw next. Walking commits newest → oldest:
//!
//! 1. **Arriving lanes** are the columns whose `expected == commit.hash`. The
//!    leftmost wins the dot column; the rest are merges that die into it
//!    (their upper-half rail slants into the dot column).
//! 2. **No arriving lane** ⇒ the commit is a fresh tip; it takes the leftmost
//!    free column and has no upper rail (nothing flows in from above).
//! 3. The **first parent** continues the dot's lane in the same column (a
//!    straight vertical in the lower half), unless another live lane already
//!    awaits that parent — then the dot lane merges into it (lower-half slant).
//! 4. **Extra parents** (merges / octopus) spawn new lanes in free columns,
//!    each drawn as a lower-half fork slant out of the dot column.

use okena_git::CommitLogEntry;

pub type LaneId = u32;

/// Which half of a row a rail segment occupies. `Upper` spans the top edge to
/// the vertical middle (where the dot sits); `Lower` spans the middle to the
/// bottom edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    Upper,
    Lower,
}

/// One painted segment within a single row half. `from_col` is the column at
/// the half's *start* edge, `to_col` at its *end* edge (so an `Upper` rail
/// goes `from_col`@top → `to_col`@mid; a `Lower` rail goes `from_col`@mid →
/// `to_col`@bottom). Equal columns ⇒ a straight vertical.
#[derive(Debug, Clone, Copy)]
pub struct Rail {
    pub lane_id: LaneId,
    pub from_col: usize,
    pub to_col: usize,
    pub half: Half,
}

/// One commit row of the laid-out graph.
#[derive(Debug, Clone)]
pub struct LaneRow {
    pub rails: Vec<Rail>,
    pub dot: (usize, LaneId),
    pub entry: CommitLogEntry,
}

/// Output of [`compute`]. `max_col + 1` lanes worth of horizontal space.
#[derive(Debug, Clone)]
pub struct LaneLayout {
    pub rows: Vec<LaneRow>,
    pub max_col: usize,
}

#[derive(Clone, Debug)]
struct Lane {
    /// Short hash of the commit this lane is waiting to draw next.
    expected: String,
    id: LaneId,
}

/// Compute the lane layout. The input must be topologically ordered (newest
/// first) — i.e. when a commit appears, its parents appear *later*.
pub fn compute(commits: &[CommitLogEntry]) -> LaneLayout {
    let mut next_id: LaneId = 0;
    let mut active: Vec<Option<Lane>> = Vec::new();
    let mut rows: Vec<LaneRow> = Vec::with_capacity(commits.len());
    let mut max_col: usize = 0;

    let alloc = |next: &mut LaneId| -> LaneId {
        let id = *next;
        *next = next.wrapping_add(1);
        id
    };

    for commit in commits {
        // 1. Which columns are waiting for this commit?
        let arriving: Vec<usize> = active
            .iter()
            .enumerate()
            .filter_map(|(i, l)| match l {
                Some(l) if l.expected == commit.hash => Some(i),
                _ => None,
            })
            .collect();

        let (dot_col, dot_lane) = if let Some(&first) = arriving.first() {
            // `arriving` indices come from a filter_map that only keeps Some lanes.
            #[allow(clippy::expect_used)]
            (first, active[first].as_ref().expect("arriving lane is Some").id)
        } else {
            (leftmost_free(&active), alloc(&mut next_id))
        };

        // 2. Upper-half rails — drawn from the *current* lanes (before we
        //    install parents). A passing lane is vertical; an arriving lane
        //    slants into the dot column. A fresh tip has no upper rail
        //    because its column is still empty in `active`.
        let mut rails: Vec<Rail> = Vec::new();
        for (i, lane) in active.iter().enumerate() {
            if let Some(lane) = lane {
                let to = if arriving.contains(&i) { dot_col } else { i };
                rails.push(Rail {
                    lane_id: lane.id,
                    from_col: i,
                    to_col: to,
                    half: Half::Upper,
                });
            }
        }

        // 3. Build the outgoing lane set.
        let mut next_active = active.clone();
        for &i in &arriving {
            next_active[i] = None;
        }

        let mut cont_col: Option<usize> = None;
        let mut new_fork_cols: Vec<usize> = Vec::new();
        // (lane_id, target_col) for merges into a lane that already exists.
        let mut extra_slants: Vec<(LaneId, usize)> = Vec::new();

        if let Some(p0) = commit.parents.first() {
            // Does a *different* live lane already await the first parent?
            let existing = next_active.iter().enumerate().find_map(|(i, l)| {
                if i == dot_col {
                    return None;
                }
                match l {
                    Some(l) if l.expected == *p0 => Some(i),
                    _ => None,
                }
            });
            if let Some(ec) = existing {
                // Dot lane merges down into the existing lane.
                if dot_col < next_active.len() {
                    next_active[dot_col] = None;
                }
                cont_col = Some(ec);
            } else {
                ensure_slot(&mut next_active, dot_col);
                next_active[dot_col] = Some(Lane {
                    expected: p0.clone(),
                    id: dot_lane,
                });
                cont_col = Some(dot_col);
            }

            for parent in commit.parents.iter().skip(1) {
                if let Some(ec) = next_active.iter().position(|l| match l {
                    Some(l) => l.expected == *parent,
                    None => false,
                }) {
                    // `ec` is the index where `position` matched a Some lane.
                    #[allow(clippy::expect_used)]
                    let id = next_active[ec].as_ref().expect("position returned Some").id;
                    extra_slants.push((id, ec));
                } else {
                    let c = leftmost_free(&next_active);
                    let id = alloc(&mut next_id);
                    ensure_slot(&mut next_active, c);
                    next_active[c] = Some(Lane {
                        expected: parent.clone(),
                        id,
                    });
                    new_fork_cols.push(c);
                }
            }
        } else if dot_col < next_active.len() {
            // Root commit — the lane ends here.
            next_active[dot_col] = None;
        }

        while matches!(next_active.last(), Some(None)) {
            next_active.pop();
        }

        // 4. Lower-half rails. Passing/continuing lanes are vertical; freshly
        //    forked columns slant out of the dot column.
        for (i, lane) in next_active.iter().enumerate() {
            if let Some(lane) = lane {
                let from = if new_fork_cols.contains(&i) { dot_col } else { i };
                rails.push(Rail {
                    lane_id: lane.id,
                    from_col: from,
                    to_col: i,
                    half: Half::Lower,
                });
            }
        }
        // Dot lane merging into an existing lane: explicit slant.
        if let Some(cc) = cont_col
            && cc != dot_col {
                rails.push(Rail {
                    lane_id: dot_lane,
                    from_col: dot_col,
                    to_col: cc,
                    half: Half::Lower,
                });
            }
        // Merges into already-existing lanes (extra parents sharing a target).
        for (id, ec) in extra_slants {
            rails.push(Rail {
                lane_id: id,
                from_col: dot_col,
                to_col: ec,
                half: Half::Lower,
            });
        }

        max_col = max_col
            .max(active.len().saturating_sub(1))
            .max(next_active.len().saturating_sub(1))
            .max(dot_col);

        rows.push(LaneRow {
            rails,
            dot: (dot_col, dot_lane),
            entry: commit.clone(),
        });
        active = next_active;
    }

    LaneLayout { rows, max_col }
}

fn leftmost_free(active: &[Option<Lane>]) -> usize {
    active.iter().position(|s| s.is_none()).unwrap_or(active.len())
}

fn ensure_slot(active: &mut Vec<Option<Lane>>, col: usize) {
    while active.len() <= col {
        active.push(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(hash: &str, parents: &[&str]) -> CommitLogEntry {
        CommitLogEntry {
            hash: hash.to_string(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            message: format!("commit {hash}"),
            author: "Tester".to_string(),
            timestamp: 0,
            refs: vec![],
        }
    }

    fn dot_cols(layout: &LaneLayout) -> Vec<usize> {
        layout.rows.iter().map(|r| r.dot.0).collect()
    }

    fn has_rail(row: &LaneRow, from: usize, to: usize, half: Half) -> bool {
        row.rails
            .iter()
            .any(|r| r.from_col == from && r.to_col == to && r.half == half)
    }

    #[test]
    fn linear_history_uses_one_lane() {
        let commits = vec![entry("c", &["b"]), entry("b", &["a"]), entry("a", &[])];
        let layout = compute(&commits);
        assert_eq!(layout.max_col, 0);
        assert_eq!(dot_cols(&layout), vec![0, 0, 0]);
        // Newest commit is a tip — no upper rail.
        assert!(!layout.rows[0].rails.iter().any(|r| r.half == Half::Upper));
        // Middle commit has both a vertical in and a vertical out at col 0.
        assert!(has_rail(&layout.rows[1], 0, 0, Half::Upper));
        assert!(has_rail(&layout.rows[1], 0, 0, Half::Lower));
        // Root has an upper rail but no lower rail (lane ends).
        assert!(has_rail(&layout.rows[2], 0, 0, Half::Upper));
        assert!(!layout.rows[2].rails.iter().any(|r| r.half == Half::Lower));
    }

    #[test]
    fn dot_lane_has_a_through_vertical() {
        // Regression: the dot lane must paint a full vertical (upper + lower)
        // so the rail passes *through* the dot instead of leaving a gap.
        let commits = vec![entry("b", &["a"]), entry("a", &[])];
        let layout = compute(&commits);
        // 'b' is a tip: lower vertical at col 0 (continues to a), no upper.
        assert!(has_rail(&layout.rows[0], 0, 0, Half::Lower));
        // 'a' is reached by b: upper vertical at col 0 into the dot.
        assert!(has_rail(&layout.rows[1], 0, 0, Half::Upper));
    }

    #[test]
    fn simple_merge_two_lanes() {
        //   m   (merge of a, b)
        //   a b
        //   r
        let commits = vec![
            entry("m", &["a", "b"]),
            entry("a", &["r"]),
            entry("b", &["r"]),
            entry("r", &[]),
        ];
        let layout = compute(&commits);
        assert_eq!(dot_cols(&layout), vec![0, 0, 1, 0]);
        // m forks lane b out to col 1 in its lower half.
        assert!(has_rail(&layout.rows[0], 0, 1, Half::Lower));
        // b (col 1) merges into the existing r-lane at col 0 (lower slant).
        assert!(has_rail(&layout.rows[2], 1, 0, Half::Lower));
    }

    #[test]
    fn fork_then_continue_in_separate_lane() {
        let commits = vec![entry("f", &["a"]), entry("b", &["a"]), entry("a", &[])];
        let layout = compute(&commits);
        assert_eq!(dot_cols(&layout), vec![0, 1, 0]);
        // 'b' on col 1 merges into the existing a-lane at col 0.
        assert!(has_rail(&layout.rows[1], 1, 0, Half::Lower));
    }

    #[test]
    fn octopus_merge_three_parents() {
        let commits = vec![
            entry("m", &["a", "b", "c"]),
            entry("a", &["r"]),
            entry("b", &["r"]),
            entry("c", &["r"]),
            entry("r", &[]),
        ];
        let layout = compute(&commits);
        assert_eq!(layout.max_col, 2);
        let dots = dot_cols(&layout);
        assert_eq!(dots[0], 0);
        assert_eq!(*dots.last().unwrap(), 0);
        // m forks two extra lanes out to cols 1 and 2.
        assert!(has_rail(&layout.rows[0], 0, 1, Half::Lower));
        assert!(has_rail(&layout.rows[0], 0, 2, Half::Lower));
    }

    #[test]
    fn parent_outside_window_is_a_dangling_tip() {
        let commits = vec![entry("a", &["z"])];
        let layout = compute(&commits);
        assert_eq!(dot_cols(&layout), vec![0]);
        assert_eq!(layout.rows.len(), 1);
        // It expects an out-of-window parent, so it still draws a lower
        // vertical (the lane continues past the window edge).
        assert!(has_rail(&layout.rows[0], 0, 0, Half::Lower));
    }

    #[test]
    fn lane_stays_in_column_across_many_rows() {
        let commits = vec![
            entry("e", &["d"]),
            entry("d", &["c"]),
            entry("c", &["b"]),
            entry("b", &["a"]),
            entry("a", &[]),
        ];
        let layout = compute(&commits);
        assert!(dot_cols(&layout).iter().all(|&c| c == 0));
        // One continuous lane: every dot shares the same lane id.
        let ids: Vec<LaneId> = layout.rows.iter().map(|r| r.dot.1).collect();
        assert!(ids.windows(2).all(|w| w[0] == w[1]));
    }
}
