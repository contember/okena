//! Provider-neutral task types for the engineering harness.
//!
//! Every provider (Linear today; Jira / Azure DevOps later) normalizes its own
//! issue representation into [`Task`]. The harness UI and the daemon only ever
//! see these types — nothing provider-specific escapes `okena-tasks`.
//!
//! These live in `okena-core` rather than `okena-tasks` for the same reason the
//! wire schema does: [`TaskRef`] is persisted on `ProjectData`, so `okena-state`
//! must see it, and `okena-state` cannot take on `okena-tasks`' networking
//! dependencies (reqwest/rustls) — it is the pure-data crate everything else
//! depends on. `okena-tasks` re-exports these and owns the client side.

use serde::{Deserialize, Serialize};

/// Identifies a task globally: which provider it came from, plus that
/// provider's own stable id.
///
/// `external_id` is the provider's *internal* id (Linear's UUID), not the
/// human-facing key — those differ, and only the internal one is stable across
/// renames and team moves. The human key lives in [`Task::display_key`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId {
    /// Provider id, e.g. `"linear"`. Matches `TaskProvider::id`.
    pub provider: String,
    /// The provider's internal identifier for this task.
    pub external_id: String,
}

impl TaskId {
    pub fn new(provider: impl Into<String>, external_id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            external_id: external_id.into(),
        }
    }
}

/// Normalized workflow state.
///
/// Providers model workflow states as open-ended user-defined lists (Linear
/// teams can name a state anything), so the concrete name is preserved in
/// [`Task::state_name`] and only the *category* is normalized here. Categories
/// are what the harness can reason about generically — "is this task done?"
/// must not depend on someone naming a column "Shipped".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Not yet triaged into the working set.
    Backlog,
    /// Ready to be picked up.
    Todo,
    /// Actively being worked.
    InProgress,
    /// Awaiting review / QA.
    InReview,
    /// Finished successfully.
    Done,
    /// Abandoned, duplicate, or otherwise closed unfinished.
    Canceled,
    /// The provider reported a category this version doesn't model.
    Unknown,
}

impl TaskState {
    /// Whether this state means the task no longer needs work.
    pub fn is_closed(self) -> bool {
        matches!(self, TaskState::Done | TaskState::Canceled)
    }

    /// Normalized name, for when a provider supplied none of its own.
    pub const fn label(self) -> &'static str {
        match self {
            TaskState::Backlog => "Backlog",
            TaskState::Todo => "Todo",
            TaskState::InProgress => "In progress",
            TaskState::InReview => "In review",
            TaskState::Done => "Done",
            TaskState::Canceled => "Canceled",
            TaskState::Unknown => "Unknown",
        }
    }

    /// Where this sits when sorting by status: furthest along first.
    ///
    /// A work queue sorted by status is being asked "what is closest to
    /// finishing", so review comes before in-progress and both come before
    /// anything not started. Closed states trail because the harness does not
    /// list them at all — they are here so the ordering is total rather than
    /// because anyone will see them.
    pub const fn sort_rank(self) -> u8 {
        match self {
            TaskState::InReview => 0,
            TaskState::InProgress => 1,
            TaskState::Todo => 2,
            TaskState::Backlog => 3,
            TaskState::Done => 4,
            TaskState::Canceled => 5,
            TaskState::Unknown => 6,
        }
    }
}

/// Where a task sits in the work breakdown.
///
/// Linear has no native epic/feature/story type — teams express it with
/// sub-issue nesting plus labels — so this is *derived*, not reported. Treat it
/// as a strong hint for grouping and colour, never as authoritative.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Epic,
    Feature,
    Story,
    /// A bug or defect — called out separately because it is triaged and
    /// scheduled differently from new work.
    Defect,
    #[default]
    #[serde(other)]
    Task,
}

