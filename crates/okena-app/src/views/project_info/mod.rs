//! The project info panel — what okena knows about a repo or worktree, shown
//! beside its terminal.
//!
//! The project counterpart to the agent-session panel, hosted the same way: a
//! project column swaps it in behind the header's info toggle, or when the
//! overview-wide switch is on. It replaces the harness Projects view, which
//! showed these facts in lanes of their own — away from the terminals they
//! were about, and in a second place a project had to be looked for.

mod model;
mod render;

pub use model::{GitFacts, ProjectInfo, ProjectInfoKind};

use crate::views::agent_session::InfoPanelContext;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_terminal::TerminalsRegistry;

pub struct ProjectInfoPanel {
    request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    workspace: Entity<Workspace>,
    focus_manager: Entity<FocusManager>,
    window_id: WindowId,
    /// Read for whether the listed sessions are running, which the workspace
    /// mirror knows the layout of but not the life inside.
    terminals: TerminalsRegistry,
    /// The project this panel describes.
    project_id: String,
}

impl ProjectInfoPanel {
    pub fn new(project_id: String, ctx: InfoPanelContext, _cx: &mut Context<Self>) -> Self {
        Self {
            request_broker: ctx.request_broker,
            workspace: ctx.workspace,
            focus_manager: ctx.focus_manager,
            window_id: ctx.window_id,
            terminals: ctx.terminals,
            project_id,
        }
    }

    /// Everything shown, read fresh from the workspace mirror each frame.
    fn info(&self, cx: &App) -> Option<ProjectInfo> {
        ProjectInfo::collect(self.workspace.read(cx), &self.project_id)
    }

    /// Focus a worktree or session, leaving any harness view.
    fn open_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        crate::views::components::project_nav::focus_project(
            &self.workspace,
            &self.focus_manager,
            self.window_id,
            &project_id,
            cx,
        );
        cx.notify();
    }

    /// Show what changed in a checkout.
    fn open_diff(&mut self, project_id: &str, cx: &mut Context<Self>) {
        crate::views::components::project_nav::open_diff(&self.request_broker, project_id, cx);
    }
}
