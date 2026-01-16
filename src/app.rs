use crate::terminal::pty_manager::{PtyEvent, PtyManager};
use crate::views::detached_terminal::DetachedTerminalView;
use crate::views::root::{RootView, TerminalsRegistry};
use crate::workspace::persistence;
use crate::workspace::state::{Workspace, WorkspaceData};
use async_channel::Receiver;
use gpui::*;
use gpui_component::Root;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Main application state and view
pub struct TermManager {
    root_view: Entity<RootView>,
    workspace: Entity<Workspace>,
    pty_manager: Arc<PtyManager>,
    terminals: TerminalsRegistry,
    /// Track which detached windows we've already opened
    opened_detached_windows: HashSet<String>,
    /// Flag indicating workspace needs to be saved (for debouncing)
    /// Note: Field is read by spawned tasks, not directly
    #[allow(dead_code)]
    save_pending: Arc<AtomicBool>,
}

impl TermManager {
    pub fn new(
        workspace_data: WorkspaceData,
        pty_manager: Arc<PtyManager>,
        pty_events: Receiver<PtyEvent>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Create workspace entity
        let workspace = cx.new(|_cx| Workspace::new(workspace_data));

        // Shared flag for debounced save
        let save_pending = Arc::new(AtomicBool::new(false));

        // Set up debounced auto-save on workspace changes
        // Instead of saving synchronously on every change, we mark save as pending
        // and a background task saves after 500ms of no changes
        let save_pending_for_observer = save_pending.clone();
        let workspace_for_save = workspace.clone();
        cx.observe(&workspace, move |_this, _workspace, cx| {
            // Mark that a save is needed
            save_pending_for_observer.store(true, Ordering::Relaxed);

            // Spawn a debounced save task
            let save_pending = save_pending_for_observer.clone();
            let workspace = workspace_for_save.clone();
            cx.spawn(async move |_, cx| {
                // Wait for debounce period
                smol::Timer::after(std::time::Duration::from_millis(500)).await;

                // Only save if still pending (no newer changes reset this)
                if save_pending.swap(false, Ordering::Relaxed) {
                    // Read workspace data and save in background
                    let data = cx.update(|cx| workspace.read(cx).data.clone());
                    if let Err(e) = persistence::save_workspace(&data) {
                        log::error!("Failed to save workspace: {}", e);
                    }
                }
            }).detach();
        })
        .detach();

        // Create root view (get terminals registry from it)
        let pty_manager_clone = pty_manager.clone();
        let root_view = cx.new(|cx| {
            RootView::new(workspace.clone(), pty_manager_clone, pty_events, cx)
        });

        // Get terminals registry from root view
        let terminals = root_view.read(cx).terminals().clone();

        let manager = Self {
            root_view,
            workspace: workspace.clone(),
            pty_manager,
            terminals,
            opened_detached_windows: HashSet::new(),
            save_pending,
        };

        // Set up observer for detached terminals
        cx.observe(&workspace, move |this, workspace, cx| {
            this.handle_detached_terminals_changed(workspace, cx);
        })
        .detach();

        manager
    }

    fn handle_detached_terminals_changed(
        &mut self,
        workspace: Entity<Workspace>,
        cx: &mut Context<Self>,
    ) {
        let ws = workspace.read(cx);
        let current_detached: HashSet<String> = ws
            .detached_terminals
            .iter()
            .map(|d| d.terminal_id.clone())
            .collect();

        // Find newly detached terminals (not yet opened)
        let new_detached: Vec<_> = ws
            .detached_terminals
            .iter()
            .filter(|d| !self.opened_detached_windows.contains(&d.terminal_id))
            .cloned()
            .collect();

        // Find terminals that were re-attached (windows should close)
        let reattached: Vec<_> = self
            .opened_detached_windows
            .iter()
            .filter(|id| !current_detached.contains(*id))
            .cloned()
            .collect();

        // Update our tracking set
        self.opened_detached_windows = current_detached;

        // Open windows for newly detached terminals
        for detached in new_detached {
            self.open_detached_window(&detached.terminal_id, cx);
        }

        // Note: Window closing when re-attached is handled by the window itself
        // through workspace observation
        let _ = reattached; // Suppress unused warning
    }

    fn open_detached_window(&self, terminal_id: &str, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let pty_manager = self.pty_manager.clone();
        let terminals = self.terminals.clone();
        let terminal_id_owned = terminal_id.to_string();

        // Get terminal name for window title
        let terminal_name = {
            let ws = workspace.read(cx);
            let mut name = terminal_id.chars().take(8).collect::<String>();
            for project in ws.projects() {
                if let Some(custom_name) = project.terminal_names.get(terminal_id) {
                    name = custom_name.clone();
                    break;
                }
            }
            name
        };

        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("{} - Detached", terminal_name).into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(800.0), px(600.0)),
                })),
                is_resizable: true,
                window_decorations: Some(WindowDecorations::Server),
                window_min_size: Some(Size {
                    width: px(300.0),
                    height: px(200.0),
                }),
                ..Default::default()
            },
            move |window, cx| {
                let detached_view = cx.new(|cx| {
                    DetachedTerminalView::new(
                        workspace.clone(),
                        terminal_id_owned.clone(),
                        pty_manager.clone(),
                        terminals.clone(),
                        cx,
                    )
                });
                cx.new(|cx| Root::new(detached_view, window, cx))
            },
        )
        .ok();
    }
}

impl Render for TermManager {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.root_view.clone())
    }
}
