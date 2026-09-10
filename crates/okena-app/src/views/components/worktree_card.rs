//! One worktree, shown the same way everywhere.
//!
//! A worktree turns up in three places — a project's lane, a task's detail, an
//! agent session's panel — and each had grown its own idea of what to say about
//! one. This is the single answer: which repo it belongs to, its branch, what
//! has changed in it, whether that has been pushed, and where the review stands.
//!
//! A free function over the host's `Context<V>` rather than an entity: the card
//! owns no state, and the three hosts are different types that each want their
//! own click behaviour.

use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::ui_text_ms;
use crate::workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{CiStatus, PrState};

/// Everything the card shows about one worktree.
#[derive(Clone, Debug)]
pub struct WorktreeSummary {
    pub project_id: String,
    /// The worktree's own name, which is usually its branch-derived directory.
    pub name: String,
    /// Repo it was created from. `None` when the parent is gone.
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub lines_added: usize,
    pub lines_removed: usize,
    /// Commits not yet pushed. `None` means the branch has never been pushed,
    /// which is a different state from "pushed, nothing outstanding".
    pub unpushed: Option<usize>,
    pub pr: Option<(u32, PrState)>,
    pub ci: Option<(CiStatus, usize, usize, usize)>,
}

impl WorktreeSummary {
    /// Read one worktree out of the workspace mirror.
    ///
    /// Returns `None` for anything that is not a worktree, so a caller cannot
    /// accidentally render a repo or a session as one.
    pub fn collect(ws: &Workspace, project_id: &str) -> Option<Self> {
        let project = ws.project(project_id)?;
        let info = project.worktree_info.as_ref()?;
        let git = ws
            .remote_snapshot(project_id)
            .and_then(|s| s.git_status.as_ref());
        Some(Self {
            project_id: project.id.clone(),
            name: project.name.clone(),
            repo: ws
                .project(&info.parent_project_id)
                .map(|parent| parent.name.clone()),
            branch: git
                .and_then(|g| g.branch.clone())
                // The daemon's git poll may not have reached a fresh worktree
                // yet; the branch it was created on is already known.
                .or_else(|| Some(info.branch_name.clone()))
                .filter(|b| !b.is_empty()),
            lines_added: git.map(|g| g.lines_added).unwrap_or(0),
            lines_removed: git.map(|g| g.lines_removed).unwrap_or(0),
            unpushed: git.and_then(|g| g.unpushed),
            pr: git.and_then(|g| g.pr_info.as_ref().map(|p| (p.number, p.state.clone()))),
            ci: git.and_then(|g| {
                g.ci_checks
                    .as_ref()
                    .map(|c| (c.status.clone(), c.passed, c.failed, c.pending))
            }),
        })
    }

    /// Whether anything has changed in this checkout.
    ///
    /// Drives whether a diff is worth offering: a button that opens an empty
    /// diff is worse than no button.
    pub fn has_changes(&self) -> bool {
        self.lines_added > 0 || self.lines_removed > 0
    }
}

/// How a branch stands against its remote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushState {
    /// Never pushed: this work exists only on this machine.
    Never,
    /// Pushed, with nothing outstanding.
    UpToDate,
    /// Pushed, with commits since.
    Outstanding(usize),
}

impl PushState {
    /// Read the push state from git's unpushed count.
    ///
    /// `None` and `Some(0)` mean genuinely different things — never pushed
    /// versus pushed and current — and collapsing them hides whether the work
    /// exists anywhere but here.
    pub fn from_unpushed(unpushed: Option<usize>) -> Self {
        match unpushed {
            None => PushState::Never,
            Some(0) => PushState::UpToDate,
            Some(n) => PushState::Outstanding(n),
        }
    }

    pub fn label(self) -> String {
        match self {
            PushState::Never => "unpushed".to_string(),
            PushState::UpToDate => "pushed".to_string(),
            PushState::Outstanding(n) => format!("{n} to push"),
        }
    }

    /// Whether this state needs the user to do something.
    pub fn is_pending(self) -> bool {
        !matches!(self, PushState::UpToDate)
    }
}

/// Colour and label for a pull request.
///
/// Coloured by state so a merged or closed PR does not read as work still
/// waiting on you.
pub fn pr_chip_style(number: u32, state: &PrState, t: &ThemeColors) -> (u32, String) {
    match state {
        PrState::Open => (t.success, format!("PR #{number}")),
        PrState::Draft => (t.text_muted, format!("PR #{number} draft")),
        PrState::Merged => (t.button_primary_bg, format!("PR #{number} merged")),
        PrState::Closed => (t.text_muted, format!("PR #{number} closed")),
    }
}

