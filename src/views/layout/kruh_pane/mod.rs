pub mod agent;
pub mod config;
pub mod git;
pub mod loop_runner;
pub mod render;
pub mod status_parser;
pub mod types;

use std::path::Path;
use std::time::Instant;

use gpui::*;

use crate::keybindings::{
    CloseTerminal, FocusDown, FocusLeft, FocusNextTerminal, FocusPrevTerminal, FocusRight, FocusUp,
};
use crate::views::components::simple_input::SimpleInputState;
use crate::views::layout::navigation::{
    get_pane_map, register_pane_bounds, NavigationDirection,
};
use crate::workspace::state::Workspace;
use config::{KruhConfig, KruhPlanOverrides};
use types::{EditTarget, IssueDetail, KruhPaneEvent, KruhState, OutputLine, PlanInfo, StatusProgress};

/// Native Kruh pane — automated AI agent loop tool.
pub struct KruhPane {
    pub workspace: Entity<Workspace>,
    pub project_id: String,
    pub project_path: String,
    pub layout_path: Vec<usize>,
    pub app_id: Option<String>,
    pub focus_handle: FocusHandle,
    pub config: KruhConfig,
    pub state: KruhState,
    pub iteration: usize,
    pub pass_count: usize,
    pub fail_count: usize,
    pub start_time: Option<Instant>,
    pub agent_handle: Option<agent::AgentHandle>,
    pub output_lines: Vec<OutputLine>,
    pub output_scroll: ScrollHandle,
    pub diff_stat: Option<String>,
    pub progress: StatusProgress,
    pub paused: bool,
    pub step_mode: bool,
    pub skip_requested: bool,
    pub quit_requested: bool,
    pub step_advance_requested: bool,
    pub _loop_task: Option<Task<()>>,

    // Plan picker state
    pub plans: Vec<PlanInfo>,
    pub selected_plan_index: usize,
    pub selected_plan: Option<PlanInfo>,
    pub plans_dir: String,
    pub plan_scroll: ScrollHandle,

    // Task browser state
    pub issues: Vec<IssueDetail>,
    pub selected_issue_index: usize,
    pub issue_scroll: ScrollHandle,

    // Settings
    pub agent_dropdown_open: bool,
    pub agent_button_bounds: Option<Bounds<Pixels>>,
    pub model_input: Entity<SimpleInputState>,
    pub settings_scroll: ScrollHandle,

    // Setup screen
    pub setup_path_input: Entity<SimpleInputState>,

    // Editor state
    pub editor_input: Option<Entity<SimpleInputState>>,
    pub editor_target: Option<EditTarget>,
    pub editor_file_path: Option<String>,

    // Frontmatter editor (active when editing INSTRUCTIONS.md)
    pub editor_fm_agent: Option<Entity<SimpleInputState>>,
    pub editor_fm_model: Option<Entity<SimpleInputState>>,
    pub editor_fm_max_iters: Option<Entity<SimpleInputState>>,
    pub editor_fm_sleep: Option<Entity<SimpleInputState>>,
    pub editor_fm_dangerous: Option<Entity<SimpleInputState>>,

    // Async scan task
    pub _scan_task: Option<Task<()>>,
}

impl KruhPane {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        project_path: String,
        layout_path: Vec<usize>,
        app_id: Option<String>,
        config: KruhConfig,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Determine initial plans_dir: saved config or default .plans/
        let plans_dir = if !config.plans_dir.is_empty() {
            config.plans_dir.clone()
        } else {
            let default_plans = Path::new(&project_path).join(".plans");
            default_plans.to_string_lossy().to_string()
        };

