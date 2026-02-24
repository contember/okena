pub mod agent;
pub mod agent_instructions;
pub mod config;
pub mod git;
pub mod loop_runner;
pub mod render;
pub mod status_parser;
pub mod types;

use std::path::Path;
use std::sync::Arc;

use gpui::*;

use crate::keybindings::{
    CloseTerminal, FocusDown, FocusLeft, FocusNextTerminal, FocusPrevTerminal, FocusRight, FocusUp,
};
use crate::remote::app_broadcaster::AppStateBroadcaster;
use crate::views::components::simple_input::SimpleInputState;
use crate::views::layout::app_entity_registry::{
    AppEntityHandle, AppEntityRegistry, GlobalAppEntityRegistry,
};
use crate::views::layout::navigation::{
    get_pane_map, register_pane_bounds, NavigationDirection,
};
use crate::workspace::state::Workspace;
use config::{KruhConfig, KruhPlanOverrides};
use types::{
    EditTarget, IssueDetail, IssueViewInfo, KruhAction, KruhPaneEvent, KruhScreen, KruhState,
    KruhViewState, LoopInstance, LoopState, LoopViewInfo, OutputLineView, PlanInfo, PlanViewInfo,
    ProgressViewInfo,
};

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

    // Multi-loop state
    pub active_loops: Vec<LoopInstance>,
    pub loop_tasks: Vec<(usize, Task<()>)>, // (loop_id, task)
    pub focused_loop_index: usize,
    pub next_loop_id: usize,

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
    pub editor_scroll: ScrollHandle,
    pub editor_input: Option<Entity<SimpleInputState>>,
    pub editor_target: Option<EditTarget>,
    pub editor_file_path: Option<String>,

    // Frontmatter editor (active when editing INSTRUCTIONS.md)
    pub editor_fm_agent: Option<Entity<SimpleInputState>>,
    pub editor_fm_model: Option<Entity<SimpleInputState>>,
    pub editor_fm_max_iters: Option<Entity<SimpleInputState>>,
    pub editor_fm_sleep: Option<Entity<SimpleInputState>>,
    pub editor_fm_dangerous: Option<Entity<SimpleInputState>>,
    pub editor_fm_extra: Vec<(String, String)>,

    // Async scan task
    pub _scan_task: Option<Task<()>>,

    // Remote state publishing
    pub app_broadcaster: Option<Arc<AppStateBroadcaster>>,
    pub publish_timer: Option<Task<()>>,

    // App entity registry (kept for unregister in Drop)
    pub app_registry: Option<Arc<AppEntityRegistry>>,
}

