//! Harness views, rendered full-width in the window's main content area.
//!
//! One view shows at a time, selected from the sidebar's HARNESS nav. The pane
//! holds handles to the same workspace mirror and focus manager the terminal
//! workspace uses, so a view can act on projects (focus one, start a worktree)
//! rather than only display them.

mod knowledge_draft;
mod knowledge_view;
mod markdown;
mod new_task_form;
mod sections;
mod specs_view;
mod task_filter;
mod tasks_view;

use crate::views::components::SimpleInputState;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_core::tasks::{Task, TaskAuthState};
use okena_terminal::TerminalsRegistry;
use std::cell::RefCell;
use std::rc::Rc;

pub use okena_core::harness::HarnessSection;

/// Tasks-view state. Grouped so the pane struct stays readable as more views
/// grow their own state.
pub(crate) struct TasksState {
    pub(crate) provider: String,
    pub(crate) provider_display_name: String,
    pub(crate) connection: TaskAuthState,
    pub(crate) tasks: Vec<Task>,
    pub(crate) loading: bool,
    /// External id of the task whose worktree is being created. Blocks a second
    /// concurrent create, which would race on the worktree path and git's
    /// index lock.
    pub(crate) starting: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) api_key_input: Entity<SimpleInputState>,
    /// Share of the board width given to the Todo lane, 0..1.
    pub(crate) lane_fraction: f32,
    /// Task ids whose sub-tasks are hidden. Collapsed rather than expanded
    /// state so a fresh view shows the whole breakdown by default.
    pub(crate) collapsed: std::collections::HashSet<String>,
    /// Sub-tasks fetched per task, keyed by the parent's provider id.
    ///
    /// Fetched rather than read off the loaded list because the list is the
    /// user's own queue: a task's children are often assigned to somebody
    /// else, or to nobody, and would simply be missing.
    pub(crate) children: std::collections::HashMap<String, Vec<Task>>,
    /// Task whose children are being fetched, so a slow provider does not
    /// spawn a request per frame.
    pub(crate) children_loading: Option<String>,
    /// The task shown in the detail pane, by provider id.
    pub(crate) selected: Option<String>,
    /// The selected task's description, parsed as Markdown.
    pub(crate) description: markdown::MarkdownCache,
    /// Sections folded shut in the list. Collapsed rather than expanded state,
    /// so a fresh view shows everything.
    pub(crate) sections_collapsed: std::collections::HashSet<String>,
    /// External id of the task whose breakdown agent is starting. Blocks a
    /// second start: creating a project and launching an agent takes seconds,
    /// and without this a second click during the wait made a second agent.
    pub(crate) breaking_down: Option<String>,
    /// Open "New task" form, if any.
    pub(crate) new_task: Option<new_task_form::NewTaskForm>,
    pub(crate) new_task_title: Entity<SimpleInputState>,
    pub(crate) new_task_body: Entity<SimpleInputState>,
    /// Open "Start work" dialog, if any.
    pub(crate) start_form: Option<StartWorkForm>,
    /// What the list is narrowed to. Empty means everything.
    pub(crate) filter: task_filter::TaskFilter,
    /// Whether the facet panel is open. Shut by default: the filters are a
    /// tool you reach for, and a permanent wall of chips above the list would
    /// cost every reader space to show nothing most of the time.
    pub(crate) filter_open: bool,
    /// How the list is ordered within each section.
    pub(crate) sort: tasks_view::TaskSort,
    /// Agent command configured on the daemon, used as the dialog's default.
    /// `None` until settings have been read.
    pub(crate) default_agent: Option<String>,
}

