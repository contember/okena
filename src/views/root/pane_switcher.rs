use crate::theme::theme;
use crate::views::layout::navigation::PaneMap;
use crate::workspace::state::Workspace;
use gpui::*;

use super::RootView;

/// Labels for pane indices: 0-9 then a-z (up to 36 panes).
const PANE_LABELS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Map a key string to a pane index.
/// Accepts "0"-"9" and "a"-"z".
fn key_to_pane_index(key: &str) -> Option<usize> {
    if key.len() != 1 {
        return None;
    }
    let ch = key.as_bytes()[0];
    PANE_LABELS.iter().position(|&l| l == ch)
}

/// Get the display label for a pane index.
fn pane_label(index: usize) -> String {
    PANE_LABELS
        .get(index)
        .map(|&b| (b as char).to_uppercase().to_string())
        .unwrap_or_default()
}

/// Pane switcher overlay entity - shows labelled badges on each visible pane.
///
/// Rendered as a separate entity so it gets its own focus path,
/// preventing key events from reaching the terminal panes underneath.
pub(super) struct PaneSwitcher {
    focus_handle: FocusHandle,
    workspace: Entity<Workspace>,
    /// Pane info: (project_id, layout_path, bounds) sorted by reading order
    panes: Vec<(String, Vec<usize>, Bounds<Pixels>)>,
}

impl PaneSwitcher {
    pub fn new(workspace: Entity<Workspace>, pane_map: &PaneMap, cx: &mut Context<Self>) -> Self {
        let panes = pane_map
            .panes()
            .iter()
            .take(PANE_LABELS.len())
            .map(|p| (p.project_id.clone(), p.layout_path.clone(), p.bounds))
            .collect();

        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            panes,
        }
    }
}

impl Render for PaneSwitcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let badge_bg = rgb(t.button_primary_bg);
        let badge_fg = rgb(t.button_primary_fg);

        // Focus on every render (same pattern as CommandPalette)
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        // Build absolutely-positioned overlays for each pane
        let mut overlay_elements: Vec<AnyElement> = Vec::new();
        for (i, (_project_id, _layout_path, bounds)) in self.panes.iter().enumerate() {
            let label = pane_label(i);

            overlay_elements.push(
                div()
                    .absolute()
                    .left(bounds.origin.x)
                    .top(bounds.origin.y)
                    .w(bounds.size.width)
                    .h(bounds.size.height)
                    .bg(hsla(0.0, 0.0, 0.0, 0.4))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(8.0))
                            .rounded(px(8.0))
                            .bg(badge_bg)
                            .child(
                                div()
                                    .text_size(px(32.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(badge_fg)
                                    .child(label),
                            ),
                    )
                    .into_any_element(),
            );
        }

        div()
            .id("pane-switcher-overlay")
            .occlude()
            .track_focus(&self.focus_handle)
            .absolute()
            .inset_0()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let key = &event.keystroke.key;

                // Try to map key to pane index (0-9, a-z)
                if let Some(index) = key_to_pane_index(key) {
                    if let Some((project_id, layout_path, _)) = this.panes.get(index) {
                        this.workspace.update(cx, |ws, cx| {
                            ws.set_focused_terminal(
                                project_id.clone(),
                                layout_path.clone(),
                                cx,
                            );
                        });
                    }
                }

                // Any key deactivates - emit close event
                cx.emit(PaneSwitcherEvent::Close);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                    cx.emit(PaneSwitcherEvent::Close);
                }),
            )
            .children(overlay_elements)
    }
}

pub(super) enum PaneSwitcherEvent {
    Close,
}

impl EventEmitter<PaneSwitcherEvent> for PaneSwitcher {}

// === RootView integration ===

impl RootView {
    /// Create and show the pane switcher overlay entity.
    pub(super) fn show_pane_switcher(&mut self, pane_map: PaneMap, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let entity = cx.new(|cx| PaneSwitcher::new(workspace, &pane_map, cx));

        cx.subscribe(&entity, |this, _, event: &PaneSwitcherEvent, cx| {
            match event {
                PaneSwitcherEvent::Close => {
                    this.pane_switch_active = false;
                    this.pane_switcher_entity = None;
                    cx.notify();
                }
            }
        })
        .detach();

        self.pane_switcher_entity = Some(entity);
    }
}
