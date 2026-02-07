use crate::views::overlays::detached_terminal::DetachedTerminalView;
use crate::workspace::state::Workspace;
use gpui::*;
#[cfg(not(target_os = "linux"))]
use gpui_component::Root;
#[cfg(target_os = "linux")]
use crate::simple_root::SimpleRoot as Root;
use std::collections::HashSet;

use super::Okena;

impl Okena {
    pub(super) fn handle_detached_terminals_changed(
        &mut self,
        workspace: Entity<Workspace>,
        cx: &mut Context<Self>,
    ) {
        let ws = workspace.read(cx);
        let current_detached: HashSet<String> = ws
            .collect_all_detached_terminals()
            .into_iter()
            .map(|(terminal_id, _, _)| terminal_id)
            .collect();

        let new_ids: Vec<_> = current_detached
            .iter()
            .filter(|id| !self.opened_detached_windows.contains(*id))
            .cloned()
            .collect();

        self.opened_detached_windows = current_detached;

        for terminal_id in new_ids {
            self.open_detached_window(&terminal_id, cx);
        }
    }

    fn open_detached_window(&self, terminal_id: &str, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let transport: std::sync::Arc<dyn crate::terminal::terminal::TerminalTransport> = self.pty_manager.clone();
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