/// Specs-view state.
pub(crate) struct SpecsState {
    /// Every root the daemon discovered. `None` until the first load lands.
    pub(crate) stores: Option<okena_core::specs::SpecStores>,
    /// Key of the root being shown. `None` until the first load picks the
    /// default one.
    pub(crate) root_key: Option<String>,
    /// The open root's planning tree.
    pub(crate) tree: Option<okena_core::specs::SpecTree>,
    pub(crate) loading: bool,
    /// Bumped on every load, so a slow response for a root the user has since
    /// left is dropped instead of replacing the newer one.
    pub(crate) load_generation: u64,
    pub(crate) error: Option<String>,
    /// Path of the document being read, relative to the root.
    pub(crate) selected: Option<String>,
    pub(crate) content: Option<markdown::OpenDocument>,
    pub(crate) content_error: Option<String>,
    /// The idea a new change is drafted from.
    pub(crate) idea_input: Entity<SimpleInputState>,
    /// Agent to draft with; `None` scaffolds the change and launches nothing.
    pub(crate) agent: Option<String>,
    /// Whether the view is showing the new-change form instead of the specs.
    ///
    /// A full-view swap rather than a pane: configuring a session and reading
    /// specs are separate tasks, and splitting the space between them served
    /// neither well.
    pub(crate) composing: bool,
    /// Directory name for the change being configured. Blank derives one from
    /// the prompt.
    pub(crate) name_input: Entity<SimpleInputState>,
    /// Whether the user has chosen an agent themselves. Until they have, the
    /// picker follows the daemon's configured default; afterwards it must not,
    /// or an explicit "No agent" would be silently overwritten when settings
    /// land.
    pub(crate) agent_picked: bool,
    /// Root a new change is drafted into. Follows the open root until the
    /// user picks another in the form.
    pub(crate) draft_root: Option<String>,
    pub(crate) drafting: bool,
    /// Change names whose documents are hidden. Collapsed rather than expanded
    /// state, so a fresh view shows everything.
    pub(crate) collapsed: std::collections::HashSet<String>,
}

impl SpecsState {
    /// Whether `path` is still listed anywhere in `tree`.
    ///
    /// Used after a refresh to drop a selection whose file has gone, so a
    /// deleted document doesn't leave stale content on screen looking current.
    pub(crate) fn contains(tree: &okena_core::specs::SpecTree, path: &str) -> bool {
        let in_change = |c: &okena_core::specs::SpecChange| {
            c.artifacts
                .iter()
                .chain(c.specs.iter())
                .any(|d| d.path == path)
        };
        tree.specs.iter().any(|d| d.path == path)
            || tree.changes.iter().any(in_change)
            || tree.archived.iter().any(in_change)
    }

    /// Whether the currently-loaded tree still lists `path`.
    fn tree_contains(&self, path: &str) -> bool {
        self.tree.as_ref().is_some_and(|t| Self::contains(t, path))
    }
}

/// State of the "Start work" dialog.
///
/// Everything the run needs is decided here rather than inferred from the view,
/// so what the user sees is exactly what gets dispatched.
pub(crate) struct StartWorkForm {
    pub(crate) task: Task,
    pub(crate) project_ids: Vec<String>,
    /// Branch name, which also determines each worktree's directory name.
    pub(crate) branch_input: Entity<SimpleInputState>,
    /// Agent to launch; `None` means create worktrees only.
    pub(crate) agent: Option<String>,
}

/// Smallest share either lane may be squeezed to, so a drag can never collapse
/// one entirely and strand its tasks.
pub(crate) const MIN_LANE_FRACTION: f32 = 0.15;

pub struct HarnessPane {
    pub(crate) client: okena_transport::remote_action::RemoteActionClient,
    /// Lets a view open an overlay — the settings modal, mainly — without
    /// knowing anything about the window that owns it.
    pub(crate) request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    pub(crate) workspace: Entity<Workspace>,
    pub(crate) focus_manager: Entity<FocusManager>,
    pub(crate) window_id: WindowId,
    pub(crate) terminals: TerminalsRegistry,
    /// Shared with the window so lane dragging uses the same global mouse-move
    /// handler every other resize in the app goes through.
    pub(crate) active_drag:
        Rc<RefCell<Option<okena_views_terminal::layout::split_pane::DragState>>>,
    /// Board width from the last frame, used to turn a drag into a fraction.
    pub(crate) board_width: Rc<RefCell<f32>>,
    pub(crate) section: HarnessSection,
    pub(crate) tasks: TasksState,
    pub(crate) specs: SpecsState,
    pub(crate) knowledge: knowledge_view::KnowledgeState,
    /// The Knowledge view's "New with agent" form.
    pub(crate) knowledge_draft: knowledge_draft::DraftForm,
}

