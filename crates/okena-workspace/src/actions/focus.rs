//! Focus and fullscreen workspace actions
//!
//! Actions for managing terminal and project focus, including fullscreen mode.
//!
//! Per slice 03 of the multi-window plan, `FocusManager` is owned per-window
//! by `WindowView` rather than as a field on the `Workspace` entity. These
//! action methods take `focus_manager: &mut FocusManager` as a parameter so
//! every caller threads in the focus state belonging to the window driving
//! the action -- mutations stay scoped to that window.

use crate::context::WorkspaceCx;
use crate::focus::{FocusManager, FocusTarget};
use crate::state::{WindowId, Workspace};

impl Workspace {
    /// Set focused project (focus mode)
    ///
    /// This zooms the main view to show only this project.
    /// Also focuses the first terminal in the project if one exists.
    /// If the project has no layout, drills into the first worktree child.
    pub fn set_focused_project(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: Option<String>,
        cx: &mut impl WorkspaceCx,
    ) {
        // Clear fullscreen without restoring old project_id (we're overriding it)
        focus_manager.clear_fullscreen_without_restore();

        // Set the focused project via FocusManager (controls main view zoom)
        focus_manager.set_focused_project_id(project_id.clone());

        // Focus the first terminal in the project
        if let Some(ref pid) = project_id {
            self.focus_first_terminal_in(focus_manager, pid);
        }

        cx.notify();
    }

    /// Set focused project in individual mode (show only this project, not its worktree children).
    /// Used when clicking a "main worktree" entry in the sidebar.
    pub fn set_focused_project_individual(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: Option<String>,
        cx: &mut impl WorkspaceCx,
    ) {
        focus_manager.clear_fullscreen_without_restore();
        focus_manager.set_focused_project_id_individual(project_id.clone());

        if let Some(ref pid) = project_id {
            self.focus_first_terminal_in(focus_manager, pid);
        }

        cx.notify();
    }

    /// Toggle folder selection on the targeted window: sets the window's
    /// folder filter and focuses the first terminal inside the folder.
    /// If the folder is already selected (per the targeted window's filter),
    /// deselects it.
    ///
    /// Reads the current filter via
    /// `active_folder_filter(window_id)` and delegates the mutation to
    /// `set_folder_filter(window_id, ...)` so both legs route through the
    /// targeted window's `WindowState::folder_filter`. Unknown extra ids
    /// are a silent no-op (close-race contract inherited from both the
    /// reader and the setter).
    pub fn toggle_folder_focus(
        &mut self,
        focus_manager: &mut FocusManager,
        window_id: WindowId,
        folder_id: &str,
        cx: &mut impl WorkspaceCx,
    ) {
        let selecting = self.active_folder_filter(window_id).map(|s| s.as_str()) != Some(folder_id);
        if selecting {
            self.set_folder_filter(window_id, Some(folder_id.to_string()), cx);
            // Clear project focus so all visible folder projects show
            focus_manager.set_focused_project_id(None);
            // Focus the first project's terminal
            if let Some(first_pid) = self
                .folder(folder_id)
                .and_then(|f| f.project_ids.first())
                .cloned()
            {
                self.focus_first_terminal_in(focus_manager, &first_pid);
            }
        } else {
            self.set_folder_filter(window_id, None, cx);
        }
        cx.notify();
    }

    /// Resolve the first visible terminal in a project (or its worktree
    /// children) as a [`FocusTarget`], mirroring `focus_first_terminal_in`'s
    /// candidate order. Returns `None` when neither the project nor any of its
    /// worktree children has a layout.
    ///
    /// Unlike `focus_first_terminal_in`, this does NOT touch the
    /// `FocusManager`, so callers that must preserve the current focus context
    /// (e.g. redirecting a modal's restore target) can use it without flipping
    /// out of `Modal`/`Fullscreen`.
    pub(crate) fn first_terminal_target_in(&self, project_id: &str) -> Option<FocusTarget> {
        // Try the project itself first, then its worktree children
        let candidates =
            std::iter::once(project_id.to_string()).chain(self.worktree_child_ids(project_id));
        for id in candidates {
            if let Some(project) = self.project(&id)
                && let Some(layout) = project.layout.as_ref()
            {
                // Follow active tabs to the currently visible terminal
                let path = layout.find_visible_terminal_path();
                return Some(FocusTarget::new(id, path));
            }
        }
        None
    }