        let model_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("claude-sonnet-4-6")
                .default_value(&config.model)
        });
        let setup_path_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Enter path...")
                .default_value(&plans_dir)
        });

        let mut pane = Self {
            workspace,
            project_id,
            project_path,
            layout_path,
            app_id,
            focus_handle: cx.focus_handle(),
            config,
            state: KruhState::Scanning,
            iteration: 0,
            pass_count: 0,
            fail_count: 0,
            start_time: None,
            agent_handle: None,
            output_lines: Vec::new(),
            output_scroll: ScrollHandle::new(),
            diff_stat: None,
            progress: StatusProgress::default(),
            paused: false,
            step_mode: false,
            skip_requested: false,
            quit_requested: false,
            step_advance_requested: false,
            _loop_task: None,
            plans: Vec::new(),
            selected_plan_index: 0,
            selected_plan: None,
            plans_dir,
            plan_scroll: ScrollHandle::new(),
            issues: Vec::new(),
            selected_issue_index: 0,
            issue_scroll: ScrollHandle::new(),
            agent_dropdown_open: false,
            agent_button_bounds: None,
            model_input,
            settings_scroll: ScrollHandle::new(),
            setup_path_input,
            editor_input: None,
            editor_target: None,
            editor_file_path: None,
            editor_fm_agent: None,
            editor_fm_model: None,
            editor_fm_max_iters: None,
            editor_fm_sleep: None,
            editor_fm_dangerous: None,
            _scan_task: None,
        };

        pane.start_scan(cx);
        pane
    }

    /// Kick off an async scan of the plans directory.
    pub fn start_scan(&mut self, cx: &mut Context<Self>) {
        self.state = KruhState::Scanning;
        let plans_dir = self.plans_dir.clone();

        self._scan_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let plans = smol::unblock(move || status_parser::scan_plans(&plans_dir)).await;

            let _ = this.update(cx, |pane, cx| {
                pane.plans = plans;
                if pane.plans.is_empty() {
                    pane.state = KruhState::Settings;
                } else {
                    pane.state = KruhState::PlanPicker;
                }
                cx.notify();
            });
        }));
    }

    /// Select a plan and transition to TaskBrowser.
    pub fn select_plan(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(plan) = self.plans.get(index).cloned() {
            self.config.docs_dir = plan.dir.clone();
            self.selected_plan = Some(plan.clone());

            // Load issues
            let docs_dir = plan.dir.clone();
            self.issues = status_parser::load_issue_details(&docs_dir);

            // Update progress from plan info
            self.progress.pending = plan.pending;
            self.progress.done = plan.done;
            self.progress.total = plan.total;

            self.selected_issue_index = 0;
            self.state = KruhState::TaskBrowser;
            cx.notify();
        }
    }

    /// Navigate back from TaskBrowser to PlanPicker.
    pub fn navigate_to_plan_picker(&mut self, cx: &mut Context<Self>) {
        self.selected_plan = None;
        self.issues.clear();
        self.selected_issue_index = 0;
        self.state = KruhState::PlanPicker;
        cx.notify();
    }

    /// Open the editor for a given target file (STATUS.md or INSTRUCTIONS.md).
    pub fn open_editor(
        &mut self,
        target: EditTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let docs_dir = &self.config.docs_dir;
        if docs_dir.is_empty() {
            return;
        }

        let file_path = match &target {
            EditTarget::Status => format!("{}/STATUS.md", docs_dir),
            EditTarget::Instructions => format!("{}/INSTRUCTIONS.md", docs_dir),
            EditTarget::Issue => return, // use open_issue_editor instead
        };

        self.open_editor_at(file_path, target, window, cx);
    }

    /// Open an issue file for editing by its index in the issues list.
    pub fn open_issue_editor(
        &mut self,
        issue_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let issue = match self.issues.get(issue_index) {
            Some(issue) => issue.clone(),
            None => return,
        };
        let docs_dir = &self.config.docs_dir;
        if docs_dir.is_empty() {
            return;
        }

        let file_path = if let Some(path) =
            status_parser::find_issue_file_path(docs_dir, &issue.ref_info.number)
        {
            path
        } else if !issue.ref_info.number.is_empty() {
            // Create a new issue file
            let issues_dir = format!("{}/issues", docs_dir);
            let _ = std::fs::create_dir_all(&issues_dir);
            let slug = issue
                .ref_info
                .name
                .to_lowercase()
                .replace(' ', "-");
            let path = format!("{}/{}-{}.md", issues_dir, issue.ref_info.number, slug);
            let _ = std::fs::write(&path, format!("# {}\n\n", issue.ref_info.name));
            path
        } else {
            // No number — fall back to STATUS.md
            format!("{}/STATUS.md", docs_dir)
        };

        self.open_editor_at(file_path, EditTarget::Issue, window, cx);
    }

    /// Open a file at the given path for editing.
    fn open_editor_at(
        &mut self,
        file_path: String,
        target: EditTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = std::fs::read_to_string(&file_path).unwrap_or_default();

        // For issue files, parse frontmatter into structured override inputs
        if target == EditTarget::Issue {
            let overrides = status_parser::parse_plan_overrides_content(&content);
            let body = status_parser::strip_frontmatter(&content);

            self.editor_fm_agent = Some(cx.new(|cx| {
                SimpleInputState::new(cx)
                    .placeholder(&self.config.agent)
                    .default_value(overrides.agent.as_deref().unwrap_or(""))
            }));
            self.editor_fm_model = Some(cx.new(|cx| {
                SimpleInputState::new(cx)
                    .placeholder(&self.config.model)
                    .default_value(overrides.model.as_deref().unwrap_or(""))
            }));
            self.editor_fm_max_iters = Some(cx.new(|cx| {
                SimpleInputState::new(cx)
                    .placeholder(&self.config.max_iterations.to_string())
                    .default_value(
                        &overrides.max_iterations.map(|n| n.to_string()).unwrap_or_default(),
                    )
            }));
            self.editor_fm_sleep = Some(cx.new(|cx| {
                SimpleInputState::new(cx)
                    .placeholder(&self.config.sleep_secs.to_string())
                    .default_value(
                        &overrides.sleep_secs.map(|n| n.to_string()).unwrap_or_default(),
                    )
            }));
            self.editor_fm_dangerous = Some(cx.new(|cx| {
                SimpleInputState::new(cx)
                    .placeholder(if self.config.dangerous { "true" } else { "false" })
                    .default_value(
                        &overrides.dangerous.map(|b| b.to_string()).unwrap_or_default(),
                    )
            }));

            let input = cx.new(|cx| {
                SimpleInputState::new(cx)
                    .multiline()
                    .default_value(body)
            });

            let editor_focus = input.read(cx).focus_handle(cx);
            window.focus(&editor_focus, cx);

            self.editor_input = Some(input);
        } else {
            let input = cx.new(|cx| {
                SimpleInputState::new(cx)
                    .multiline()
                    .default_value(&content)
            });

            let editor_focus = input.read(cx).focus_handle(cx);
            window.focus(&editor_focus, cx);

            self.editor_input = Some(input);
        }

        self.editor_target = Some(target);
        self.editor_file_path = Some(file_path);
        self.state = KruhState::Editing;
        cx.notify();
    }

    /// Save the current editor content to disk.
    pub fn save_editor(&mut self, cx: &mut Context<Self>) -> bool {
        if let (Some(input), Some(path)) = (&self.editor_input, &self.editor_file_path) {
            let body = input.read(cx).value().to_string();

            // For issue files, reconstruct frontmatter from structured inputs
            let content = if self.editor_target == Some(EditTarget::Issue) {
                let overrides = self.read_frontmatter_inputs(cx);
                let frontmatter = overrides.to_frontmatter();
                if frontmatter.is_empty() {
                    body
                } else {
                    format!("{frontmatter}\n{body}")
                }
            } else {
                body
            };

            if std::fs::write(path, &content).is_ok() {
                return true;
            }
        }
        false
    }

    /// Read frontmatter override values from the editor inputs.
    fn read_frontmatter_inputs(&self, cx: &Context<Self>) -> KruhPlanOverrides {
        let read_opt = |input: &Option<Entity<SimpleInputState>>| -> Option<String> {
            input.as_ref().map(|e| {
                let v = e.read(cx).value().trim().to_string();
                if v.is_empty() { None } else { Some(v) }
            }).flatten()
        };

        KruhPlanOverrides {
            agent: read_opt(&self.editor_fm_agent),
            model: read_opt(&self.editor_fm_model),
            max_iterations: read_opt(&self.editor_fm_max_iters)
                .and_then(|v| v.parse::<usize>().ok()),
            sleep_secs: read_opt(&self.editor_fm_sleep)
                .and_then(|v| v.parse::<u64>().ok()),
            dangerous: read_opt(&self.editor_fm_dangerous)
                .and_then(|v| v.parse::<bool>().ok()),
        }
    }

    /// Close the editor and return to TaskBrowser, refreshing issues if needed.
    pub fn close_editor(&mut self, cx: &mut Context<Self>) {
        // Refresh issues and progress after editing any plan file
        if let Some(plan) = &self.selected_plan {
            self.issues = status_parser::load_issue_details(&plan.dir);
            if let Ok(progress) = status_parser::parse_status(&plan.dir) {
                self.progress = progress;
            }
        }

        self.editor_input = None;
        self.editor_target = None;
        self.editor_file_path = None;
        self.editor_fm_agent = None;
        self.editor_fm_model = None;
        self.editor_fm_max_iters = None;
        self.editor_fm_sleep = None;
        self.editor_fm_dangerous = None;
        self.state = KruhState::TaskBrowser;
        cx.notify();
    }

    /// Start the loop runner from the currently selected plan.
    pub fn start_loop_from_plan(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Read model from input field (agent, max_iterations, etc. are on config directly)
        self.config.model = self.model_input.read(cx).value().to_string();
        self.config.plans_dir = self.plans_dir.clone();

        // Validate docs_dir (should already be set by select_plan)
        if self.config.docs_dir.is_empty() {
            self.add_output("Error: no plan selected", true);
            cx.notify();
            return;
        }

        // Reset state for a new run
        self.state = KruhState::Running;
        self.iteration = 0;
        self.pass_count = 0;
        self.fail_count = 0;
        self.output_lines.clear();
        self.diff_stat = None;
        self.paused = false;
        self.skip_requested = false;
        self.quit_requested = false;
        self.step_advance_requested = false;
        self.start_time = Some(Instant::now());

        // Save config to layout node's app_config for persistence
        if let Ok(config_json) = serde_json::to_value(&self.config) {
            let project_id = self.project_id.clone();
            let layout_path = self.layout_path.clone();
            self.workspace.update(cx, |ws, cx| {
                ws.with_layout_node(&project_id, &layout_path, cx, |node| {
                    if let crate::workspace::state::LayoutNode::App { app_config, .. } = node {
                        *app_config = config_json;
                        return true;
                    }
                    false
                });
            });
        }

        self._loop_task = Some(loop_runner::start_loop(cx));
        self.add_output("Loop started.", false);
        cx.notify();
    }

    /// Handle directional navigation (Opt+Cmd+Arrow) — navigate to neighboring pane.
    fn handle_navigation(&mut self, direction: NavigationDirection, cx: &mut Context<Self>) {
        let pane_map = get_pane_map();

        let source = match pane_map.find_pane(&self.project_id, &self.layout_path) {
            Some(pane) => pane.clone(),
            None => return,
        };

        if let Some(target) = pane_map.find_nearest_in_direction(&source, direction) {
            self.workspace.update(cx, |ws, cx| {
                ws.set_focused_terminal(
                    target.project_id.clone(),
                    target.layout_path.clone(),
                    cx,
                );
            });
        }
    }

    /// Handle sequential navigation (Ctrl+Tab / Ctrl+Shift+Tab).
    fn handle_sequential_navigation(&mut self, next: bool, cx: &mut Context<Self>) {
        let pane_map = get_pane_map();

        let source = match pane_map.find_pane(&self.project_id, &self.layout_path) {
            Some(pane) => pane.clone(),
            None => return,
        };

        let target = if next {
            pane_map.find_next_pane(&source)
        } else {
            pane_map.find_prev_pane(&source)
        };

        if let Some(target) = target {
            self.workspace.update(cx, |ws, cx| {
                ws.set_focused_terminal(
                    target.project_id.clone(),
                    target.layout_path.clone(),
                    cx,
                );
            });
        }
    }
}