impl TaskKind {
    pub const fn label(self) -> &'static str {
        match self {
            TaskKind::Epic => "Epic",
            TaskKind::Feature => "Feature",
            TaskKind::Story => "Story",
            TaskKind::Defect => "Defect",
            TaskKind::Task => "Task",
        }
    }

    /// Broadest first, so a task carrying both `epic` and `story` labels reads
    /// as the wider one rather than depending on label order.
    /// Stable lowercase name used on the wire and by agents.
    ///
    /// Separate from `label`, which is for reading: a display string that
    /// changed would silently change the protocol.
    pub const fn wire_name(self) -> &'static str {
        match self {
            TaskKind::Epic => "epic",
            TaskKind::Feature => "feature",
            TaskKind::Story => "story",
            TaskKind::Defect => "defect",
            TaskKind::Task => "task",
        }
    }

    pub const fn all() -> [TaskKind; 5] {
        [
            TaskKind::Epic,
            TaskKind::Feature,
            TaskKind::Story,
            TaskKind::Defect,
            TaskKind::Task,
        ]
    }

    /// Derive a kind from a task's labels.
    ///
    /// Matched case-insensitively against a small synonym set, because teams
    /// spell these differently ("bug" vs "defect", "epic" vs "initiative").
    /// Defect wins over the breakdown levels: a bug filed under a feature is
    /// still a bug, and that is what a reader needs to see.
    pub fn from_labels<'a>(labels: impl IntoIterator<Item = &'a str>) -> Option<TaskKind> {
        let mut found: Option<TaskKind> = None;
        for label in labels {
            let l = label.trim().to_ascii_lowercase();
            let kind = match l.as_str() {
                "bug" | "defect" | "fix" | "hotfix" => TaskKind::Defect,
                "epic" | "initiative" => TaskKind::Epic,
                "feature" => TaskKind::Feature,
                "story" | "user story" => TaskKind::Story,
                _ => continue,
            };
            // A defect label is decisive; otherwise keep the broadest seen.
            if kind == TaskKind::Defect {
                return Some(TaskKind::Defect);
            }
            found = Some(match found {
                None => kind,
                Some(existing) => {
                    let rank =
                        |k: TaskKind| TaskKind::all().iter().position(|x| *x == k).unwrap_or(9);
                    if rank(kind) < rank(existing) {
                        kind
                    } else {
                        existing
                    }
                }
            });
        }
        found
    }
}

/// Which axis a [`TaskGroup`] groups on.
///
/// Every backend has its own words for the same few shapes: Linear calls a
/// body of work a Project and a time box a Cycle; Azure DevOps calls them an
/// Area Path and an Iteration Path. Normalizing to the shape rather than the
/// word means the harness can offer "filter by iteration" without knowing
/// whose iteration it is.
///
/// [`GroupAxis::Other`] is the escape hatch, and it carries its name rather
/// than discarding it: a backend with an axis this build has never heard of
/// still gets grouped and filtered under whatever it calls the thing, instead
/// of vanishing. Ordering is display order — derived, so the known axes come
/// in the order above and unknown ones sort alphabetically after them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum GroupAxis {
    /// The owning team, or an Azure DevOps team project.
    Team,
    /// A named body of work: a Linear project, an ADO area path.
    Project,
    /// A time box: a Linear cycle, an ADO iteration path, a sprint.
    Iteration,
    /// An axis this build doesn't model, kept under the provider's own name.
    Other(String),
}

impl GroupAxis {
    /// Stable lowercase name used on the wire.
    pub fn wire_name(&self) -> &str {
        match self {
            GroupAxis::Team => "team",
            GroupAxis::Project => "project",
            GroupAxis::Iteration => "iteration",
            GroupAxis::Other(name) => name,
        }
    }

    /// Heading to show above this axis's values, upper-cased by the caller if
    /// that is the house style.
    pub fn label(&self) -> &str {
        match self {
            GroupAxis::Team => "Team",
            GroupAxis::Project => "Project",
            GroupAxis::Iteration => "Iteration",
            GroupAxis::Other(name) => name,
        }
    }
}

impl From<String> for GroupAxis {
    fn from(s: String) -> Self {
        match s.as_str() {
            "team" => GroupAxis::Team,
            "project" => GroupAxis::Project,
            "iteration" => GroupAxis::Iteration,
            _ => GroupAxis::Other(s),
        }
    }
}

impl From<GroupAxis> for String {
    fn from(a: GroupAxis) -> Self {
        match a {
            GroupAxis::Other(name) => name,
            other => other.wire_name().to_string(),
        }
    }
}

/// One grouping a task belongs to, beyond its labels.
///
/// Carries the provider's own id alongside the name because filtering has to
/// survive a rename: a sprint renamed mid-week must not silently empty the
/// board for anyone who had it selected.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskGroup {
    pub axis: GroupAxis,
    /// The provider's stable id for this group.
    pub id: String,
    /// What to show. Never empty — a provider with only a number to go on
    /// (Linear's unnamed cycles) composes one.
    pub name: String,
}

impl TaskGroup {
    pub fn new(axis: GroupAxis, id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            axis,
            id: id.into(),
            name: name.into(),
        }
    }
}