    /// Resolve a focusable project and focus its first terminal.
    ///
    /// If the project has no layout (e.g. only worktree children), drills into
    /// the first worktree child that has a terminal.
    pub(crate) fn focus_first_terminal_in(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
    ) {
        if let Some(target) = self.first_terminal_target_in(project_id) {
            focus_manager.focus_terminal(target.project_id, target.layout_path);
        }
    }

    /// Enter fullscreen mode for a terminal
    pub fn set_fullscreen_terminal(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: String,
        terminal_id: String,
        cx: &mut impl WorkspaceCx,
    ) {
        log::info!(
            "set_fullscreen_terminal called with project_id={}, terminal_id={}",
            project_id,
            terminal_id
        );

        // Find the layout path for this terminal
        let layout_path = self
            .project(&project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.find_terminal_path(&terminal_id))
            .unwrap_or_default();

        log::info!("layout_path for terminal: {:?}", layout_path);

        // Use FocusManager for fullscreen entry (saves current state + sets focused_project_id)
        focus_manager.enter_fullscreen(project_id, layout_path, terminal_id.clone());

        log::info!(
            "fullscreen_terminal set via FocusManager with terminal_id={}",
            terminal_id
        );

        cx.notify();
    }

    /// Exit fullscreen mode
    ///
    /// Restores focus to the previously focused terminal and project view mode.
    pub fn exit_fullscreen(&mut self, focus_manager: &mut FocusManager, cx: &mut impl WorkspaceCx) {
        focus_manager.exit_fullscreen();
        cx.notify();
    }

    /// Set focused terminal (for visual indicator)
    ///
    /// Focus events propagate: terminal focus -> pane focus -> project awareness
    pub fn set_focused_terminal(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: String,
        layout_path: Vec<usize>,
        cx: &mut impl WorkspaceCx,
    ) {
        // Update FocusManager
        focus_manager.focus_terminal(project_id.clone(), layout_path.clone());

        // Record project access time for recency sorting, and stamp activity
        // so the activity-sorted sidebar surfaces the project the user just
        // moved into. `bump_activity` already notifies + persists (debounced).
        self.touch_project(&project_id);
        self.bump_activity(&project_id, cx);

        cx.notify();
    }

    /// Clear focused terminal
    ///
    /// This is typically called when entering a modal context (search, rename, etc.)
    /// The current focus is saved for restoration when the modal closes.
    pub fn clear_focused_terminal(
        &mut self,
        focus_manager: &mut FocusManager,
        cx: &mut impl WorkspaceCx,
    ) {
        focus_manager.enter_modal();
        cx.notify();
    }

    /// Restore focused terminal after modal dismissal
    ///
    /// Called when exiting a modal context to restore the previous focus.
    pub fn restore_focused_terminal(
        &mut self,
        focus_manager: &mut FocusManager,
        cx: &mut impl WorkspaceCx,
    ) {
        focus_manager.exit_modal();
        cx.notify();
    }

    /// Focus a terminal by its ID, *revealing* it first (finds path automatically).
    ///
    /// This is the shared "navigate to this terminal" primitive behind agent-row
    /// / sidebar / notification clicks and remote focus requests. It activates
    /// the tabs along the path, then calls [`FocusManager::reveal_terminal`] so
    /// the target becomes visible even when the view is currently zoomed into
    /// another project or has a different terminal fullscreened — retargeting
    /// the active view mode onto the target rather than focusing a pane the
    /// current view is hiding. (Use `set_focused_terminal` for clicks on a pane
    /// that's already on screen.)
    /// The one navigation operation behind every "jump to this terminal":
    /// agent-row and sidebar clicks, cursor navigation, notification jumps,
    /// remote focus requests.
    ///
    /// Window-aware on purpose. Callers used to decide for themselves whether
    /// the target needed revealing and pre-emptively zoom before calling — which
    /// both duplicated the rule and broke fullscreen, since changing the focused
    /// project cancels it *before* the reveal below can retarget it. Deciding
    /// here keeps the whole jump one step.
    pub fn focus_terminal_by_id(
        &mut self,
        focus_manager: &mut FocusManager,
        window_id: WindowId,
        project_id: &str,
        terminal_id: &str,
        cx: &mut impl WorkspaceCx,
    ) {
        if let Some(project) = self.project(project_id)
            && let Some(ref layout) = project.layout
            && let Some(path) = layout.find_terminal_path(terminal_id)
        {
            // Whether this window renders the target's column at all. A project
            // in the hidden set or outside the active folder filter does not, so
            // the reveal has to zoom instead of just moving focus.
            let project_offscreen = !self
                .visible_projects(
                    window_id,
                    focus_manager.focused_project_id(),
                    focus_manager.is_focus_individual(),
                )
                .iter()
                .any(|p| p.id == project_id);

            // Activate any tabs along the path so the terminal becomes visible
            if let Some(project_mut) = self.project_mut(project_id)
                && let Some(ref mut layout) = project_mut.layout
            {
                layout.activate_tabs_along_path(&path);
            }
            self.notify_data(cx);
            // Reveal: retarget the active zoom/fullscreen onto the target so it
            // is actually visible, then focus it.
            focus_manager.reveal_terminal(
                project_id.to_string(),
                path,
                terminal_id.to_string(),
                project_offscreen,
            );
            self.touch_project(project_id);
            self.bump_activity(project_id, cx);
            cx.notify();
        }
    }
}

