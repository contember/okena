//! Narrowing the task list by the groupings a provider reports.
//!
//! Kept apart from the view and free of GPUI so the one part with real rules —
//! what "filtered" means when several things are selected at once — can be
//! tested directly.
//!
//! Two rules, which together are what people expect of a faceted filter and
//! are the whole of the logic below:
//!
//! - **Within an axis, OR.** Picking two iterations means "either", because
//!   picking a second one is how you widen a view you have already narrowed.
//! - **Across axes, AND.** A project *and* an iteration means both, because
//!   picking a second axis is how you narrow.
//!
//! Labels and status are the same shape as a group axis — a set of values to
//! pick from — so they go through the same rules under their own headings. A
//! task has exactly one status where it may have many labels, which changes
//! nothing: "either of these two statuses" is still what picking two means.

use okena_core::tasks::{GroupAxis, Task};
use std::collections::{BTreeMap, BTreeSet};

/// The heading labels are filed under. Not a [`GroupAxis`]: a label is not a
/// grouping the provider defines, it is a free tag, and conflating the two
/// would let a provider define an axis that collides with it.
pub(super) const LABELS_HEADING: &str = "Labels";

/// The heading statuses are filed under. Status is the provider's own workflow
/// state, not a grouping it defines, so it is its own facet like labels.
pub(super) const STATUS_HEADING: &str = "Status";

/// What the task list is currently narrowed to.
///
/// Empty means "everything", which is the state a fresh view opens in — so
/// `Default` is the un-filtered filter and no caller has to special-case it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TaskFilter {
    /// Selected group ids, per axis. By id rather than name so a project
    /// renamed mid-session doesn't silently empty the board.
    groups: BTreeMap<GroupAxis, BTreeSet<String>>,
    /// Selected labels. By name because that is all a label has.
    labels: BTreeSet<String>,
    /// Selected statuses, by the provider's own name for them — the same
    /// string the row's chip shows, so the filter and the list cannot
    /// disagree about what a status is called.
    statuses: BTreeSet<String>,
}

impl TaskFilter {
    pub(crate) fn is_empty(&self) -> bool {
        self.labels.is_empty()
            && self.statuses.is_empty()
            && self.groups.values().all(|v| v.is_empty())
    }

    /// How many values are selected in total, for the "Filters (3)" badge.
    pub(super) fn selected_count(&self) -> usize {
        self.labels.len()
            + self.statuses.len()
            + self.groups.values().map(BTreeSet::len).sum::<usize>()
    }

    pub(super) fn clear(&mut self) {
        self.groups.clear();
        self.labels.clear();
        self.statuses.clear();
    }

    pub(super) fn group_selected(&self, axis: &GroupAxis, id: &str) -> bool {
        self.groups.get(axis).is_some_and(|v| v.contains(id))
    }

    pub(super) fn label_selected(&self, label: &str) -> bool {
        self.labels.contains(label)
    }

    pub(super) fn toggle_group(&mut self, axis: GroupAxis, id: &str) {
        let set = self.groups.entry(axis).or_default();
        if !set.remove(id) {
            set.insert(id.to_string());
        }
    }

    pub(super) fn toggle_label(&mut self, label: &str) {
        if !self.labels.remove(label) {
            self.labels.insert(label.to_string());
        }
    }

    pub(super) fn status_selected(&self, status: &str) -> bool {
        self.statuses.contains(status)
    }

    pub(super) fn toggle_status(&mut self, status: &str) {
        if !self.statuses.remove(status) {
            self.statuses.insert(status.to_string());
        }
    }

    /// Whether `task` survives the filter.
    pub(super) fn matches(&self, task: &Task) -> bool {
        for (axis, wanted) in &self.groups {
            if wanted.is_empty() {
                continue;
            }
            let hit = task
                .groups
                .iter()
                .any(|g| &g.axis == axis && wanted.contains(&g.id));
            if !hit {
                return false;
            }
        }
        if !self.labels.is_empty() && !task.labels.iter().any(|l| self.labels.contains(l)) {
            return false;
        }
        if !self.statuses.is_empty() && !self.statuses.contains(status_name(task)) {
            return false;
        }
        true
    }

