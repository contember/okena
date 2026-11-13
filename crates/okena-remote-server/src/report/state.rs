//! Which terminal reports to which, what each child last reported, and which
//! reports are due for delivery. Synchronous and clock-injected, so the rules
//! (turn detection, wait, rate limits, loops) are tested without a daemon.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use okena_core::agent_status::AgentLifecycle;
use okena_core::report::{
    ApiReportChild, ApiReportEvent, ReportDelivery, ReportEventKind, ReportProbeEntry,
};

use super::message::format_message;

/// Links in one daemon.
pub(super) const MAX_LINKS: usize = 256;
/// Children of one parent.
pub(super) const MAX_CHILDREN: usize = 32;
/// Parent → grandparent → … chain length.
pub(super) const MAX_DEPTH: usize = 8;
/// Between two deliveries of one child's reports; later ones are delivered
/// together when it has passed.
pub(super) const CHILD_COOLDOWN: Duration = Duration::from_secs(3);
/// Deliveries into one parent per [`RATE_WINDOW`].
pub(super) const PARENT_RATE: usize = 10;
pub(super) const RATE_WINDOW: Duration = Duration::from_secs(60);
/// After a wait returns, reports are held this long for the next wait, so a
/// caller looping over `okena wait` gets them there and not into its input.
pub(super) const WAIT_GRACE: Duration = Duration::from_secs(2);
/// Undelivered reports kept per child; older ones are dropped.
const MAX_PENDING: usize = 8;
/// Delivery attempts (busy parent) before a report is kept for `wait` only.
pub(super) const MAX_ATTEMPTS: u32 = 30;
/// A closed child's link stays this long, so `wait` can still return its
/// `closed` report.
pub(super) const GONE_RETENTION: Duration = Duration::from_secs(300);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum LinkError {
    SelfLink,
    Cycle,
    TooDeep,
    TooMany,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SelfLink => "a terminal cannot report to itself",
            Self::Cycle => "the parent already reports to this terminal (directly or through others)",
            Self::TooDeep => "too many levels of parents",
            Self::TooMany => "too many children",
        })
    }
}

struct Link {
    parent: String,
    owned: bool,
    name: Option<String>,
    project_id: String,
    /// Agent lifecycle and its change counter at the last poll.
    lifecycle: Option<AgentLifecycle>,
    seen_changes: Option<u64>,
    custom: Option<String>,
    /// A turn is under way: the agent was seen working (or blocked) since its
    /// last done/error. Only an armed agent reports its turn's end.
    armed: bool,
    /// The child reported explicitly during this turn, so the turn's own end
    /// adds nothing.
    explicit_this_turn: bool,
    /// Started or steered since its last report.
    running: bool,
    last_event: Option<ApiReportEvent>,
    /// `last_event` was returned by a wait.
    consumed: bool,
    /// Reports waiting for delivery into the parent, oldest first.
    pending: VecDeque<ApiReportEvent>,
    attempts: u32,
    last_delivery: Option<Instant>,
    gone_at: Option<Instant>,
}

#[derive(Default)]
struct ParentState {
    /// Waits in progress.
    waiting: u32,
    /// Hold reports until then (see [`WAIT_GRACE`]).
    hold_until: Option<Instant>,
    deliveries: VecDeque<Instant>,
}

/// A report for one parent, from one or more children, ready to deliver.
#[derive(Debug)]
pub(super) struct Due {
    pub parent: String,
    pub events: Vec<ApiReportEvent>,
}

/// What a poll found: turn ends to report, children that went away, and the
/// owned children of parents that went away.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Observed {
    pub turns: Vec<(String, ReportEventKind)>,
    pub closed: Vec<String>,
    /// `(child, project)` to close.
    pub orphans: Vec<(String, String)>,
}

#[derive(Default)]
pub(super) struct Links {
    links: HashMap<String, Link>,
    parents: HashMap<String, ParentState>,
    next_seq: u64,
}

impl Links {
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    pub fn is_linked(&self, child: &str) -> bool {
        self.links
            .get(child)
            .is_some_and(|link| link.gone_at.is_none())
    }

    /// Every child and parent, for the poll.
    pub fn terminal_ids(&self) -> Vec<String> {
        let mut ids: HashSet<&str> = HashSet::new();
        for (child, link) in &self.links {
            ids.insert(child);
            ids.insert(&link.parent);
        }
        ids.into_iter().map(str::to_string).collect()
    }