#[cfg(all(test, feature = "gpui"))]
mod gpui_tests {
    use crate::focus::FocusManager;
    use crate::state::{FolderData, WindowId, WindowState, Workspace, WorkspaceData};
    use gpui::AppContext as _;
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn make_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: vec![],
            project_order: vec![],
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: vec![FolderData {
                id: "f1".to_string(),
                name: "Folder".to_string(),
                project_ids: vec![],
                folder_color: FolderColor::default(),
            }],
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    #[gpui::test]
    fn toggle_folder_focus_main_writes_to_main_folder_filter(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: targeting WindowId::Main flips
        // main_window.folder_filter through Workspace::set_folder_filter.
        // First toggle: None -> Some("f1"). Second toggle (selecting state
        // computed against main, which now matches): Some("f1") -> None.
        // Pins the round-trip semantic on the main path, byte-for-byte
        // identical to the pre-migration shape.
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));
        let mut fm = FocusManager::new();

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_focus(&mut fm, WindowId::Main, "f1", cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().main_window.folder_filter.as_deref(), Some("f1"));
        });

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_focus(&mut fm, WindowId::Main, "f1", cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.folder_filter.is_none());
        });
    }

    #[gpui::test]
    fn toggle_folder_focus_extra_writes_only_to_targeted_window(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: targeting WindowId::Extra(uuid) writes
        // to that extra's folder_filter -- main and any sibling extras stay
        // untouched. Defends against a regression that ignores window_id and
        // unconditionally writes to main, scatters the write across all
        // extras, or routes through main's slot. Pre-populate sibling extra
        // with a non-default filter to verify isolation.
        let mut data = make_workspace_data();
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = WindowState {
            folder_filter: Some("f1".to_string()),
            ..Default::default()
        };
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];
        let workspace = cx.new(|_cx| Workspace::new(data));
        let mut fm = FocusManager::new();

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_focus(&mut fm, WindowId::Extra(extra_a_id), "f1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Targeted extra got the filter.
            assert_eq!(
                ws.data().extra_windows[0].folder_filter.as_deref(),
                Some("f1")
            );
            // Main stays untouched.
            assert!(ws.data().main_window.folder_filter.is_none());
            // Sibling extra's pre-existing filter is preserved.
            assert_eq!(
                ws.data().extra_windows[1].folder_filter.as_deref(),
                Some("f1")
            );
            assert_eq!(extra_b_id, ws.data().extra_windows[1].id);
        });
    }

    #[gpui::test]
    fn toggle_folder_focus_extra_round_trip_uses_extras_own_filter(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: with `active_folder_filter` migrated to
        // take a WindowId, the SELECT/DESELECT round trip on an extra reads
        // and writes the extra's own slot. First toggle on extra_a:
        // None -> Some("f1") (selecting=true since extra_a's filter is
        // None). Second toggle on the same extra: Some("f1") -> None
        // (selecting=false since extra_a's filter now matches). Defends
        // against a regression that re-introduces a main-only reader and
        // makes every extra's first toggle compute selecting=true.
        let mut data = make_workspace_data();
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        data.extra_windows = vec![extra_a];
        let workspace = cx.new(|_cx| Workspace::new(data));
        let mut fm = FocusManager::new();

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_focus(&mut fm, WindowId::Extra(extra_a_id), "f1", cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(
                ws.data().extra_windows[0].folder_filter.as_deref(),
                Some("f1")
            );
        });

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_focus(&mut fm, WindowId::Extra(extra_a_id), "f1", cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().extra_windows[0].folder_filter.is_none());
        });
    }

    #[gpui::test]
    fn toggle_folder_focus_unknown_extra_is_silent_noop(cx: &mut gpui::TestAppContext) {
        // Close-race contract: a fresh uuid that does not match any extra
        // produces no panic; main_window stays untouched. Pre-populate main
        // with an existing filter to ensure the unknown-extra path does NOT
        // silently fall back to main as a default. data_version still bumps
        // via notify_data, matching the silent-no-op contract on the
        // data-layer setter (the entity-level set_folder_filter wrapper
        // unconditionally calls notify_data after delegating to the data
        // layer's window_mut lookup).
        let mut data = make_workspace_data();
        data.main_window.folder_filter = Some("f1".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));
        let mut fm = FocusManager::new();
        let unknown = uuid::Uuid::new_v4();

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_focus(&mut fm, WindowId::Extra(unknown), "f1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Main's pre-existing filter is unchanged (NOT cleared by a
            // fallback-to-main bug).
            assert_eq!(ws.data().main_window.folder_filter.as_deref(), Some("f1"));
            assert!(ws.data_version() >= 1);
        });
    }
}