    /// Drop selections that nothing on offer can satisfy any more.
    ///
    /// Without this, finishing the sprint you had selected leaves a filter
    /// that matches nothing, and the view reads as "you have no tasks" rather
    /// than "you are filtered to something that is gone".
    pub(super) fn prune(&mut self, facets: &Facets) {
        for (axis, selected) in self.groups.iter_mut() {
            let available: BTreeSet<&str> = facets
                .axes
                .iter()
                .find(|(a, _)| a == axis)
                .map(|(_, values)| values.iter().map(|v| v.id.as_str()).collect())
                .unwrap_or_default();
            selected.retain(|id| available.contains(id.as_str()));
        }
        self.groups.retain(|_, v| !v.is_empty());
        let available: BTreeSet<&str> = facets.labels.iter().map(|v| v.id.as_str()).collect();
        self.labels.retain(|l| available.contains(l.as_str()));
        let available: BTreeSet<&str> = facets.statuses.iter().map(|v| v.id.as_str()).collect();
        self.statuses.retain(|s| available.contains(s.as_str()));
    }
}

/// One value you can filter on, and how many of the loaded tasks carry it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FacetValue {
    /// What `TaskFilter` stores: a group's provider id, or a label's name.
    pub id: String,
    pub name: String,
    pub count: usize,
}

/// Everything the loaded tasks can be filtered by.
///
/// Derived from the tasks on screen rather than fetched, so the filter never
/// offers a choice that would empty the list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Facets {
    /// Axes in display order, each with its values most-used first.
    pub axes: Vec<(GroupAxis, Vec<FacetValue>)>,
    pub labels: Vec<FacetValue>,
    /// Statuses present, most-used first. Unlike an axis, this is offered even
    /// when every task shares one: with the list no longer split by status,
    /// seeing which statuses exist at all is half of what it is for.
    pub statuses: Vec<FacetValue>,
}

impl Facets {
    pub(super) fn is_empty(&self) -> bool {
        self.axes.is_empty() && self.labels.is_empty() && self.statuses.is_empty()
    }
}

/// Read the filterable facets off a set of tasks.
///
/// An axis every task agrees on is left out: filtering by the one team you
/// work in cannot change what you see, and a control that does nothing is
/// worse than no control. An axis only *some* tasks have is kept — narrowing
/// to "in a project" genuinely excludes the ones in none.
pub(super) fn collect_facets(tasks: &[Task]) -> Facets {
    let mut by_axis: BTreeMap<GroupAxis, BTreeMap<String, FacetValue>> = BTreeMap::new();
    let mut labels: BTreeMap<String, FacetValue> = BTreeMap::new();
    let mut statuses: BTreeMap<String, FacetValue> = BTreeMap::new();
    // How many tasks carry any value at all on each axis, which is what
    // decides whether a single-valued axis still distinguishes anything.
    let mut carriers: BTreeMap<GroupAxis, usize> = BTreeMap::new();

    for task in tasks {
        let mut seen_axes: BTreeSet<GroupAxis> = BTreeSet::new();
        for group in &task.groups {
            let entry = by_axis
                .entry(group.axis.clone())
                .or_default()
                .entry(group.id.clone())
                .or_insert_with(|| FacetValue {
                    id: group.id.clone(),
                    name: group.name.clone(),
                    count: 0,
                });
            entry.count += 1;
            seen_axes.insert(group.axis.clone());
        }
        for axis in seen_axes {
            *carriers.entry(axis).or_default() += 1;
        }
        for label in &task.labels {
            let entry = labels.entry(label.clone()).or_insert_with(|| FacetValue {
                id: label.clone(),
                name: label.clone(),
                count: 0,
            });
            entry.count += 1;
        }
        let status = status_name(task);
        let entry = statuses
            .entry(status.to_string())
            .or_insert_with(|| FacetValue {
                id: status.to_string(),
                name: status.to_string(),
                count: 0,
            });
        entry.count += 1;
    }

    let total = tasks.len();
    let axes = by_axis
        .into_iter()
        .filter(|(axis, values)| {
            // Two values distinguish tasks from each other; one value that not
            // every task has distinguishes them from the unassigned ones.
            values.len() > 1 || carriers.get(axis).copied().unwrap_or(0) < total
        })
        .map(|(axis, values)| (axis, ranked(values)))
        .collect();

    Facets {
        axes,
        labels: ranked(labels),
        statuses: ranked(statuses),
    }
}