    /// Report `child` to `parent` from now on. The child's current agent
    /// status (`entry`) is the baseline, so a turn that already ended is not
    /// reported.
    pub fn link(
        &mut self,
        child: &str,
        parent: &str,
        owned: bool,
        entry: &ReportProbeEntry,
    ) -> Result<(), LinkError> {
        if child == parent {
            return Err(LinkError::SelfLink);
        }
        let mut depth = 1;
        let mut up = parent;
        while let Some(link) = self.links.get(up).filter(|link| link.gone_at.is_none()) {
            if link.parent == child {
                return Err(LinkError::Cycle);
            }
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(LinkError::TooDeep);
            }
            up = &link.parent;
        }
        let relink = self.links.contains_key(child);
        if !relink && self.links.len() >= MAX_LINKS {
            return Err(LinkError::TooMany);
        }
        let siblings = self
            .links
            .iter()
            .filter(|(id, link)| link.parent == parent && id.as_str() != child)
            .count();
        if siblings >= MAX_CHILDREN {
            return Err(LinkError::TooMany);
        }
        self.links.insert(
            child.to_string(),
            Link {
                parent: parent.to_string(),
                owned,
                name: entry.name.clone(),
                project_id: entry.project_id.clone(),
                lifecycle: entry.lifecycle,
                seen_changes: Some(entry.lifecycle_changes),
                custom: entry.custom.clone(),
                armed: entry.lifecycle == Some(AgentLifecycle::Working),
                explicit_this_turn: false,
                running: entry.lifecycle == Some(AgentLifecycle::Working),
                last_event: None,
                consumed: false,
                pending: VecDeque::new(),
                attempts: 0,
                last_delivery: None,
                gone_at: None,
            },
        );
        Ok(())
    }

    pub fn unlink(&mut self, child: &str) -> bool {
        self.links.remove(child).is_some()
    }

    /// The parent's input (or a wait) gets this child's reports.
    pub fn parent_of(&self, child: &str) -> Option<&str> {
        self.links.get(child).map(|link| link.parent.as_str())
    }

    pub fn custom_of(&self, child: &str) -> Option<&str> {
        self.links.get(child).and_then(|link| link.custom.as_deref())
    }

    /// Something typed into the child: it runs again, and its next report is
    /// awaited.
    pub fn steer(&mut self, child: &str) {
        if let Some(link) = self.links.get_mut(child).filter(|link| link.gone_at.is_none()) {
            link.running = true;
        }
    }

    /// Apply a poll. `entries` are the terminals that still exist.
    pub fn observe(&mut self, entries: &[ReportProbeEntry], now: Instant) -> Observed {
        let present: HashMap<&str, &ReportProbeEntry> = entries
            .iter()
            .map(|entry| (entry.terminal_id.as_str(), entry))
            .collect();
        let mut observed = Observed::default();

        // Parents that went away take their links along, and their owned
        // children with them.
        let orphaned: Vec<String> = self
            .links
            .iter()
            .filter(|(_, link)| !present.contains_key(link.parent.as_str()))
            .map(|(child, _)| child.clone())
            .collect();
        for child in orphaned {
            let link = self.links.remove(&child).expect("collected above");
            if link.owned && link.gone_at.is_none() && present.contains_key(child.as_str()) {
                observed.orphans.push((child, link.project_id));
            }
        }
        self.parents
            .retain(|parent, _| present.contains_key(parent.as_str()));

        self.links.retain(|_, link| {
            link.gone_at
                .is_none_or(|gone_at| now.duration_since(gone_at) < GONE_RETENTION)
        });

        for (child, link) in &mut self.links {
            if link.gone_at.is_some() {
                continue;
            }
            let Some(entry) = present.get(child.as_str()) else {
                link.gone_at = Some(now);
                observed.closed.push(child.clone());
                continue;
            };
            link.name = entry.name.clone();
            link.project_id = entry.project_id.clone();
            link.custom = entry.custom.clone();
            if let Some(kind) = link.observe_lifecycle(entry) {
                observed.turns.push((child.clone(), kind));
            }
        }
        observed.turns.sort_by(|a, b| a.0.cmp(&b.0));
        observed.closed.sort();
        observed
    }

    /// Record a report of `child`: its last event, due for delivery. `None`
    /// for a child without a link.
    pub fn record(
        &mut self,
        child: &str,
        event: ReportEventKind,
        exit_code: Option<i32>,
        body: String,
        explicit: bool,
        at_ms: u64,
    ) -> Option<ApiReportEvent> {
        let link = self.links.get_mut(child)?;
        self.next_seq += 1;
        let report = ApiReportEvent {
            seq: self.next_seq,
            child_terminal_id: child.to_string(),
            child_name: link.name.clone(),
            event,
            exit_code,
            message: format_message(child, link.name.as_deref(), event, exit_code, &body),
            body,
            at_ms,
            delivery: None,
        };
        if explicit && link.armed {
            link.explicit_this_turn = true;
        }
        link.running = false;
        link.consumed = false;
        link.last_event = Some(report.clone());
        if link.pending.len() >= MAX_PENDING {
            link.pending.pop_front();
        }
        link.pending.push_back(report.clone());
        Some(report)
    }

    /// A wait of `parent` for `children` (empty: all of its children) starts.
    pub fn begin_wait(&mut self, parent: &str) {
        self.parents.entry(parent.to_string()).or_default().waiting += 1;
    }

    pub fn end_wait(&mut self, parent: &str, now: Instant) {
        if let Some(state) = self.parents.get_mut(parent) {
            state.waiting = state.waiting.saturating_sub(1);
            state.hold_until = Some(now + WAIT_GRACE);
        }
    }

    /// The children a wait of `parent` covers: `children`, or all of its own.
    /// An id that is not the parent's child is an error.
    pub fn awaited(&self, parent: &str, children: &[String]) -> Result<Vec<String>, String> {
        if children.is_empty() {
            let mut own: Vec<String> = self
                .links
                .iter()
                .filter(|(_, link)| link.parent == parent)
                .map(|(child, _)| child.clone())
                .collect();
            own.sort();
            if own.is_empty() {
                return Err("this terminal has no children".to_string());
            }
            return Ok(own);
        }
        for child in children {
            if self.parent_of(child) != Some(parent) {
                return Err(format!("{child} is not a child of this terminal"));
            }
        }
        Ok(children.to_vec())
    }

    /// The reports a wait returns now: with `all`, once each awaited child has
    /// an unreturned report and is not running again; else as soon as one
    /// does. Returned reports are no longer delivered into the parent.
    /// `Err` lists the children still awaited.
    pub fn take_for_wait(
        &mut self,
        awaited: &[String],
        all: bool,
    ) -> Result<Vec<ApiReportEvent>, Vec<String>> {
        let ready = |link: &Link| !link.running && !link.consumed && link.last_event.is_some();
        let (done, waiting): (Vec<&String>, Vec<&String>) = awaited
            .iter()
            .partition(|child| self.links.get(child.as_str()).is_some_and(ready));
        if done.is_empty() || (all && !waiting.is_empty()) {
            return Err(waiting.into_iter().cloned().collect());
        }
        let mut events = Vec::new();
        for child in done {
            let link = self.links.get_mut(child.as_str()).expect("partitioned above");
            link.consumed = true;
            let mut event = link.last_event.clone().expect("ready");
            let undelivered = link.pending.iter().any(|pending| pending.seq == event.seq);
            link.pending.clear();
            link.attempts = 0;
            if undelivered || event.delivery == Some(ReportDelivery::Kept) {
                event.delivery = Some(ReportDelivery::Wait);
                link.last_event = Some(event.clone());
            }
            events.push(event);
        }
        events.sort_by_key(|event| event.seq);
        Ok(events)
    }

    /// Take the reports due for delivery: not while their parent waits or
    /// just waited, at most one delivery per child per [`CHILD_COOLDOWN`] and
    /// [`PARENT_RATE`] per parent per [`RATE_WINDOW`].
    pub fn take_due(&mut self, now: Instant) -> Vec<Due> {
        let mut by_parent: HashMap<String, Vec<String>> = HashMap::new();
        for (child, link) in &self.links {
            let cooled = link
                .last_delivery
                .is_none_or(|at| now.duration_since(at) >= CHILD_COOLDOWN);
            if !link.pending.is_empty() && cooled {
                by_parent.entry(link.parent.clone()).or_default().push(child.clone());
            }
        }
        let mut due = Vec::new();
        for (parent, mut children) in by_parent {
            let state = self.parents.entry(parent.clone()).or_default();
            if state.waiting > 0 || state.hold_until.is_some_and(|until| now < until) {
                continue;
            }
            while state
                .deliveries
                .front()
                .is_some_and(|at| now.duration_since(*at) >= RATE_WINDOW)
            {
                state.deliveries.pop_front();
            }
            if state.deliveries.len() >= PARENT_RATE {
                continue;
            }
            state.deliveries.push_back(now);
            children.sort();
            let mut events = Vec::new();
            for child in children {
                let link = self.links.get_mut(&child).expect("collected above");
                link.last_delivery = Some(now);
                events.extend(link.pending.drain(..));
            }
            events.sort_by_key(|event| event.seq);
            due.push(Due { parent, events });
        }
        due.sort_by(|a, b| a.parent.cmp(&b.parent));
        due
    }

    /// Note how `events` were delivered.
    pub fn delivered(&mut self, events: &[ApiReportEvent], delivery: ReportDelivery) {
        for event in events {
            if let Some(link) = self.links.get_mut(&event.child_terminal_id) {
                link.attempts = 0;
                if let Some(last) = link.last_event.as_mut().filter(|last| last.seq == event.seq) {
                    last.delivery = Some(delivery);
                }
            }
        }
    }

    /// The parent could not take `events` now (a busy Claude): try again
    /// later, or keep them for a wait after [`MAX_ATTEMPTS`].
    pub fn retry(&mut self, events: Vec<ApiReportEvent>) {
        let mut kept = Vec::new();
        for event in events.into_iter().rev() {
            let Some(link) = self.links.get_mut(&event.child_terminal_id) else {
                continue;
            };
            if link.attempts >= MAX_ATTEMPTS {
                kept.push(event);
                continue;
            }
            link.attempts += 1;
            // Newer reports recorded meanwhile stay behind these.
            link.pending.push_front(event);
        }
        self.delivered(&kept, ReportDelivery::Kept);
    }

    /// `parent`'s children (all children when `None`), by id.
    pub fn children(&self, parent: Option<&str>) -> Vec<ApiReportChild> {
        let mut children: Vec<ApiReportChild> = self
            .links
            .iter()
            .filter(|(_, link)| parent.is_none_or(|parent| link.parent == parent))
            .map(|(child, link)| ApiReportChild {
                terminal_id: child.clone(),
                name: link.name.clone(),
                parent_terminal_id: link.parent.clone(),
                owned: link.owned,
                running: link.running,
                last_event: link.last_event.clone(),
            })
            .collect();
        children.sort_by(|a, b| a.terminal_id.cmp(&b.terminal_id));
        children
    }
}