/// A task assigned to the authenticated user.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    /// Human-facing key shown in the UI, e.g. `"LIN-123"`.
    pub display_key: String,
    pub title: String,
    /// Long-form body. `None` when the provider omits it from list responses —
    /// absent means "not fetched", not "empty".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub state: TaskState,
    /// The provider's own name for the state, e.g. `"In Review"`. Shown in the
    /// UI so a team's own vocabulary survives the normalization above.
    pub state_name: String,
    /// Permalink to the task in the provider's web UI.
    pub url: String,
    /// Branch name the provider suggests for this task. Linear supplies one
    /// natively; providers without the concept get a derived fallback (see
    /// `TaskProvider::branch_name`).
    pub branch_name: String,
    /// Last modification time, RFC 3339. Used to detect changes between polls.
    pub updated_at: String,
    /// Where this sits in the breakdown. Derived — see [`TaskKind`].
    #[serde(default)]
    pub kind: TaskKind,
    /// Provider id of the parent task, when this is a sub-task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Human-facing key of the parent, so a child can name its parent without
    /// the parent needing to be in the same result set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_key: Option<String>,
    /// Provider labels, kept so the UI can show them and so kind derivation
    /// stays inspectable rather than a black box.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Groupings this task belongs to — its team, project, iteration.
    ///
    /// A list rather than a field per axis so a provider can report an axis
    /// this build has never heard of (see [`GroupAxis::Other`]) without a
    /// schema change on the way through.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<TaskGroup>,
}

impl Task {
    /// This task's group on `axis`, if it has one.
    pub fn group(&self, axis: &GroupAxis) -> Option<&TaskGroup> {
        self.groups.iter().find(|g| &g.axis == axis)
    }
}

/// Backlink stored on a worktree project, pointing at the task it was started
/// for. Persisted in the workspace, so the association survives restarts and
/// mirrors to web/mobile clients through the existing snapshot path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskRef {
    pub id: TaskId,
    /// Denormalized so the sidebar can label a worktree without a provider
    /// round-trip (and while offline). Refreshed on every successful poll.
    pub display_key: String,
    pub title: String,
    pub url: String,
}

impl From<&Task> for TaskRef {
    fn from(t: &Task) -> Self {
        Self {
            id: t.id.clone(),
            display_key: t.display_key.clone(),
            title: t.title.clone(),
            url: t.url.clone(),
        }
    }
}

// ─── Provider auth status (wire contract) ────────────────────────────────────

/// Whether a provider is ready to make calls.
///
/// Lives here rather than being hand-rolled as JSON on each side: the daemon
/// produces this and every client consumes it, so it is a wire contract and
/// both ends should share one definition. `Unknown` exists so an older client
/// meeting a newer daemon degrades to "reconnect" rather than misreading an
/// unfamiliar state as connected and showing an empty task list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TaskAuthState {
    /// No credential stored — the user has not connected this provider.
    #[default]
    Disconnected,
    /// Credential present and usable.
    Connected {
        /// Display name of the authenticated account, when the provider says.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
    },
    /// Credential present but rejected or expired; the user must re-authorize.
    Expired,
    /// A state this build doesn't model. Treated as "needs connecting".
    #[serde(other)]
    Unknown,
}

impl TaskAuthState {
    /// Whether calls can be attempted. Only a live credential qualifies —
    /// `Unknown` deliberately does not.
    pub fn is_connected(&self) -> bool {
        matches!(self, TaskAuthState::Connected { .. })
    }
}

/// One provider's identity and auth state, as reported by the daemon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskProviderStatus {
    /// Provider id, e.g. `"linear"`.
    pub provider: String,
    /// Human-facing name for the UI, e.g. `"Linear"`.
    pub display_name: String,
    #[serde(flatten)]
    pub auth: TaskAuthState,
}

/// Payload of the `TasksAuthStatus` action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TaskAuthStatusResponse {
    #[serde(default)]
    pub providers: Vec<TaskProviderStatus>,
}

impl TaskAuthStatusResponse {
    pub fn provider(&self, id: &str) -> Option<&TaskProviderStatus> {
        self.providers.iter().find(|p| p.provider == id)
    }
}

#[cfg(test)]
mod auth_status_tests {
    use super::*;

    fn decode(json: serde_json::Value) -> TaskAuthStatusResponse {
        serde_json::from_value(json).expect("should decode")
    }