/// Everything a harness pane needs from its window.
///
/// Bundled because the window hands over the same set for every pane, and a
/// seven-parameter constructor invites silent argument transposition.
#[derive(Clone)]
pub struct PaneContext {
    pub client: okena_transport::remote_action::RemoteActionClient,
    pub request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    pub workspace: Entity<Workspace>,
    pub focus_manager: Entity<FocusManager>,
    pub window_id: WindowId,
    pub terminals: TerminalsRegistry,
    pub active_drag: Rc<RefCell<Option<okena_views_terminal::layout::split_pane::DragState>>>,
}

impl HarnessPane {
    pub fn new(section: HarnessSection, ctx: PaneContext, cx: &mut Context<Self>) -> Self {
        let api_key_input = cx
            .new(|cx| SimpleInputState::new(cx).placeholder("Paste your Linear personal API key…"));
        let new_task_title =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("What needs doing?"));
        let new_task_body = cx.new(|cx| {
            SimpleInputState::new(cx).placeholder("What it covers, and what finishing it means")
        });
        let name_input = cx.new(|cx| SimpleInputState::new(cx).placeholder("add-login"));
        let idea_input = cx.new(|cx| {
            SimpleInputState::new(cx).placeholder(
                "e.g. let users sign in with Google, alongside the existing \
                     email flow",
            )
        });
        let knowledge = knowledge_view::KnowledgeState::new(cx);
        let knowledge_draft = knowledge_draft::DraftForm::new(cx);
        let mut pane = Self {
            client: ctx.client,
            request_broker: ctx.request_broker,
            workspace: ctx.workspace,
            focus_manager: ctx.focus_manager,
            window_id: ctx.window_id,
            terminals: ctx.terminals,
            active_drag: ctx.active_drag,
            board_width: Rc::new(RefCell::new(0.0)),
            section,
            tasks: TasksState {
                provider: "linear".to_string(),
                provider_display_name: "Linear".to_string(),
                connection: TaskAuthState::Unknown,
                tasks: Vec::new(),
                loading: false,
                starting: None,
                error: None,
                status: None,
                api_key_input,
                lane_fraction: 0.5,
                collapsed: std::collections::HashSet::new(),
                children: std::collections::HashMap::new(),
                children_loading: None,
                selected: None,
                description: markdown::MarkdownCache::default(),
                sections_collapsed: std::collections::HashSet::new(),
                breaking_down: None,
                new_task: None,
                new_task_title,
                new_task_body,
                start_form: None,
                filter: task_filter::TaskFilter::default(),
                filter_open: false,
                sort: tasks_view::TaskSort::default(),
                default_agent: None,
            },
            specs: SpecsState {
                stores: None,
                root_key: None,
                tree: None,
                loading: false,
                load_generation: 0,
                error: None,
                selected: None,
                content: None,
                content_error: None,
                idea_input,
                agent: None,
                agent_picked: false,
                composing: false,
                name_input,
                draft_root: None,
                drafting: false,
                collapsed: std::collections::HashSet::new(),
            },
            knowledge,
            knowledge_draft,
        };
        match section {
            HarnessSection::Tasks => {
                pane.refresh_auth(cx);
                // So the dialog can default to the daemon's configured agent.
                pane.refresh_default_agent(cx);
            }
            HarnessSection::Specs => {
                pane.refresh_specs(cx);
                pane.refresh_default_agent(cx);
            }
            HarnessSection::Knowledge => {
                pane.refresh_knowledge(cx);
                // So "New with agent" defaults to the configured agent.
                pane.refresh_default_agent(cx);
            }
        }
        pane
    }
}

impl HarnessPane {
    /// Apply a lane-divider drag. Clamped so neither lane can be collapsed.
    pub fn set_lane_fraction(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let clamped = fraction.clamp(MIN_LANE_FRACTION, 1.0 - MIN_LANE_FRACTION);
        if (self.tasks.lane_fraction - clamped).abs() > f32::EPSILON {
            self.tasks.lane_fraction = clamped;
            cx.notify();
        }
    }
}