impl Link {
    /// Follow the agent status: a turn that ends (`done`, `idle`, `error`)
    /// after the agent was seen working is reported once, and so is every
    /// `blocked`. Two or more lifecycle changes between polls that end in
    /// `done` count as a whole turn seen in passing.
    fn observe_lifecycle(&mut self, entry: &ReportProbeEntry) -> Option<ReportEventKind> {
        let changes = entry
            .lifecycle_changes
            .saturating_sub(self.seen_changes.unwrap_or(entry.lifecycle_changes));
        self.seen_changes = Some(entry.lifecycle_changes);
        self.lifecycle = entry.lifecycle;
        if changes == 0 {
            return None;
        }
        match entry.lifecycle? {
            AgentLifecycle::Working => {
                if !self.armed {
                    self.armed = true;
                    self.explicit_this_turn = false;
                }
                self.running = true;
                None
            }
            AgentLifecycle::Blocked => {
                self.armed = true;
                Some(ReportEventKind::Blocked)
            }
            lifecycle @ (AgentLifecycle::Done | AgentLifecycle::Idle | AgentLifecycle::Error) => {
                let turn_seen = self.armed || changes >= 2;
                let explicit = std::mem::take(&mut self.explicit_this_turn);
                self.armed = false;
                if !turn_seen || explicit {
                    if explicit {
                        self.running = false;
                    }
                    return None;
                }
                Some(if lifecycle == AgentLifecycle::Error {
                    ReportEventKind::Error
                } else {
                    ReportEventKind::Done
                })
            }
            AgentLifecycle::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> ReportProbeEntry {
        ReportProbeEntry {
            terminal_id: id.to_string(),
            project_id: "p".to_string(),
            name: None,
            lifecycle: None,
            custom: None,
            lifecycle_changes: 0,
            screen: None,
        }
    }

    fn agent(id: &str, lifecycle: AgentLifecycle, changes: u64) -> ReportProbeEntry {
        ReportProbeEntry {
            lifecycle: Some(lifecycle),
            lifecycle_changes: changes,
            ..entry(id)
        }
    }

    fn linked(child: &str, parent: &str) -> Links {
        let mut links = Links::default();
        links.link(child, parent, false, &entry(child)).unwrap();
        links
    }

    fn report(links: &mut Links, child: &str, kind: ReportEventKind) -> ApiReportEvent {
        links
            .record(child, kind, None, "body".to_string(), false, 0)
            .expect("linked")
    }

    #[test]
    fn links_refuse_self_cycles_and_deep_chains() {
        let mut links = Links::default();
        assert_eq!(links.link("a", "a", false, &entry("a")), Err(LinkError::SelfLink));
        links.link("b", "a", false, &entry("b")).unwrap();
        links.link("c", "b", false, &entry("c")).unwrap();
        assert_eq!(links.link("a", "c", false, &entry("a")), Err(LinkError::Cycle));
        let mut chain = Links::default();
        for level in 1..=MAX_DEPTH {
            let (child, parent) = (format!("t{level}"), format!("t{}", level - 1));
            chain.link(&child, &parent, false, &entry(&child)).unwrap();
        }
        let last = format!("t{}", MAX_DEPTH + 1);
        assert_eq!(
            chain.link(&last, &format!("t{MAX_DEPTH}"), false, &entry(&last)),
            Err(LinkError::TooDeep)
        );
    }

    #[test]
    fn a_turn_is_reported_once_it_ends_and_blocked_every_time() {
        let mut links = linked("c", "p");
        let now = Instant::now();
        let poll = |links: &mut Links, lifecycle, changes| {
            links
                .observe(&[entry("p"), agent("c", lifecycle, changes)], now)
                .turns
        };
        // An idle agent at link time reports nothing until it works.
        assert!(poll(&mut links, AgentLifecycle::Idle, 1).is_empty());
        assert!(poll(&mut links, AgentLifecycle::Working, 2).is_empty());
        assert!(links.children(Some("p"))[0].running);
        assert_eq!(
            poll(&mut links, AgentLifecycle::Blocked, 3),
            vec![("c".to_string(), ReportEventKind::Blocked)]
        );
        assert!(poll(&mut links, AgentLifecycle::Working, 4).is_empty());
        assert_eq!(
            poll(&mut links, AgentLifecycle::Done, 5),
            vec![("c".to_string(), ReportEventKind::Done)]
        );
        // The same status polled again, and `done` turning `idle`, are no news.
        assert!(poll(&mut links, AgentLifecycle::Done, 5).is_empty());
        assert!(poll(&mut links, AgentLifecycle::Idle, 6).is_empty());
        // A whole turn between two polls still counts; an error says so.
        assert_eq!(
            poll(&mut links, AgentLifecycle::Error, 8),
            vec![("c".to_string(), ReportEventKind::Error)]
        );
    }

    #[test]
    fn an_explicit_report_replaces_the_end_of_its_turn() {
        let mut links = linked("c", "p");
        let now = Instant::now();
        links.observe(&[entry("p"), agent("c", AgentLifecycle::Working, 1)], now);
        links.record("c", ReportEventKind::Report, None, "PR #3".into(), true, 0);
        let observed = links.observe(&[entry("p"), agent("c", AgentLifecycle::Done, 2)], now);
        assert!(observed.turns.is_empty());
        // The next turn reports its end again.
        links.observe(&[entry("p"), agent("c", AgentLifecycle::Working, 3)], now);
        let observed = links.observe(&[entry("p"), agent("c", AgentLifecycle::Done, 4)], now);
        assert_eq!(observed.turns, vec![("c".to_string(), ReportEventKind::Done)]);
    }

    #[test]
    fn closed_children_and_orphans() {
        let mut links = Links::default();
        links.link("kept", "p", false, &entry("kept")).unwrap();
        links.link("owned", "p", true, &entry("owned")).unwrap();
        links.link("gone", "q", false, &entry("gone")).unwrap();
        let now = Instant::now();
        let observed = links.observe(&[entry("q"), entry("kept"), entry("owned")], now);
        assert_eq!(observed.closed, vec!["gone".to_string()]);
        assert_eq!(observed.orphans, vec![("owned".to_string(), "p".to_string())]);
        assert!(!links.is_linked("kept") && !links.is_linked("owned"));
        // A closed child's link lingers for a wait, then goes.
        assert!(!links.is_linked("gone"));
        assert_eq!(links.parent_of("gone"), Some("q"));
        links.observe(&[entry("q")], now + GONE_RETENTION);
        assert_eq!(links.parent_of("gone"), None);
    }

    #[test]
    fn a_wait_takes_reports_before_they_are_delivered() {
        let mut links = linked("a", "p");
        links.link("b", "p", false, &entry("b")).unwrap();
        let now = Instant::now();
        let awaited = links.awaited("p", &[]).unwrap();
        assert_eq!(awaited, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(links.awaited("q", &[]).unwrap_err(), "this terminal has no children");
        assert!(links.awaited("q", &["a".to_string()]).is_err());

        links.begin_wait("p");
        assert_eq!(links.take_for_wait(&awaited, false), Err(awaited.clone()));
        report(&mut links, "a", ReportEventKind::Exit);
        // Held while the parent waits.
        assert!(links.take_due(now).is_empty());
        let events = links.take_for_wait(&awaited, false).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].delivery, Some(ReportDelivery::Wait));
        // Returned once only, and no longer due.
        assert!(links.take_for_wait(&awaited, false).is_err());
        links.end_wait("p", now);
        assert!(links.take_due(now + WAIT_GRACE).is_empty());

        // `all` waits for every child.
        links.begin_wait("p");
        report(&mut links, "a", ReportEventKind::Done);
        assert_eq!(links.take_for_wait(&awaited, true), Err(vec!["b".to_string()]));
        report(&mut links, "b", ReportEventKind::Done);
        assert_eq!(links.take_for_wait(&awaited, true).unwrap().len(), 2);
    }

    #[test]
    fn a_steered_child_is_awaited_again() {
        let mut links = linked("a", "p");
        report(&mut links, "a", ReportEventKind::Done);
        links.steer("a");
        let awaited = vec!["a".to_string()];
        assert!(links.take_for_wait(&awaited, false).is_err());
    }

    #[test]
    fn deliveries_are_held_after_a_wait_cooled_per_child_and_capped_per_parent() {
        let mut links = linked("a", "p");
        let now = Instant::now();
        links.begin_wait("p");
        links.end_wait("p", now);
        report(&mut links, "a", ReportEventKind::Report);
        assert!(links.take_due(now).is_empty());
        let due = links.take_due(now + WAIT_GRACE);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].parent, "p");

