//! Tiered, activity-based ordering for the sidebar's `ProjectSortMode::Activity`
//! view.
//!
//! Pure ordering logic, decoupled from GPUI so it can be unit-tested. The
//! render path gathers one [`ActivityEntry`] per project (resolving live
//! terminal state from the registry) and calls [`order_by_activity`], which
//! splits projects into fixed tiers and sorts each tier.
//!
//! ## Tiers (top to bottom)
//!
//! 1. **Pinned** — projects the user pinned. Kept in their stable manual order
//!    (ascending `manual_index`), never reordered by activity, so they stay a
//!    fixed anchor for muscle memory. A pinned project stays pinned even if it
//!    also has attention/running state.
//! 2. **Attention** — non-pinned projects with an unseen bell/notification.
//!    These are what the user most needs to look at, so they sit just under the
//!    pins. Sorted by most-recent activity first.
//! 3. **Running** — non-pinned projects with a live foreground process but no
//!    unseen alert. Sorted by most-recent activity first.
//! 4. **Rest** — everything else, sorted by most-recent activity first.
//!
//! Within the three non-pinned tiers, ties (equal or both-`None`
//! `last_activity_at`) break by ascending `manual_index` so ordering is stable
//! and deterministic rather than hash-order noise.

/// One project's inputs to the activity ordering. Built per render from
/// persisted data (`pinned`, `last_activity_at`) plus live terminal state
/// aggregated over the project's terminals (`has_attention`, `is_running`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityEntry {
    pub id: String,
    /// Whether the project is pinned (top tier, stable order).
    pub pinned: bool,
    /// Stable manual position used for the pinned tier order and as the
    /// tiebreaker within non-pinned tiers. Lower sorts first.
    pub manual_index: usize,
    /// Persisted unix-millis of last meaningful activity; `None` = never.
    pub last_activity_at: Option<u64>,
    /// Any of the project's terminals has an unseen bell or OSC notification.
    pub has_attention: bool,
    /// Any of the project's terminals has a running foreground child process.
    pub is_running: bool,
}

/// Which tier a project landed in, for rendering section headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityTier {
    Pinned,
    Attention,
    Running,
    Rest,
}

impl ActivityTier {
    /// Human-readable section header label.
    pub fn label(self) -> &'static str {
        match self {
            ActivityTier::Pinned => "PINNED",
            ActivityTier::Attention => "NEEDS ATTENTION",
            ActivityTier::Running => "RUNNING",
            ActivityTier::Rest => "RECENT",
        }
    }
}