impl KruhPane {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        project_path: String,
        layout_path: Vec<usize>,
        app_id: Option<String>,
        config: KruhConfig,
        app_broadcaster: Option<Arc<AppStateBroadcaster>>,
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
            active_loops: Vec::new(),
            loop_tasks: Vec::new(),
            focused_loop_index: 0,
            next_loop_id: 0,
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
            editor_scroll: ScrollHandle::new(),
            editor_input: None,
            editor_target: None,
            editor_file_path: None,
            editor_fm_agent: None,
            editor_fm_model: None,
            editor_fm_max_iters: None,
            editor_fm_sleep: None,
            editor_fm_dangerous: None,
            editor_fm_extra: Vec::new(),
            _scan_task: None,
            app_broadcaster,
            publish_timer: None,
            app_registry: None,
        };

        // Publish state on every notify (debounced 100ms)
        if pane.app_broadcaster.is_some() {
            cx.observe_self(|this, cx| {
                this.schedule_state_publish(cx);
            })
            .detach();
        }

        pane.start_scan(cx);

        // Register with the app entity registry if available (enables remote control)
        if let Some(registry) = cx.try_global::<GlobalAppEntityRegistry>().map(|g| g.0.clone()) {
            if let Some(ref app_id) = pane.app_id {
                let entity = cx.entity().downgrade();
                let entity_for_action = entity.clone();
                registry.register(
                    app_id.clone(),
                    AppEntityHandle {
                        app_kind: "kruh".to_string(),
                        view_state: Arc::new(move |cx| {
                            entity
                                .update(cx, |pane, cx| {
                                    serde_json::to_value(pane.view_state(cx)).ok()
                                })
                                .ok()
                                .flatten()
                        }),
                        handle_action: Arc::new(move |action, cx| {
                            let action: KruhAction = serde_json::from_value(action)
                                .map_err(|e| format!("Invalid action: {}", e))?;
                            entity_for_action
                                .update(cx, |pane, cx| {
                                    pane.handle_action(action, cx);
                                })
                                .map_err(|_| "Entity released".to_string())
                        }),
                    },
                );
                pane.app_registry = Some(registry);
            }
        }

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

            // Ensure shared INSTRUCTIONS.md exists with default agent guidelines
            let shared_instructions =
                std::path::Path::new(&self.plans_dir).join("INSTRUCTIONS.md");
            if Path::new(&self.plans_dir).is_dir() && !shared_instructions.exists() {
                let _ = std::fs::write(
                    &shared_instructions,
                    agent_instructions::DEFAULT_AGENT_INSTRUCTIONS,
                );
            }

            // Load issues
            let docs_dir = plan.dir.clone();
            self.issues = status_parser::load_issue_details(&docs_dir);

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
            EditTarget::Instructions => {
                status_parser::resolve_instructions_path(docs_dir, &self.plans_dir)
            }
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
            self.editor_fm_extra = status_parser::extract_extra_frontmatter_keys(&content);
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

            // For issue files, reconstruct frontmatter from structured inputs,
            // merging in extra (run metadata) keys preserved when the editor was opened.
            let content = if self.editor_target == Some(EditTarget::Issue) {
                let overrides = self.read_frontmatter_inputs(cx);
                // Collect config overrides + extra metadata as ordered key-value pairs
                let mut owned: Vec<(String, String)> = Vec::new();
                if let Some(ref a) = overrides.agent {
                    owned.push(("agent".to_string(), a.clone()));
                }
                if let Some(ref m) = overrides.model {
                    owned.push(("model".to_string(), m.clone()));
                }
                if let Some(n) = overrides.max_iterations {
                    owned.push(("max_iterations".to_string(), n.to_string()));
                }
                if let Some(s) = overrides.sleep_secs {
                    owned.push(("sleep_secs".to_string(), s.to_string()));
                }
                if let Some(d) = overrides.dangerous {
                    owned.push(("dangerous".to_string(), d.to_string()));
                }
                for (k, v) in &self.editor_fm_extra {
                    owned.push((k.clone(), v.clone()));
                }
                if owned.is_empty() {
                    body
                } else {
                    let updates: Vec<(&str, &str)> =
                        owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                    status_parser::update_frontmatter_content(&body, &updates)
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
        // Refresh issues after editing any plan file
        if let Some(plan) = &self.selected_plan {
            self.issues = status_parser::load_issue_details(&plan.dir);
        }

        self.editor_input = None;
        self.editor_target = None;
        self.editor_file_path = None;
        self.editor_fm_agent = None;
        self.editor_fm_model = None;
        self.editor_fm_max_iters = None;
        self.editor_fm_sleep = None;
        self.editor_fm_dangerous = None;
        self.editor_fm_extra = Vec::new();
        self.state = KruhState::TaskBrowser;
        cx.notify();
    }

    /// Start the loop runner from the currently selected plan.
    pub fn start_loop_from_plan(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Read model from input field (agent, max_iterations, etc. are on config directly)
        self.config.model = self.model_input.read(cx).value().to_string();
        self.config.plans_dir = self.plans_dir.clone();

        let plan = match &self.selected_plan {
            Some(p) => p.clone(),
            None => {
                cx.notify();
                return;
            }
        };

        // Validate docs_dir (should already be set by select_plan)
        if self.config.docs_dir.is_empty() {
            cx.notify();
            return;
        }

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

        // Create loop instance
        let loop_id = self.next_loop_id;
        self.next_loop_id += 1;
        let mut config = self.config.clone();
        config.docs_dir = plan.dir.clone();
        let instance = LoopInstance::new(loop_id, plan, config);
        self.active_loops.push(instance);
        self.focused_loop_index = self.active_loops.len() - 1;

        // Start the loop task
        let task = loop_runner::start_loop(loop_id, cx);
        self.loop_tasks.push((loop_id, task));

        self.state = KruhState::LoopOverview;
        cx.notify();
    }

    /// Start loops for all plans that have pending tasks.
    pub fn start_all_loops(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.config.model = self.model_input.read(cx).value().to_string();
        self.config.plans_dir = self.plans_dir.clone();

        // Save config
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

        let plans: Vec<_> = self.plans.iter()
            .filter(|p| p.pending > 0)
            .cloned()
            .collect();

        if plans.is_empty() {
            return;
        }

        for plan in plans {
            let loop_id = self.next_loop_id;
            self.next_loop_id += 1;
            let mut config = self.config.clone();
            config.docs_dir = plan.dir.clone();
            let instance = LoopInstance::new(loop_id, plan, config);
            self.active_loops.push(instance);

            let task = loop_runner::start_loop(loop_id, cx);
            self.loop_tasks.push((loop_id, task));
        }

        self.focused_loop_index = 0;
        self.state = KruhState::LoopOverview;
        cx.notify();
    }

    /// Get a mutable reference to a loop instance by ID.
    pub fn loop_mut(&mut self, id: usize) -> Option<&mut LoopInstance> {
        self.active_loops.iter_mut().find(|l| l.id == id)
    }

    /// Get an immutable reference to a loop instance by ID.
    pub fn loop_ref(&self, id: usize) -> Option<&LoopInstance> {
        self.active_loops.iter().find(|l| l.id == id)
    }

    /// Get the currently focused loop instance, if any.
    pub fn focused_loop(&self) -> Option<&LoopInstance> {
        self.active_loops.get(self.focused_loop_index)
    }

    /// Check if all loops are completed.
    pub fn all_loops_completed(&self) -> bool {
        !self.active_loops.is_empty()
            && self.active_loops.iter().all(|l| l.state == LoopState::Completed)
    }

    /// Reset loop state and go back to plan picker.
    pub fn close_loops(&mut self, cx: &mut Context<Self>) {
        // Signal all loops to quit
        for l in &mut self.active_loops {
            l.quit_requested = true;
        }
        // Drop all tasks (cancels async loops)
        self.loop_tasks.clear();
        self.active_loops.clear();
        self.focused_loop_index = 0;
        self.selected_plan = None;
        self.state = KruhState::PlanPicker;
        // Re-scan to refresh plan status
        self.start_scan(cx);
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
    /// Dispatch a `KruhAction` to the corresponding mutation method.
    pub fn handle_action(&mut self, action: KruhAction, cx: &mut Context<Self>) {
        match action {
            KruhAction::StartScan => self.start_scan(cx),
            KruhAction::SelectPlan { index } => self.select_plan(index, cx),
            KruhAction::OpenPlan { name } => self.open_plan(&name, cx),
            KruhAction::BackToPlans => self.back_to_plans(cx),
            KruhAction::StartLoop { plan_name } => self.start_loop_for_name(&plan_name, cx),
            KruhAction::StartAllLoops => self.start_all_loops_headless(cx),
            KruhAction::PauseLoop { loop_id } => self.pause_loop(loop_id, cx),
            KruhAction::ResumeLoop { loop_id } => self.resume_loop(loop_id, cx),
            KruhAction::StopLoop { loop_id } => self.stop_loop(loop_id, cx),
            KruhAction::CloseLoops => self.close_loops(cx),
            KruhAction::FocusLoop { index } => self.focus_loop(index, cx),
            KruhAction::OpenEditor { file_path } => self.open_editor_for_path(&file_path, cx),
            KruhAction::SaveEditor { content } => self.save_editor_content(&content, cx),
            KruhAction::CloseEditor => self.close_editor(cx),
            KruhAction::OpenSettings => self.open_settings(cx),
            KruhAction::UpdateSettings { model, max_iterations, auto_start } => {
                self.update_settings(model, max_iterations, auto_start, cx);
            }
            KruhAction::CloseSettings => self.close_settings(cx),
            KruhAction::BrowseTasks { plan_name } => self.browse_tasks(&plan_name, cx),
        }
    }

    /// Navigate to TaskBrowser for a plan identified by name.
    pub fn open_plan(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(index) = self.plans.iter().position(|p| p.name == name) {
            self.select_plan(index, cx);
        }
    }

    /// Navigate back to PlanPicker.
    pub fn back_to_plans(&mut self, cx: &mut Context<Self>) {
        self.navigate_to_plan_picker(cx);
    }

    /// Start a loop for a plan identified by name (remote / windowless path).
    pub fn start_loop_for_name(&mut self, plan_name: &str, cx: &mut Context<Self>) {
        if self.selected_plan.as_ref().map(|p| p.name.as_str()) != Some(plan_name) {
            if let Some(idx) = self.plans.iter().position(|p| p.name == plan_name) {
                self.select_plan(idx, cx);
            } else {
                return;
            }
        }
        let plan = match self.selected_plan.clone() {
            Some(p) => p,
            None => return,
        };
        self.config.plans_dir = self.plans_dir.clone();
        let loop_id = self.next_loop_id;
        self.next_loop_id += 1;
        let mut config = self.config.clone();
        config.docs_dir = plan.dir.clone();
        let instance = LoopInstance::new(loop_id, plan, config);
        self.active_loops.push(instance);
        self.focused_loop_index = self.active_loops.len() - 1;
        let task = loop_runner::start_loop(loop_id, cx);
        self.loop_tasks.push((loop_id, task));
        self.state = KruhState::LoopOverview;
        cx.notify();
    }

    /// Start loops for all plans with pending tasks (remote / windowless path).
    pub fn start_all_loops_headless(&mut self, cx: &mut Context<Self>) {
        self.config.plans_dir = self.plans_dir.clone();
        let plans: Vec<_> = self.plans.iter().filter(|p| p.pending > 0).cloned().collect();
        if plans.is_empty() {
            return;
        }
        for plan in plans {
            let loop_id = self.next_loop_id;
            self.next_loop_id += 1;
            let mut config = self.config.clone();
            config.docs_dir = plan.dir.clone();
            let instance = LoopInstance::new(loop_id, plan, config);
            self.active_loops.push(instance);
            let task = loop_runner::start_loop(loop_id, cx);
            self.loop_tasks.push((loop_id, task));
        }
        self.focused_loop_index = 0;
        self.state = KruhState::LoopOverview;
        cx.notify();
    }

    /// Pause a running loop by ID.
    pub fn pause_loop(&mut self, loop_id: usize, cx: &mut Context<Self>) {
        if let Some(instance) = self.loop_mut(loop_id) {
            instance.paused = true;
            instance.state = LoopState::Paused;
        }
        cx.notify();
    }

    /// Resume a paused loop by ID.
    pub fn resume_loop(&mut self, loop_id: usize, cx: &mut Context<Self>) {
        if let Some(instance) = self.loop_mut(loop_id) {
            instance.paused = false;
            instance.state = LoopState::Running;
        }
        cx.notify();
    }

    /// Request a loop to stop by ID.
    pub fn stop_loop(&mut self, loop_id: usize, cx: &mut Context<Self>) {
        if let Some(instance) = self.loop_mut(loop_id) {
            instance.quit_requested = true;
        }
        cx.notify();
    }

    /// Set the focused loop by index.
    pub fn focus_loop(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.active_loops.len() {
            self.focused_loop_index = index;
            cx.notify();
        }
    }

    /// Open a file for editing by its absolute path (remote / windowless path).
    pub fn open_editor_for_path(&mut self, file_path: &str, cx: &mut Context<Self>) {
        let content = std::fs::read_to_string(file_path).unwrap_or_default();
        let input = cx.new(|cx| {
            SimpleInputState::new(cx).multiline().default_value(&content)
        });
        self.editor_input = Some(input);
        self.editor_target = Some(EditTarget::Status);
        self.editor_file_path = Some(file_path.to_string());
        self.state = KruhState::Editing;
        cx.notify();
    }

    /// Write content directly to the current editor file path.
    pub fn save_editor_content(&mut self, content: &str, _cx: &mut Context<Self>) {
        if let Some(path) = &self.editor_file_path {
            let _ = std::fs::write(path, content);
        }
    }

    /// Transition to the Settings screen.
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.state = KruhState::Settings;
        cx.notify();
    }

    /// Apply new settings values.
    pub fn update_settings(
        &mut self,
        model: String,
        max_iterations: usize,
        _auto_start: bool,
        cx: &mut Context<Self>,
    ) {
        self.config.model = model;
        self.config.max_iterations = max_iterations;
        cx.notify();
    }

    /// Close the Settings screen and return to PlanPicker.
    pub fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.state = KruhState::PlanPicker;
        cx.notify();
    }

    /// Navigate to TaskBrowser for a plan identified by name.
    pub fn browse_tasks(&mut self, plan_name: &str, cx: &mut Context<Self>) {
        self.open_plan(plan_name, cx);
    }
}

