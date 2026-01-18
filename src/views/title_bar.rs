use crate::keybindings::ToggleSidebar;
use crate::theme::theme;
use crate::workspace::state::Workspace;
use gpui::*;
use gpui::prelude::*;

/// Window control button types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowControlType {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Title bar with window controls, sidebar toggle, and focused project indicator
pub struct TitleBar {
    title: SharedString,
    workspace: Entity<Workspace>,
}

impl TitleBar {
    pub fn new(
        title: impl Into<SharedString>,
        workspace: Entity<Workspace>,
    ) -> Self {
        Self {
            title: title.into(),
            workspace,
        }
    }

    fn render_window_control(
        &self,
        control_type: WindowControlType,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let icon = match control_type {
            WindowControlType::Minimize => "─",
            WindowControlType::Maximize => "□",
            WindowControlType::Restore => "❐",
            WindowControlType::Close => "✕",
        };

        let is_close = control_type == WindowControlType::Close;

        div()
            .id(ElementId::Name(format!("window-control-{:?}", control_type).into()))
            .cursor_pointer()
            .w(px(28.0))
            .h(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .text_size(px(12.0))
            .text_color(rgb(t.text_secondary))
            .when(is_close, |d| {
                d.hover(|s| s.bg(rgb(0xE81123)).text_color(rgb(0xffffff)))
            })
            .when(!is_close, |d| {
                d.hover(|s| s.bg(rgb(t.bg_hover)))
            })
            .child(icon)
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click({
                let control_type = control_type;
                move |_, window, cx| {
                    cx.stop_propagation();
                    match control_type {
                        WindowControlType::Minimize => window.minimize_window(),
                        WindowControlType::Maximize | WindowControlType::Restore => {
                            window.zoom_window()
                        }
                        WindowControlType::Close => {
                            cx.quit();
                        }
                    }
                }
            })
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_maximized = window.is_maximized();
        let decorations = window.window_decorations();

        // Check if we need client-side decorations
        let needs_controls = match decorations {
            Decorations::Server => false,
            Decorations::Client { .. } => true,
        };

        // On macOS with server decorations, we need to leave space for traffic lights
        let traffic_light_padding = if cfg!(target_os = "macos") && !needs_controls {
            px(80.0) // Space for macOS traffic lights (close, minimize, fullscreen)
        } else {
            px(8.0)
        };

        // Get focused project info
        let focused_project = {
            let ws = self.workspace.read(cx);
            ws.focused_project_id
                .as_ref()
                .and_then(|id| ws.project(id))
                .map(|p| p.name.clone())
        };

        let workspace = self.workspace.clone();

        div()
            .id("title-bar")
            .h(px(32.0))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(t.bg_header))
            .border_b_1()
            .border_color(rgb(t.border))
            // Make title bar draggable for window move
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.start_window_move();
                cx.stop_propagation();
            })
            .child(
                // Left side - sidebar toggle + title
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .pl(traffic_light_padding)
                    .child(
                        // Sidebar toggle
                        div()
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                            .child("☰")
                            .id("sidebar-toggle")
                            // Stop propagation to prevent title bar drag from capturing the click
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(|_, window, cx| {
                                cx.stop_propagation();
                                window.dispatch_action(Box::new(ToggleSidebar), cx);
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .child(self.title.clone()),
                    ),
            )
            .child(
                // Center - spacer
                div().flex_1()
            )
            .child(
                // Right side - focused project indicator + theme toggle + window controls
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .pr(px(4.0))
                    // Focused project indicator
                    .children(focused_project.map(|name| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(t.text_muted))
                                    .child("Focused:"),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.border_active))
                                    .text_size(px(11.0))
                                    .text_color(rgb(0xffffff))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .cursor_pointer()
                                    .px(px(4.0))
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .hover(|s| s.text_color(rgb(t.text_primary)))
                                    .child("✕")
                                    .id("clear-focus-btn")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click({
                                        let workspace = workspace.clone();
                                        move |_, _window, cx| {
                                            cx.stop_propagation();
                                            workspace.update(cx, |ws, cx| {
                                                ws.set_focused_project(None, cx);
                                            });
                                        }
                                    }),
                            )
                    }))
                    .when(needs_controls, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(2.0))
                                .child(self.render_window_control(WindowControlType::Minimize, window, cx))
                                .child(if is_maximized {
                                    self.render_window_control(WindowControlType::Restore, window, cx).into_any_element()
                                } else {
                                    self.render_window_control(WindowControlType::Maximize, window, cx).into_any_element()
                                })
                                .child(self.render_window_control(WindowControlType::Close, window, cx))
                        )
                    }),
            )
    }
}