/// Order `entries` into the four tiers. Returns one `(tier, entry)` pair per
/// input entry, in final render order (pinned first, then attention, running,
/// rest). The caller renders a section header whenever the tier changes.
pub fn order_by_activity(mut entries: Vec<ActivityEntry>) -> Vec<(ActivityTier, ActivityEntry)> {
    // Stable manual order is the baseline; tier sorts refine it.
    entries.sort_by_key(|e| e.manual_index);

    let tier_of = |e: &ActivityEntry| -> ActivityTier {
        if e.pinned {
            ActivityTier::Pinned
        } else if e.has_attention {
            ActivityTier::Attention
        } else if e.is_running {
            ActivityTier::Running
        } else {
            ActivityTier::Rest
        }
    };

    let mut pinned = Vec::new();
    let mut attention = Vec::new();
    let mut running = Vec::new();
    let mut rest = Vec::new();
    for e in entries {
        match tier_of(&e) {
            ActivityTier::Pinned => pinned.push(e),
            ActivityTier::Attention => attention.push(e),
            ActivityTier::Running => running.push(e),
            ActivityTier::Rest => rest.push(e),
        }
    }

    // Most-recent activity first; `None` (never active) sorts last; ties break
    // by ascending manual_index (the Vec is already in manual order, so a
    // stable sort on the activity key preserves that for equal keys).
    let by_recency = |group: &mut Vec<ActivityEntry>| {
        group.sort_by(|a, b| {
            // Reverse so larger timestamp (more recent) comes first; `None`
            // becomes the smallest, landing at the end after reversal.
            b.last_activity_at.cmp(&a.last_activity_at)
        });
    };
    by_recency(&mut attention);
    by_recency(&mut running);
    by_recency(&mut rest);

    let mut out = Vec::new();
    out.extend(pinned.into_iter().map(|e| (ActivityTier::Pinned, e)));
    out.extend(attention.into_iter().map(|e| (ActivityTier::Attention, e)));
    out.extend(running.into_iter().map(|e| (ActivityTier::Running, e)));
    out.extend(rest.into_iter().map(|e| (ActivityTier::Rest, e)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, manual_index: usize) -> ActivityEntry {
        ActivityEntry {
            id: id.to_string(),
            pinned: false,
            manual_index,
            last_activity_at: None,
            has_attention: false,
            is_running: false,
        }
    }

    fn ids(ordered: &[(ActivityTier, ActivityEntry)]) -> Vec<String> {
        ordered.iter().map(|(_, e)| e.id.clone()).collect()
    }

    #[test]
    fn pinned_come_first_in_manual_order_regardless_of_activity() {
        // Two pinned projects keep their manual order even though the second
        // has more recent activity than the first. Pins are an anchor, not
        // activity-sorted.
        let mut a = entry("a", 0);
        a.pinned = true;
        a.last_activity_at = Some(10);
        let mut b = entry("b", 1);
        b.pinned = true;
        b.last_activity_at = Some(999);
        let out = order_by_activity(vec![b.clone(), a.clone()]);
        assert_eq!(ids(&out), vec!["a", "b"]);
        assert!(out.iter().all(|(t, _)| *t == ActivityTier::Pinned));
    }

    #[test]
    fn attention_outranks_running_outranks_rest() {
        let mut att = entry("att", 2);
        att.has_attention = true;
        let mut run = entry("run", 1);
        run.is_running = true;
        let rest = entry("rest", 0);
        let out = order_by_activity(vec![rest, run, att]);
        assert_eq!(ids(&out), vec!["att", "run", "rest"]);
        assert_eq!(
            out.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
            vec![ActivityTier::Attention, ActivityTier::Running, ActivityTier::Rest],
        );
    }

    #[test]
    fn pinned_wins_even_with_attention_elsewhere() {
        // A pinned project sits above a non-pinned attention project.
        let mut pinned = entry("p", 5);
        pinned.pinned = true;
        let mut att = entry("a", 0);
        att.has_attention = true;
        let out = order_by_activity(vec![att, pinned]);
        assert_eq!(ids(&out), vec!["p", "a"]);
    }

    #[test]
    fn within_tier_most_recent_activity_first() {
        let mut older = entry("older", 0);
        older.last_activity_at = Some(100);
        let mut newer = entry("newer", 1);
        newer.last_activity_at = Some(200);
        let never = entry("never", 2); // None -> last
        let out = order_by_activity(vec![older, never, newer]);
        assert_eq!(ids(&out), vec!["newer", "older", "never"]);
    }

    #[test]
    fn ties_break_by_manual_index() {
        // Equal timestamps -> stable ascending manual_index.
        let mut x = entry("x", 3);
        x.last_activity_at = Some(50);
        let mut y = entry("y", 1);
        y.last_activity_at = Some(50);
        let out = order_by_activity(vec![x, y]);
        assert_eq!(ids(&out), vec!["y", "x"]);
    }

    #[test]
    fn pinned_attention_running_rest_full_interleave() {
        let mut pin = entry("pin", 9);
        pin.pinned = true;
        let mut att1 = entry("att1", 0);
        att1.has_attention = true;
        att1.last_activity_at = Some(10);
        let mut att2 = entry("att2", 1);
        att2.has_attention = true;
        att2.last_activity_at = Some(20);
        let mut run = entry("run", 2);
        run.is_running = true;
        let mut rest = entry("rest", 3);
        rest.last_activity_at = Some(5);
        let out = order_by_activity(vec![rest, run, att1, att2, pin]);
        // pin (pinned) -> att2,att1 (attention by recency) -> run -> rest
        assert_eq!(ids(&out), vec!["pin", "att2", "att1", "run", "rest"]);
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(order_by_activity(vec![]).is_empty());
    }
}