impl KruhPane {
    /// Schedule a debounced (100ms) publish of the current view state to the broadcaster.
    ///
    /// Drops any pending timer, so rapid notify calls coalesce into a single publish.
    fn schedule_state_publish(&mut self, cx: &mut Context<Self>) {
        let broadcaster = match self.app_broadcaster.clone() {
            Some(b) => b,
            None => return,
        };
        let app_id = match self.app_id.clone() {
            Some(id) => id,
            None => return,
        };

        self.publish_timer = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(std::time::Duration::from_millis(100)).await;
            let _ = this.update(cx, |pane, cx| {
                let state = pane.view_state(cx);
                if let Ok(json) = serde_json::to_value(&state) {
                    broadcaster.publish(app_id, "kruh".to_string(), json);
                }
            });
        }));
    }

    /// Append an output line to a specific loop's display and auto-scroll to bottom.
    pub fn add_loop_output(&mut self, loop_id: usize, text: &str, is_error: bool) {
        if let Some(instance) = self.loop_mut(loop_id) {
            instance.add_output(text, is_error);
        }
    }

    /// Snapshot KruhPane into a pure-data `KruhViewState` suitable for remote rendering.
    ///
    /// No GPUI handles or `Instant` values appear in the output.
    pub fn view_state(&self, cx: &Context<Self>) -> KruhViewState {
        let screen = match &self.state {
            KruhState::Scanning => KruhScreen::Scanning,
            KruhState::PlanPicker => KruhScreen::PlanPicker {
                plans: self
                    .plans
                    .iter()
                    .map(|p| PlanViewInfo {
                        name: p.name.clone(),
                        path: p.dir.clone(),
                        issue_count: p.total,
                        completed_count: p.done,
                    })
                    .collect(),
                selected_index: self.selected_plan_index,
            },
            KruhState::TaskBrowser => KruhScreen::TaskBrowser {
                plan_name: self
                    .selected_plan
                    .as_ref()
                    .map(|p| p.name.clone())
                    .unwrap_or_default(),
                issues: self
                    .issues
                    .iter()
                    .map(|issue| IssueViewInfo {
                        number: issue.ref_info.number.clone(),
                        title: issue.ref_info.name.clone(),
                        status: if issue.done {
                            "completed".to_string()
                        } else {
                            "pending".to_string()
                        },
                        priority: None,
                    })
                    .collect(),
            },
            KruhState::Editing => KruhScreen::Editing {
                file_path: self.editor_file_path.clone().unwrap_or_default(),
                content: self
                    .editor_input
                    .as_ref()
                    .map(|e| e.read(cx).value().to_string())
                    .unwrap_or_default(),
                is_new: false,
            },
            KruhState::Settings => KruhScreen::Settings {
                model: self.config.model.clone(),
                max_iterations: self.config.max_iterations,
                auto_start: false,
            },
            KruhState::LoopOverview => KruhScreen::LoopOverview {
                loops: self
                    .active_loops
                    .iter()
                    .map(|l| LoopViewInfo {
                        loop_id: l.id,
                        plan_name: l.plan.name.clone(),
                        phase: format!("{}", l.loop_phase),
                        state: match l.state {
                            LoopState::Running => "running".to_string(),
                            LoopState::Paused => "paused".to_string(),
                            LoopState::WaitingForStep => "waiting".to_string(),
                            LoopState::Completed => "completed".to_string(),
                        },
                        current_issue: l.current_issue_name.clone(),
                        progress: ProgressViewInfo {
                            completed: l.progress.done,
                            total: l.progress.total,
                        },
                        // Cap output at last 200 lines
                        output_lines: l
                            .output_lines
                            .iter()
                            .rev()
                            .take(200)
                            .rev()
                            .map(|o| OutputLineView {
                                text: o.text.clone(),
                                is_error: o.is_error,
                            })
                            .collect(),
                    })
                    .collect(),
                focused_index: self.focused_loop_index,
            },
        };

        KruhViewState { app_id: self.app_id.clone(), screen }
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
                        // Don't steal focus from frontmatter inputs —
                        // let SimpleInput handle focus via its own mouse_down handler.
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
        // Signal all loops to quit so agents get killed in their async tasks
        for l in &mut self.active_loops {
            l.quit_requested = true;
        }
        // loop_tasks are dropped automatically, which cancels the async tasks

        // Unregister from the app entity registry
        if let (Some(registry), Some(app_id)) = (&self.app_registry, &self.app_id) {
            registry.unregister(app_id);
        }
    }
}