    #[test]
    fn decodes_connected_with_account() {
        let r = decode(serde_json::json!({
            "providers": [{
                "provider": "linear", "display_name": "Linear",
                "state": "connected", "account": "Nima"
            }]
        }));
        let p = r.provider("linear").expect("linear present");
        assert_eq!(p.display_name, "Linear");
        assert_eq!(
            p.auth,
            TaskAuthState::Connected {
                account: Some("Nima".into())
            }
        );
        assert!(p.auth.is_connected());
    }

    #[test]
    fn decodes_connected_without_account() {
        let r = decode(serde_json::json!({
            "providers": [{
                "provider": "linear", "display_name": "Linear", "state": "connected"
            }]
        }));
        let p = r.provider("linear").expect("linear present");
        assert_eq!(p.auth, TaskAuthState::Connected { account: None });
    }

    #[test]
    fn decodes_disconnected_and_expired() {
        let r = decode(serde_json::json!({
            "providers": [
                { "provider": "a", "display_name": "A", "state": "disconnected" },
                { "provider": "b", "display_name": "B", "state": "expired" }
            ]
        }));
        assert_eq!(r.provider("a").unwrap().auth, TaskAuthState::Disconnected);
        assert_eq!(r.provider("b").unwrap().auth, TaskAuthState::Expired);
    }

    #[test]
    fn unrecognized_state_degrades_to_unknown_not_connected() {
        // Forward compatibility: a newer daemon adding a state must never read
        // as connected on an older client.
        let r = decode(serde_json::json!({
            "providers": [{
                "provider": "linear", "display_name": "Linear", "state": "reauthorizing"
            }]
        }));
        let p = r.provider("linear").expect("linear present");
        assert_eq!(p.auth, TaskAuthState::Unknown);
        assert!(!p.auth.is_connected());
    }

    #[test]
    fn absent_provider_is_none() {
        let r = decode(serde_json::json!({ "providers": [] }));
        assert!(r.provider("linear").is_none());
    }

    #[test]
    fn missing_providers_key_decodes_as_empty() {
        assert!(decode(serde_json::json!({})).providers.is_empty());
    }

    #[test]
    fn round_trips_through_json() {
        let original = TaskAuthStatusResponse {
            providers: vec![TaskProviderStatus {
                provider: "linear".into(),
                display_name: "Linear".into(),
                auth: TaskAuthState::Connected {
                    account: Some("Nima".into()),
                },
            }],
        };
        let json = serde_json::to_value(&original).expect("serialize");
        let back: TaskAuthStatusResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, original);
    }
}

#[cfg(test)]
mod kind_tests {
    use super::TaskKind;

    #[test]
    fn recognizes_each_level() {
        assert_eq!(TaskKind::from_labels(["epic"]), Some(TaskKind::Epic));
        assert_eq!(TaskKind::from_labels(["Feature"]), Some(TaskKind::Feature));
        assert_eq!(TaskKind::from_labels(["User Story"]), Some(TaskKind::Story));
        assert_eq!(TaskKind::from_labels(["bug"]), Some(TaskKind::Defect));
    }

    #[test]
    fn defect_synonyms_all_map_to_defect() {
        for l in ["bug", "defect", "Fix", "HOTFIX"] {
            assert_eq!(TaskKind::from_labels([l]), Some(TaskKind::Defect), "{l}");
        }
    }

    #[test]
    fn a_bug_under_a_feature_reads_as_a_defect() {
        // Order must not decide this: a bug is a bug wherever it is filed.
        assert_eq!(
            TaskKind::from_labels(["feature", "bug"]),
            Some(TaskKind::Defect)
        );
        assert_eq!(
            TaskKind::from_labels(["bug", "feature"]),
            Some(TaskKind::Defect)
        );
    }

    #[test]
    fn the_broadest_level_wins_when_several_apply() {
        assert_eq!(
            TaskKind::from_labels(["story", "epic"]),
            Some(TaskKind::Epic)
        );
        assert_eq!(
            TaskKind::from_labels(["epic", "story"]),
            Some(TaskKind::Epic)
        );
    }

    #[test]
    fn unrelated_labels_derive_nothing() {
        // `None`, not `Task`: the caller decides the fallback, and conflating
        // "no signal" with "it's a task" would hide that this is a guess.
        assert_eq!(TaskKind::from_labels(["p1", "frontend"]), None);
        assert_eq!(TaskKind::from_labels([]), None);
    }

    #[test]
    fn unknown_kind_decodes_as_task() {
        let k: TaskKind = serde_json::from_str("\"chore\"").expect("decode");
        assert_eq!(k, TaskKind::Task);
    }
}