impl KruhPane {
    /// Append an output line to the display.
    pub fn add_output(&mut self, text: &str, is_error: bool) {
        self.output_lines.push(OutputLine {
            text: text.to_string(),
            timestamp: Instant::now(),
            is_error,
        });
    }
}

impl EventEmitter<KruhPaneEvent> for KruhPane {}

impl Render for KruhPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.focus_handle.clone();

        // Check if FocusManager says we should be focused and restore GPUI focus.
        // Use contains_focused so we don't steal focus from child inputs (Settings, Editing).
        let should_focus = {
            let ws = self.workspace.read(cx);
            if let Some(focused) = ws.focus_manager.focused_terminal_state() {
                focused.project_id == self.project_id
                    && focused.layout_path == self.layout_path
                    && !focus_handle.contains_focused(window, cx)
                    && !ws.focus_manager.is_modal()
            } else {
                false
            }
        };
        if should_focus {
            if self.state == KruhState::Editing {
                if let Some(input) = &self.editor_input {
                    let editor_focus = input.read(cx).focus_handle(cx);
                    window.focus(&editor_focus, cx);
                }
            } else {
                window.focus(&focus_handle, cx);
            }
        }

        // Register pane bounds for spatial navigation (same pattern as TerminalPane)
        let bounds_setter = {
            let project_id = self.project_id.clone();
            let layout_path = self.layout_path.clone();
            move |bounds: Bounds<Pixels>, _window: &mut Window, _cx: &mut App| {
                register_pane_bounds(project_id.clone(), layout_path.clone(), bounds);
            }
        };

        div()
            .id("kruh-pane-root")
            .size_full()
            .min_h_0()
            .track_focus(&focus_handle)
            .key_context("TerminalPane")
            // Register bounds for spatial navigation
            .child(canvas(bounds_setter, |_, _, _, _| {}).absolute().size_full())
            // Click to focus — blurs terminal panes
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.state == KruhState::Editing {
                        if let Some(input) = &this.editor_input {
                            let editor_focus = input.read(cx).focus_handle(cx);
                            window.focus(&editor_focus, cx);
                        }
                    } else if this.state == KruhState::Settings {
                        // Don't steal focus from inputs — let SimpleInput/PathAutoComplete
                        // handle focus via their own mouse_down handlers.
                    } else {
                        window.focus(&this.focus_handle, cx);
                    }
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_focused_terminal(
                            this.project_id.clone(),
                            this.layout_path.clone(),
                            cx,
                        );
                    });
                }),
            )
            // Directional navigation actions (Opt+Cmd+Arrow)
            .on_action(cx.listener(|this, _: &FocusLeft, _, cx| {
                this.handle_navigation(NavigationDirection::Left, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusRight, _, cx| {
                this.handle_navigation(NavigationDirection::Right, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusUp, _, cx| {
                this.handle_navigation(NavigationDirection::Up, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusDown, _, cx| {
                this.handle_navigation(NavigationDirection::Down, cx);
            }))
            // Sequential navigation (Ctrl+Tab)
            .on_action(cx.listener(|this, _: &FocusNextTerminal, _, cx| {
                this.handle_sequential_navigation(true, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrevTerminal, _, cx| {
                this.handle_sequential_navigation(false, cx);
            }))
            // Close action (Cmd+W)
            .on_action(cx.listener(|_this, _: &CloseTerminal, _, _cx| {
                // TODO: handle close if needed
            }))
            // Keyboard handler for app-specific keys
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key_event(event, window, cx);
            }))
            .child(self.render_view(window, cx))
    }
}

impl Focusable for KruhPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for KruhPane {
    fn drop(&mut self) {
        // Kill any running agent subprocess
        if let Some(mut handle) = self.agent_handle.take() {
            handle.kill();
        }
        // _loop_task is dropped automatically, which cancels the async task
    }
}
