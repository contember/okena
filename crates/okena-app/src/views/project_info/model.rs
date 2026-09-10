//! What okena knows about one project, independent of how it is shown.
//!
//! The facts the harness Projects view used to gather into a lane per repo,
//! read here for a single project so they can sit beside its own terminal.

use crate::workspace::state::{ProjectData, Workspace};
use okena_core::api::{CiStatus, PrState};
use std::collections::HashSet;

/// Git facts for one checkout: branch, diff, divergence, PR and pipeline.
#[derive(Clone, Debug, Default)]
pub struct GitFacts {
    pub branch: Option<String>,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub pr: Option<(u32, PrState)>,
    pub ci: Option<(CiStatus, usize, usize, usize)>,
}

impl GitFacts {
    /// Read from the daemon snapshot that already feeds the column header, so
    /// the panel and the header chip cannot disagree.
    pub fn collect(ws: &Workspace, project_id: &str) -> Self {
        let Some(g) = ws
            .remote_snapshot(project_id)
            .and_then(|snap| snap.git_status.as_ref())
        else {
            return Self::default();
        };
        Self {
            branch: g.branch.clone(),
            lines_added: g.lines_added,
            lines_removed: g.lines_removed,
            ahead: g.ahead,
            behind: g.behind,
            pr: g.pr_info.as_ref().map(|pr| (pr.number, pr.state.clone())),
            ci: g
                .ci_checks
                .as_ref()
                .map(|c| (c.status.clone(), c.passed, c.failed, c.pending)),
        }
    }

    /// Whether anything has changed in the checkout — and so whether a diff is
    /// worth offering at all.
    pub fn has_changes(&self) -> bool {
        self.lines_added > 0 || self.lines_removed > 0
    }
}

/// What the project is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectInfoKind {
    Repo,
    /// A worktree, with the name of the repo it was created from while that is
    /// still open.
    Worktree {
        repo: Option<String>,
    },
}

/// Everything the panel shows about one project.
#[derive(Clone, Debug)]
pub struct ProjectInfo {
    pub project_id: String,
    pub path: String,
    pub kind: ProjectInfoKind,
    pub git: GitFacts,
    /// Worktrees open against a repo, by id. Always empty for a worktree.
    pub worktrees: Vec<String>,
    /// Agent sessions working this project's tasks, by id.
    pub sessions: Vec<String>,
}

impl ProjectInfo {
    /// Collect everything shown about `project_id`, or `None` if it is gone.
    pub fn collect(ws: &Workspace, project_id: &str) -> Option<Self> {
        let project = ws.project(project_id)?;
        let worktrees: Vec<&ProjectData> = project
            .worktree_ids
            .iter()
            .filter_map(|id| ws.project(id))
            .collect();
        let kind = match &project.worktree_info {
            Some(info) => ProjectInfoKind::Worktree {
                repo: ws
                    .project(&info.parent_project_id)
                    .map(|parent| parent.name.clone()),
            },
            None => ProjectInfoKind::Repo,
        };
        let tasks = task_ids(project, worktrees.iter().copied());

        Some(Self {
            project_id: project.id.clone(),
            path: project.path.clone(),
            kind,
            git: GitFacts::collect(ws, project_id),
            worktrees: worktrees.iter().map(|w| w.id.clone()).collect(),
            sessions: sessions_working(ws.projects(), &tasks),
        })
    }
}

/// Tasks a project has work in flight for: its own, when one was started on the
/// project itself, and those of the worktrees open against it.
pub(super) fn task_ids<'a>(
    project: &'a ProjectData,
    worktrees: impl Iterator<Item = &'a ProjectData>,
) -> HashSet<String> {
    project
        .task_ref
        .iter()
        .chain(worktrees.filter_map(|w| w.task_ref.as_ref()))
        .map(|t| t.id.external_id.clone())
        .collect()
}

/// Agent sessions working any of `task_ids`, in workspace order.
///
/// Matched by task rather than by directory: a session is rooted above the
/// repos precisely so one agent can span several, so it has no path that would
/// place it under a project. A worktree carries its task too, but it is a
/// checkout, not a session, and is never listed as one.
pub(super) fn sessions_working(
    projects: &[ProjectData],
    task_ids: &HashSet<String>,
) -> Vec<String> {
    projects
        .iter()
        .filter(|p| p.worktree_info.is_none() && p.is_any_agent_session())
        .filter(|p| {
            p.task_ref
                .as_ref()
                .is_some_and(|t| task_ids.contains(&t.id.external_id))
        })
        .map(|p| p.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{sessions_working, task_ids};
    use crate::workspace::state::ProjectData;
    use std::collections::HashSet;

    fn project(json: serde_json::Value) -> ProjectData {
        serde_json::from_value(json).unwrap()
    }

    fn task(external_id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": { "provider": "linear", "external_id": external_id },
            "display_key": "QBL-1", "title": "t", "url": "http://x",
        })
    }

    fn task_session(id: &str, external_id: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": "QBL-1 (agent)", "path": "/p", "task_ref": task(external_id),
        }))
    }

    fn worktree(id: &str, external_id: &str) -> ProjectData {
        project(serde_json::json!({
            "id": id, "name": "okena (QBL-1)", "path": "/p/wt",
            "worktree_info": {
                "parent_project_id": "repo",
                "main_repo_path": "/p/okena",
                "worktree_path": "/p/wt",
                "branch_name": "feat/x",
            },
            "task_ref": task(external_id),
        }))
    }

    fn ids(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_repo_has_the_tasks_of_its_worktrees() {
        let repo = project(serde_json::json!({ "id": "repo", "name": "okena", "path": "/p" }));
        let worktrees = [worktree("wt1", "u1"), worktree("wt2", "u2")];
        assert_eq!(task_ids(&repo, worktrees.iter()), ids(&["u1", "u2"]));
    }

    #[test]
    fn a_task_started_on_the_repo_itself_counts_too() {
        let repo = project(serde_json::json!({
            "id": "repo", "name": "okena", "path": "/p", "task_ref": task("u3"),
        }));
        assert_eq!(task_ids(&repo, std::iter::empty()), ids(&["u3"]));
    }

    #[test]
    fn a_session_is_listed_under_the_project_whose_task_it_works() {
        let projects = [task_session("s1", "u1"), task_session("s2", "u9")];
        assert_eq!(sessions_working(&projects, &ids(&["u1", "u2"])), ["s1"]);
    }

    #[test]
    fn a_worktree_on_the_task_is_not_listed_as_a_session() {
        // It carries the same task as the session working it, and is the
        // checkout that session writes to — not a second agent.
        let projects = [worktree("wt1", "u1"), task_session("s1", "u1")];
        assert_eq!(sessions_working(&projects, &ids(&["u1"])), ["s1"]);
    }

    #[test]
    fn a_spec_session_belongs_to_no_project() {
        // It works the spec repository, which is not a project here.
        let spec = project(serde_json::json!({
            "id": "s1", "name": "add-login (spec)", "path": "/specs",
            "spec_change": "add-login",
        }));
        assert!(sessions_working(&[spec], &ids(&["u1"])).is_empty());
    }

    #[test]
    fn a_project_with_no_tasks_claims_no_sessions() {
        // Nothing but the task links a session to a project — a loose match
        // would list one agent under every repo.
        assert!(sessions_working(&[task_session("s1", "u1")], &ids(&[])).is_empty());
    }
}
