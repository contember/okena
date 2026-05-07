//! Focus and fullscreen workspace actions
//!
//! Actions for managing terminal and project focus, including fullscreen mode.
//!
//! Per slice 03 of the multi-window plan, `FocusManager` is owned per-window
//! by `WindowView` rather than as a field on the `Workspace` entity. These
//! action methods take `focus_manager: &mut FocusManager` as a parameter so
//! every caller threads in the focus state belonging to the window driving
//! the action -- mutations stay scoped to that window.

use crate::focus::FocusManager;
use crate::state::{Workspace, WindowId};
use gpui::*;

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
        cx: &mut Context<Self>,
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
        cx: &mut Context<Self>,
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
        cx: &mut Context<Self>,
    ) {
        let selecting = self.active_folder_filter(window_id).map(|s| s.as_str()) != Some(folder_id);
        if selecting {
            self.set_folder_filter(window_id, Some(folder_id.to_string()), cx);
            // Clear project focus so all visible folder projects show
            focus_manager.set_focused_project_id(None);
            // Focus the first project's terminal
            if let Some(first_pid) = self.folder(folder_id).and_then(|f| f.project_ids.first()).cloned() {
                self.focus_first_terminal_in(focus_manager, &first_pid);
            }
        } else {
            self.set_folder_filter(window_id, None, cx);
        }
        cx.notify();
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
        // Try the project itself first, then its worktree children
        let candidates = std::iter::once(project_id.to_string())
            .chain(self.worktree_child_ids(project_id));
        for id in candidates {
            if let Some(project) = self.project(&id) {
                if let Some(layout) = project.layout.as_ref() {
                    // Focus the currently visible terminal (follows active tabs)
                    let path = layout.find_visible_terminal_path();
                    focus_manager.focus_terminal(id, path);
                    return;
                }
            }
        }
    }

    /// Enter fullscreen mode for a terminal
    pub fn set_fullscreen_terminal(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: String,
        terminal_id: String,
        cx: &mut Context<Self>,
    ) {
        log::info!("set_fullscreen_terminal called with project_id={}, terminal_id={}", project_id, terminal_id);

        // Find the layout path for this terminal
        let layout_path = self.project(&project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.find_terminal_path(&terminal_id))
            .unwrap_or_default();

        log::info!("layout_path for terminal: {:?}", layout_path);

        // Use FocusManager for fullscreen entry (saves current state + sets focused_project_id)
        focus_manager.enter_fullscreen(project_id, layout_path, terminal_id.clone());

        log::info!("fullscreen_terminal set via FocusManager with terminal_id={}", terminal_id);

        cx.notify();
    }

    /// Exit fullscreen mode
    ///
    /// Restores focus to the previously focused terminal and project view mode.
    pub fn exit_fullscreen(&mut self, focus_manager: &mut FocusManager, cx: &mut Context<Self>) {
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
        cx: &mut Context<Self>,
    ) {
        // Update FocusManager
        focus_manager.focus_terminal(project_id.clone(), layout_path.clone());

        // Record project access time for recency sorting
        self.touch_project(&project_id);

        cx.notify();
    }

    /// Clear focused terminal
    ///
    /// This is typically called when entering a modal context (search, rename, etc.)
    /// The current focus is saved for restoration when the modal closes.
    pub fn clear_focused_terminal(&mut self, focus_manager: &mut FocusManager, cx: &mut Context<Self>) {
        focus_manager.enter_modal();
        cx.notify();
    }

    /// Restore focused terminal after modal dismissal
    ///
    /// Called when exiting a modal context to restore the previous focus.
    pub fn restore_focused_terminal(&mut self, focus_manager: &mut FocusManager, cx: &mut Context<Self>) {
        focus_manager.exit_modal();
        cx.notify();
    }

    /// Focus a terminal by its ID (finds path automatically)
    ///
    /// This is a convenience method that looks up the layout path and calls set_focused_terminal.
    pub fn focus_terminal_by_id(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        terminal_id: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.project(project_id) {
            if let Some(ref layout) = project.layout {
                if let Some(path) = layout.find_terminal_path(terminal_id) {
                    // Activate any tabs along the path so the terminal becomes visible
                    if let Some(project_mut) = self.project_mut(project_id) {
                        if let Some(ref mut layout) = project_mut.layout {
                            layout.activate_tabs_along_path(&path);
                        }
                    }
                    self.notify_data(cx);
                    // Focus the terminal without changing which projects are shown
                    self.set_focused_terminal(focus_manager, project_id.to_string(), path, cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod gpui_tests {
    use gpui::AppContext as _;
    use crate::focus::FocusManager;
    use crate::state::{FolderData, WindowId, WindowState, Workspace, WorkspaceData};
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
        let mut extra_b = WindowState::default();
        extra_b.folder_filter = Some("f1".to_string());
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];
        let workspace = cx.new(|_cx| Workspace::new(data));
        let mut fm = FocusManager::new();

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_folder_focus(&mut fm, WindowId::Extra(extra_a_id), "f1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Targeted extra got the filter.
            assert_eq!(ws.data().extra_windows[0].folder_filter.as_deref(), Some("f1"));
            // Main stays untouched.
            assert!(ws.data().main_window.folder_filter.is_none());
            // Sibling extra's pre-existing filter is preserved.
            assert_eq!(ws.data().extra_windows[1].folder_filter.as_deref(), Some("f1"));
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
            assert_eq!(ws.data().extra_windows[0].folder_filter.as_deref(), Some("f1"));
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