/// What to call a task's status.
///
/// The provider's own name, so a team that calls it "Shipping" sees
/// "Shipping" — falling back to the normalized category only when the
/// provider gave no name at all, which would otherwise render as a blank chip
/// and a blank filter row.
pub(super) fn status_name(task: &Task) -> &str {
    if task.state_name.trim().is_empty() {
        task.state.label()
    } else {
        &task.state_name
    }
}

/// Most-used first, ties broken by name so the order never shuffles between
/// renders of the same data.
fn ranked(values: BTreeMap<String, FacetValue>) -> Vec<FacetValue> {
    let mut out: Vec<FacetValue> = values.into_values().collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::{TaskFilter, collect_facets};
    use okena_core::tasks::{GroupAxis, Task, TaskGroup, TaskId, TaskKind, TaskState};

    fn task(key: &str, groups: Vec<TaskGroup>, labels: &[&str]) -> Task {
        with_status(task_named(key, groups, labels), TaskState::Todo, "Todo")
    }

    fn with_status(mut task: Task, state: TaskState, name: &str) -> Task {
        task.state = state;
        task.state_name = name.into();
        task
    }

    fn task_named(key: &str, groups: Vec<TaskGroup>, labels: &[&str]) -> Task {
        Task {
            id: TaskId::new("linear", key),
            display_key: key.into(),
            title: key.into(),
            description: None,
            state: TaskState::Todo,
            state_name: "Todo".into(),
            url: String::new(),
            branch_name: String::new(),
            updated_at: String::new(),
            kind: TaskKind::Task,
            parent_id: None,
            parent_key: None,
            labels: labels.iter().map(|s| s.to_string()).collect(),
            groups,
        }
    }

    fn project(id: &str) -> TaskGroup {
        TaskGroup::new(GroupAxis::Project, id, format!("Project {id}"))
    }

    fn iteration(id: &str) -> TaskGroup {
        TaskGroup::new(GroupAxis::Iteration, id, format!("Cycle {id}"))
    }

    fn keys(filter: &TaskFilter, tasks: &[Task]) -> Vec<String> {
        tasks
            .iter()
            .filter(|t| filter.matches(t))
            .map(|t| t.display_key.clone())
            .collect()
    }

    fn sample() -> Vec<Task> {
        vec![
            task("A", vec![project("p1"), iteration("c1")], &["bug"]),
            task("B", vec![project("p2"), iteration("c1")], &[]),
            task("C", vec![project("p1"), iteration("c2")], &["bug", "ui"]),
            task("D", vec![], &["ui"]),
        ]
    }

    #[test]
    fn an_empty_filter_keeps_everything() {
        let f = TaskFilter::default();
        assert!(f.is_empty());
        assert_eq!(keys(&f, &sample()), ["A", "B", "C", "D"]);
    }

    #[test]
    fn two_values_on_one_axis_widen_the_view() {
        // Picking a second project is how you ask to see more, not less.
        let mut f = TaskFilter::default();
        f.toggle_group(GroupAxis::Project, "p1");
        assert_eq!(keys(&f, &sample()), ["A", "C"]);
        f.toggle_group(GroupAxis::Project, "p2");
        assert_eq!(keys(&f, &sample()), ["A", "B", "C"]);
    }

    #[test]
    fn two_axes_narrow_each_other() {
        // Project p1 alone is A and C; adding cycle c1 must leave only A.
        let mut f = TaskFilter::default();
        f.toggle_group(GroupAxis::Project, "p1");
        f.toggle_group(GroupAxis::Iteration, "c1");
        assert_eq!(keys(&f, &sample()), ["A"]);
    }

    #[test]
    fn labels_narrow_alongside_groups_and_or_among_themselves() {
        let mut f = TaskFilter::default();
        f.toggle_label("bug");
        assert_eq!(keys(&f, &sample()), ["A", "C"]);
        f.toggle_label("ui");
        assert_eq!(keys(&f, &sample()), ["A", "C", "D"]);
        // Across the two kinds of facet it narrows, like any other axis pair.
        f.toggle_group(GroupAxis::Project, "p1");
        assert_eq!(keys(&f, &sample()), ["A", "C"]);
    }

    #[test]
    fn a_task_with_nothing_on_an_axis_is_excluded_by_it() {
        // D is in no project, so asking for a project is asking to not see D.
        let mut f = TaskFilter::default();
        f.toggle_group(GroupAxis::Project, "p1");
        assert!(!keys(&f, &sample()).contains(&"D".to_string()));
    }

    #[test]
    fn toggling_the_last_value_off_restores_everything() {
        let mut f = TaskFilter::default();
        f.toggle_group(GroupAxis::Project, "p1");
        f.toggle_group(GroupAxis::Project, "p1");
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(keys(&f, &sample()), ["A", "B", "C", "D"]);
    }

    #[test]
    fn clear_and_count_track_the_selection() {
        let mut f = TaskFilter::default();
        f.toggle_group(GroupAxis::Project, "p1");
        f.toggle_group(GroupAxis::Iteration, "c1");
        f.toggle_label("bug");
        assert_eq!(f.selected_count(), 3);
        f.clear();
        assert_eq!(f.selected_count(), 0);
        assert!(f.is_empty());
    }

    #[test]
    fn facets_count_each_value_and_rank_by_use() {
        let facets = collect_facets(&sample());
        let projects = facets
            .axes
            .iter()
            .find(|(a, _)| *a == GroupAxis::Project)
            .expect("projects are a facet");
        assert_eq!(
            projects
                .1
                .iter()
                .map(|v| (&*v.name, v.count))
                .collect::<Vec<_>>(),
            // p1 has two tasks, p2 one — most-used first.
            vec![("Project p1", 2), ("Project p2", 1)]
        );
        assert_eq!(
            facets
                .labels
                .iter()
                .map(|v| (&*v.name, v.count))
                .collect::<Vec<_>>(),
            vec![("bug", 2), ("ui", 2)]
        );
    }

    #[test]
    fn an_axis_every_task_agrees_on_is_not_offered() {
        // Everyone is in the one team, so a team filter cannot change the
        // list — it would be a control that does nothing.
        let team = |id: &str| TaskGroup::new(GroupAxis::Team, id, "Qblok");
        let tasks = vec![
            task("A", vec![team("t1")], &[]),
            task("B", vec![team("t1")], &[]),
        ];
        let facets = collect_facets(&tasks);
        assert!(facets.axes.is_empty(), "{facets:?}");
    }

    #[test]
    fn one_value_some_tasks_lack_is_still_offered() {
        // Here the filter means "the ones in a project", which is a real
        // question to ask of this list.
        let tasks = vec![task("A", vec![project("p1")], &[]), task("B", vec![], &[])];
        let facets = collect_facets(&tasks);
        assert_eq!(facets.axes.len(), 1);
        assert_eq!(facets.axes[0].0, GroupAxis::Project);
    }

    #[test]
    fn axes_come_back_in_display_order() {
        let tasks = vec![
            task("A", vec![iteration("c1"), project("p1")], &[]),
            task("B", vec![], &[]),
        ];
        let facets = collect_facets(&tasks);
        assert_eq!(
            facets
                .axes
                .iter()
                .map(|(a, _)| a.clone())
                .collect::<Vec<_>>(),
            vec![GroupAxis::Project, GroupAxis::Iteration]
        );
    }

    #[test]
    fn pruning_drops_a_selection_whose_value_is_gone() {
        // The sprint you filtered to closed and its tasks left the queue.
        // Keeping the selection would show an empty board reading as "no
        // work" rather than "you are looking at something that is gone".
        let mut f = TaskFilter::default();
        f.toggle_group(GroupAxis::Iteration, "c2");
        f.toggle_label("ui");
        let remaining = vec![
            task("A", vec![iteration("c1")], &["bug"]),
            task("B", vec![], &[]),
        ];
        f.prune(&collect_facets(&remaining));
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn pruning_keeps_a_selection_that_is_still_on_offer() {
        let mut f = TaskFilter::default();
        f.toggle_group(GroupAxis::Project, "p1");
        f.prune(&collect_facets(&sample()));
        assert!(f.group_selected(&GroupAxis::Project, "p1"));
        assert_eq!(f.selected_count(), 1);
    }

    #[test]
    fn statuses_narrow_like_any_other_facet() {
        let tasks = vec![
            with_status(task("A", vec![], &[]), TaskState::InProgress, "Started"),
            with_status(task("B", vec![], &[]), TaskState::Todo, "Todo"),
            with_status(task("C", vec![], &[]), TaskState::InReview, "In Review"),
        ];
        let mut f = TaskFilter::default();
        f.toggle_status("Started");
        assert_eq!(keys(&f, &tasks), ["A"]);
        // Within the facet it widens, like every other one.
        f.toggle_status("In Review");
        assert_eq!(keys(&f, &tasks), ["A", "C"]);
    }

    #[test]
    fn status_narrows_against_the_other_facets() {
        let tasks = vec![
            with_status(
                task("A", vec![project("p1")], &[]),
                TaskState::InProgress,
                "Started",
            ),
            with_status(
                task("B", vec![project("p2")], &[]),
                TaskState::InProgress,
                "Started",
            ),
        ];
        let mut f = TaskFilter::default();
        f.toggle_status("Started");
        f.toggle_group(GroupAxis::Project, "p1");
        assert_eq!(keys(&f, &tasks), ["A"]);
    }

    #[test]
    fn a_single_status_is_still_offered_unlike_a_uniform_axis() {
        // The list is no longer split by status, so which statuses are present
        // is information in itself — even when there is only one.
        let tasks = vec![task("A", vec![], &[]), task("B", vec![], &[])];
        let facets = collect_facets(&tasks);
        assert_eq!(
            facets
                .statuses
                .iter()
                .map(|v| (&*v.name, v.count))
                .collect::<Vec<_>>(),
            vec![("Todo", 2)]
        );
    }

    #[test]
    fn a_status_with_no_provider_name_falls_back_to_the_normalized_one() {
        // A blank chip and a blank filter row are both unreadable.
        let tasks = vec![with_status(
            task("A", vec![], &[]),
            TaskState::InReview,
            "  ",
        )];
        let facets = collect_facets(&tasks);
        assert_eq!(facets.statuses[0].name, "In review");
        let mut f = TaskFilter::default();
        f.toggle_status("In review");
        assert_eq!(keys(&f, &tasks), ["A"]);
    }

    #[test]
    fn pruning_drops_a_status_nothing_carries_any_more() {
        let mut f = TaskFilter::default();
        f.toggle_status("Started");
        f.prune(&collect_facets(&[task("A", vec![], &[])]));
        assert!(f.is_empty(), "{f:?}");
    }
}