#[cfg(test)]
mod group_tests {
    use super::{GroupAxis, Task, TaskGroup, TaskId, TaskKind, TaskState};

    fn task(groups: Vec<TaskGroup>) -> Task {
        Task {
            id: TaskId::new("linear", "x"),
            display_key: "X-1".into(),
            title: "t".into(),
            description: None,
            state: TaskState::Todo,
            state_name: "Todo".into(),
            url: String::new(),
            branch_name: String::new(),
            updated_at: String::new(),
            kind: TaskKind::Task,
            parent_id: None,
            parent_key: None,
            labels: Vec::new(),
            groups,
        }
    }

    #[test]
    fn a_known_axis_round_trips_through_its_wire_name() {
        for axis in [GroupAxis::Team, GroupAxis::Project, GroupAxis::Iteration] {
            let wire = serde_json::to_string(&axis).expect("serialize");
            let back: GroupAxis = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(back, axis, "{wire}");
        }
    }

    #[test]
    fn an_unmodelled_axis_keeps_its_name_instead_of_collapsing() {
        // The point of `Other`: an ADO area path reaching a build that has
        // never heard of one is still filterable, under ADO's own word.
        let axis: GroupAxis = serde_json::from_str("\"area_path\"").expect("deserialize");
        assert_eq!(axis, GroupAxis::Other("area_path".into()));
        assert_eq!(axis.label(), "area_path");
        assert_eq!(serde_json::to_string(&axis).unwrap(), "\"area_path\"");
    }

    #[test]
    fn axes_sort_in_display_order_with_unknown_ones_last() {
        let mut axes = vec![
            GroupAxis::Other("area".into()),
            GroupAxis::Iteration,
            GroupAxis::Team,
            GroupAxis::Project,
        ];
        axes.sort();
        assert_eq!(
            axes,
            vec![
                GroupAxis::Team,
                GroupAxis::Project,
                GroupAxis::Iteration,
                GroupAxis::Other("area".into()),
            ]
        );
    }

    #[test]
    fn a_task_finds_its_group_by_axis() {
        let t = task(vec![
            TaskGroup::new(GroupAxis::Team, "t1", "Qblok"),
            TaskGroup::new(GroupAxis::Iteration, "c9", "Cycle 9"),
        ]);
        assert_eq!(
            t.group(&GroupAxis::Iteration).map(|g| &*g.name),
            Some("Cycle 9")
        );
        assert_eq!(t.group(&GroupAxis::Project), None);
    }

    #[test]
    fn a_task_from_an_older_daemon_decodes_with_no_groups() {
        // `groups` is additive: a daemon that predates it must not fail to
        // decode, it must simply report nothing to group by.
        let json = serde_json::json!({
            "id": { "provider": "linear", "external_id": "x" },
            "display_key": "X-1", "title": "t",
            "state": "todo", "state_name": "Todo",
            "url": "", "branch_name": "", "updated_at": ""
        });
        let t: Task = serde_json::from_value(json).expect("decode");
        assert!(t.groups.is_empty());
    }
}

#[cfg(test)]
mod state_tests {
    use super::TaskState;

    #[test]
    fn sorting_by_status_puts_the_furthest_along_first() {
        let mut states = vec![
            TaskState::Backlog,
            TaskState::InProgress,
            TaskState::Todo,
            TaskState::InReview,
        ];
        states.sort_by_key(|s| s.sort_rank());
        assert_eq!(
            states,
            vec![
                TaskState::InReview,
                TaskState::InProgress,
                TaskState::Todo,
                TaskState::Backlog,
            ]
        );
    }

    #[test]
    fn every_state_ranks_distinctly_so_the_order_is_total() {
        let all = [
            TaskState::Backlog,
            TaskState::Todo,
            TaskState::InProgress,
            TaskState::InReview,
            TaskState::Done,
            TaskState::Canceled,
            TaskState::Unknown,
        ];
        let mut ranks: Vec<u8> = all.iter().map(|s| s.sort_rank()).collect();
        ranks.sort_unstable();
        ranks.dedup();
        assert_eq!(ranks.len(), all.len(), "two states share a rank");
    }

    #[test]
    fn closed_states_rank_after_open_ones() {
        for closed in [TaskState::Done, TaskState::Canceled] {
            for open in [TaskState::InReview, TaskState::InProgress, TaskState::Todo] {
                assert!(
                    closed.sort_rank() > open.sort_rank(),
                    "{closed:?} vs {open:?}"
                );
            }
        }
    }
}