        // Within the cooldown, reports pile up and go out together.
        let later = now + WAIT_GRACE + Duration::from_secs(1);
        report(&mut links, "a", ReportEventKind::Blocked);
        report(&mut links, "a", ReportEventKind::Done);
        assert!(links.take_due(later).is_empty());
        let due = links.take_due(later + CHILD_COOLDOWN);
        assert_eq!(due[0].events.len(), 2);

        // A parent gets at most PARENT_RATE deliveries a minute.
        let mut links = Links::default();
        for child in 0..=PARENT_RATE {
            let child = format!("c{child}");
            links.link(&child, "p", false, &entry(&child)).unwrap();
        }
        let mut delivered = 0;
        for (i, child) in (0..=PARENT_RATE).map(|c| format!("c{c}")).enumerate() {
            report(&mut links, &child, ReportEventKind::Done);
            delivered += links.take_due(now + Duration::from_millis(i as u64)).len();
        }
        assert_eq!(delivered, PARENT_RATE);
        assert_eq!(links.take_due(now + RATE_WINDOW).len(), 1);
    }

    #[test]
    fn a_report_the_parent_cannot_take_is_retried_then_kept() {
        let mut links = linked("a", "p");
        let now = Instant::now();
        report(&mut links, "a", ReportEventKind::Done);
        for attempt in 0..=MAX_ATTEMPTS {
            let at = now + CHILD_COOLDOWN * (attempt + 1);
            let due = links.take_due(at);
            assert_eq!(due.len(), 1, "attempt {attempt}");
            links.retry(due.into_iter().next().unwrap().events);
            // Keep the parent under its rate limit.
            links.parents.get_mut("p").unwrap().deliveries.clear();
        }
        assert!(links.take_due(now + CHILD_COOLDOWN * (MAX_ATTEMPTS + 3)).is_empty());
        let child = &links.children(Some("p"))[0];
        assert_eq!(
            child.last_event.as_ref().unwrap().delivery,
            Some(ReportDelivery::Kept)
        );
        // A wait still returns it.
        assert_eq!(
            links.take_for_wait(&["a".to_string()], false).unwrap()[0].delivery,
            Some(ReportDelivery::Wait)
        );
    }
}