/// Window-aware navigation: `focus_terminal_by_id` is the single entry point for
/// every "jump to this terminal", so it has to reach panes the calling window
/// currently renders nowhere.
#[cfg(all(test, feature = "gpui"))]
mod jump_gpui_tests {
    use crate::focus::FocusManager;
    use crate::settings::HooksConfig;
    use crate::state::{
        LayoutNode, ProjectData, WindowId, WindowState, Workspace, WorkspaceData,
    };
    use gpui::AppContext as _;
    use okena_core::theme::FolderColor;
    use okena_terminal::shell_config::ShellType;
    use std::collections::HashMap;

    fn project(id: &str, terminal_id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {id}"),
            path: "/tmp/test".to_string(),
            layout: Some(LayoutNode::Terminal {
                terminal_id: Some(terminal_id.to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            agent_sessions: Default::default(),
            pending_agent_resumes: Default::default(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        }
    }

    fn workspace_data(hidden: &[&str]) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: vec![project("p1", "t1"), project("p2", "t2")],
            project_order: vec!["p1".to_string(), "p2".to_string()],
            folders: Vec::new(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            main_window: WindowState {
                hidden_project_ids: hidden.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            extra_windows: Vec::new(),
        }
    }

    #[gpui::test]
    fn jumping_to_a_hidden_project_zooms_it_into_view(cx: &mut gpui::TestAppContext) {
        // p2 is in this window's hidden set, so overview renders no column for
        // it — focusing without revealing would land on an invisible pane.
        let workspace = cx.new(|_cx| Workspace::new(workspace_data(&["p2"])));
        let mut fm = FocusManager::new();

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.focus_terminal_by_id(&mut fm, WindowId::Main, "p2", "t2", cx);
        });

        assert_eq!(fm.focused_project_id().map(String::as_str), Some("p2"));
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let visible = ws.visible_projects(WindowId::Main, fm.focused_project_id(), false);
            assert!(
                visible.iter().any(|p| p.id == "p2"),
                "the jump target must end up on screen"
            );
        });
        assert_eq!(
            fm.focused_terminal_state().map(|s| s.project_id).as_deref(),
            Some("p2")
        );
    }

    #[gpui::test]
    fn jumping_within_the_overview_does_not_zoom(cx: &mut gpui::TestAppContext) {
        // p2 is already on screen, so the multi-project overview stays intact.
        let workspace = cx.new(|_cx| Workspace::new(workspace_data(&[])));
        let mut fm = FocusManager::new();

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.focus_terminal_by_id(&mut fm, WindowId::Main, "p2", "t2", cx);
        });

        assert_eq!(fm.focused_project_id(), None, "overview must be preserved");
        assert_eq!(
            fm.focused_terminal_state().map(|s| s.project_id).as_deref(),
            Some("p2")
        );
    }

    #[gpui::test]
    fn jumping_out_of_fullscreen_stays_fullscreen(cx: &mut gpui::TestAppContext) {
        // The regression this pins: callers used to zoom to the target project
        // *before* the reveal, and changing the focused project drops the
        // fullscreen context — so a notification jump dumped the user back into
        // the overview instead of following them into the target pane.
        let workspace = cx.new(|_cx| Workspace::new(workspace_data(&["p2"])));
        let mut fm = FocusManager::new();
        fm.enter_fullscreen("p1".to_string(), vec![], "t1".to_string());
        assert!(fm.has_fullscreen());

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.focus_terminal_by_id(&mut fm, WindowId::Main, "p2", "t2", cx);
        });

        assert_eq!(
            fm.fullscreen_state(),
            Some(("p2", "t2")),
            "fullscreen follows the jump onto the target pane"
        );
    }
}
