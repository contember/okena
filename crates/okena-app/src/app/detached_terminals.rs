use crate::views::overlays::detached_terminal::DetachedTerminalView;
use crate::terminal::terminal::TerminalTransport;
use crate::workspace::state::Workspace;
use gpui::*;
#[cfg(not(target_os = "linux"))]
use gpui_component::Root;
#[cfg(target_os = "linux")]
use crate::simple_root::SimpleRoot as Root;
use std::collections::HashSet;
use std::sync::Arc;

use super::Okena;

impl Okena {
    pub(super) fn handle_detached_terminals_changed(
        &mut self,
        workspace: Entity<Workspace>,
        cx: &mut Context<Self>,
    ) {
        // Keep each detached terminal's owning project so we can resolve the
        // daemon connection that carries its PTY (every terminal is remote).
        let current: Vec<(String, String)> = workspace
            .read(cx)
            .collect_all_detached_terminals()
            .into_iter()
            .map(|(terminal_id, project_id, _)| (terminal_id, project_id))
            .collect();

        let current_ids: HashSet<String> =
            current.iter().map(|(terminal_id, _)| terminal_id.clone()).collect();

        let new: Vec<(String, String)> = current
            .iter()
            .filter(|(terminal_id, _)| !self.opened_detached_windows.contains(terminal_id))
            .cloned()
            .collect();

        self.opened_detached_windows = current_ids;

        for (terminal_id, project_id) in new {
            self.open_detached_window(&terminal_id, &project_id, cx);
        }
    }

    /// Resolve the `TerminalTransport` that carries a project's terminals.
    /// Every project is daemon-served, so its terminal bytes and input ride the
    /// connection's `RemoteTransport`. Returns `None` if the project is unknown
    /// or its connection isn't currently established.
    fn transport_for_project(
        &self,
        project_id: &str,
        cx: &App,
    ) -> Option<Arc<dyn TerminalTransport>> {
        let connection_id = self
            .workspace
            .read(cx)
            .project(project_id)?
            .connection_id
            .clone()?;
        let backend = self.remote_manager.read(cx).backend_for(&connection_id)?;
        Some(backend.transport())
    }

    fn open_detached_window(&self, terminal_id: &str, project_id: &str, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        // A detached terminal reuses the live `Arc<Terminal>` already in the
        // registry; the transport only matters on the re-create fallback. Route
        // it over the project's daemon connection so input/recreation never
        // touch a local PTY (the thin client owns none).
        let Some(transport) = self.transport_for_project(project_id, cx) else {
            log::warn!(
                "Cannot open detached window for terminal {terminal_id}: no remote \
                 transport for its connection (project {project_id})"
            );
            return;
        };
        let terminals = self.terminals.clone();
        let terminal_id_owned = terminal_id.to_string();

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
                // On Windows the chrome is fully client-drawn (matches main window);
                // other platforms keep the transparent titlebar.
                titlebar: if cfg!(target_os = "windows") {
                    None
                } else {
                    Some(TitlebarOptions {
                        title: Some(format!("{} - Detached", terminal_name).into()),
                        appears_transparent: true,
                        ..Default::default()
                    })
                },
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(800.0), px(600.0)),
                })),
                is_resizable: true,
                window_decorations: Some(if cfg!(target_os = "windows") {
                    WindowDecorations::Client
                } else {
                    WindowDecorations::Server
                }),
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
                        transport.clone(),
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