/// Colour and label for a pipeline's checks.
pub fn ci_chip_style(
    status: &CiStatus,
    passed: usize,
    failed: usize,
    pending: usize,
    t: &ThemeColors,
) -> (u32, String) {
    match status {
        CiStatus::Success => (t.success, format!("checks {passed}/{}", passed + failed)),
        CiStatus::Failure => (t.error, format!("{failed} failing")),
        CiStatus::Pending => (t.warning, format!("{pending} pending")),
    }
}

/// A small coloured label, the unit every checkout fact is shown in.
pub fn chip(text: String, color: u32, cx: &App) -> AnyElement {
    div()
        .flex_shrink_0()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(with_alpha(color, 0.15))
        .text_size(ui_text_ms(cx))
        .text_color(rgb(color))
        .child(text)
        .into_any_element()
}

/// Render one worktree.
///
/// `on_open` focuses it; `on_diff` shows what changed in it. Both are the
/// host's, because "open" means something slightly different in each place the
/// card appears.
pub fn render_worktree_card<V: 'static>(
    w: &WorktreeSummary,
    on_open: impl Fn(&mut V, &str, &mut Context<V>) + 'static,
    on_diff: impl Fn(&mut V, &str, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> AnyElement {
    let t = theme(cx);

    let mut chips: Vec<AnyElement> = Vec::new();
    if w.has_changes() {
        chips.push(chip(
            format!("+{} −{}", w.lines_added, w.lines_removed),
            t.text_muted,
            cx,
        ));
    }
    let push = PushState::from_unpushed(w.unpushed);
    chips.push(chip(
        push.label(),
        if push.is_pending() {
            t.warning
        } else {
            t.success
        },
        cx,
    ));
    if let Some((number, state)) = &w.pr {
        let (color, label) = pr_chip_style(*number, state, &t);
        chips.push(chip(label, color, cx));
    }
    if let Some((status, passed, failed, pending)) = &w.ci {
        let (color, label) = ci_chip_style(status, *passed, *failed, *pending, &t);
        chips.push(chip(label, color, cx));
    }

    let open_id = w.project_id.clone();
    let diff_id = w.project_id.clone();
    let show_diff = w.has_changes();

    v_flex()
        .id(SharedString::from(format!("wt-card-{}", w.project_id)))
        .cursor_pointer()
        .w_full()
        .min_w_0()
        .gap(px(3.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(4.0))
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(px(6.0))
                .child(
                    // The repo leads: in a task's detail or a session's panel
                    // the same branch name exists in several repos at once, and
                    // which one this is is the first thing you need.
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_primary))
                        .child(w.repo.clone().unwrap_or_else(|| w.name.clone())),
                )
                .when(show_diff, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("wt-diff-{}", w.project_id)))
                            .cursor_pointer()
                            .flex_shrink_0()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child("Diff")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    // The card itself opens the worktree; the
                                    // button must not do both.
                                    cx.stop_propagation();
                                    on_diff(this, &diff_id, cx);
                                }),
                            ),
                    )
                }),
        )
        .children(w.branch.clone().map(|branch| {
            div()
                .w_full()
                .min_w_0()
                .truncate()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(branch)
                .into_any_element()
        }))
        .child(h_flex().gap(px(4.0)).flex_wrap().children(chips))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _window, cx| {
                on_open(this, &open_id, cx);
            }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{PushState, WorktreeSummary};

    fn summary(added: usize, removed: usize) -> WorktreeSummary {
        WorktreeSummary {
            project_id: "wt1".into(),
            name: "okena (feat/x)".into(),
            repo: Some("okena".into()),
            branch: Some("feat/x".into()),
            lines_added: added,
            lines_removed: removed,
            unpushed: Some(0),
            pr: None,
            ci: None,
        }
    }

    #[test]
    fn a_diff_is_offered_only_when_something_changed() {
        // A button that opens an empty diff is worse than no button.
        assert!(summary(3, 1).has_changes());
        assert!(summary(0, 2).has_changes(), "deletions are changes too");
        assert!(!summary(0, 0).has_changes());
    }

    #[test]
    fn never_pushed_is_not_the_same_as_up_to_date() {
        // The difference between work that exists elsewhere and work that only
        // exists on this machine. Collapsing them hides a real risk.
        assert_eq!(PushState::from_unpushed(None), PushState::Never);
        assert_eq!(PushState::from_unpushed(Some(0)), PushState::UpToDate);
        assert_ne!(
            PushState::from_unpushed(None).label(),
            PushState::from_unpushed(Some(0)).label()
        );
    }

    #[test]
    fn outstanding_commits_are_counted() {
        assert_eq!(PushState::from_unpushed(Some(3)), PushState::Outstanding(3));
        assert_eq!(PushState::from_unpushed(Some(3)).label(), "3 to push");
    }

    #[test]
    fn only_an_up_to_date_branch_needs_nothing() {
        // Drives the chip colour: everything else is something to act on.
        assert!(!PushState::UpToDate.is_pending());
        assert!(PushState::Never.is_pending());
        assert!(PushState::Outstanding(1).is_pending());
    }
}
