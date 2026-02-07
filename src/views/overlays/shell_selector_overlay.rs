//! Shell selector overlay for switching terminal shells.
//!
//! Displays available shells in a modal dialog for selection.

use crate::keybindings::Cancel;
use crate::terminal::shell_config::{available_shells, AvailableShell, ShellType};
use crate::theme::theme;
use crate::views::components::{
    handle_list_overlay_key, modal_backdrop, modal_content, modal_header, ListOverlayAction,
    ListOverlayConfig, ListOverlayState,
};
use gpui::*;
use gpui_component::h_flex;
use gpui::prelude::*;

/// Shell selector overlay for choosing a shell.
pub struct ShellSelectorOverlay {
    focus_handle: FocusHandle,
    state: ListOverlayState<AvailableShell>,
    current_shell: ShellType,
    /// Context: which terminal this is for (project_id, terminal_id)
    context: Option<(String, String)>,
}

impl ShellSelectorOverlay {
    pub fn new(current_shell: ShellType, context: Option<(String, String)>, cx: &mut Context<Self>) -> Self {
        let shells: Vec<_> = available_shells().into_iter().filter(|s| s.available).collect();
        let selected_index = shells
            .iter()
            .position(|s| s.shell_type == current_shell)
            .unwrap_or(0);

        let config = ListOverlayConfig::new("Switch Shell")
            .subtitle("Select shell for this terminal")
            .size(280.0, 300.0)
            .centered()
            .key_context("ShellSelectorOverlay");

        let state = ListOverlayState::with_selected(shells, config, selected_index, cx);
        let focus_handle = state.focus_handle.clone();

        Self {
            focus_handle,
            state,
            current_shell,
            context,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(ShellSelectorOverlayEvent::Close);
    }

    fn select_shell(&mut self, shell_type: ShellType, cx: &mut Context<Self>) {
        cx.emit(ShellSelectorOverlayEvent::ShellSelected {
            shell_type,
            context: self.context.clone(),
        });
    }

    fn select_current(&mut self, cx: &mut Context<Self>) {
        if let Some(shell) = self.state.selected_item() {
            self.select_shell(shell.shell_type.clone(), cx);
        }
    }
}

#[derive(Clone)]
pub enum ShellSelectorOverlayEvent {
    Close,
    ShellSelected {
        shell_type: ShellType,
        context: Option<(String, String)>,
    },
}

impl EventEmitter<ShellSelectorOverlayEvent> for ShellSelectorOverlay {}

impl Render for ShellSelectorOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let current_shell = self.current_shell.clone();
        let selected_index = self.state.selected_index;
        let config_width = self.state.config.width;
        let config_title = self.state.config.title.clone();
        let config_subtitle = self.state.config.subtitle.clone();

        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("shell-selector-overlay-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("ShellSelectorOverlay")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match handle_list_overlay_key(&mut this.state, event, &[]) {
                    ListOverlayAction::Close => this.close(cx),
                    ListOverlayAction::Confirm => this.select_current(cx),
                    ListOverlayAction::SelectPrev | ListOverlayAction::SelectNext => cx.notify(),
                    _ => {}
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .child(
                modal_content("shell-selector-overlay-modal", &t)
                    .w(px(config_width))
                    .child(modal_header(
                        config_title,
                        config_subtitle,
                        &t,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(
                        div()
                            .id("shell-selector-list")
                            .py(px(4.0))
                            .max_h(px(self.state.config.max_height))
                            .overflow_y_scroll()
                            .children(self.state.filtered.iter().enumerate().map(|(i, filter_result)| {
                                let shell = &self.state.items[filter_result.index];
                                let is_current = shell.shell_type == current_shell;
                                let is_selected = i == selected_index;
                                let shell_type = shell.shell_type.clone();
                                let name = shell.name.clone();

                                div()
                                    .id(ElementId::Name(format!("shell-opt-{}", i).into()))
                                    .w_full()
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .cursor_pointer()
                                    .when(is_selected, |d| d.bg(rgb(t.bg_hover)))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _window, cx| {
                                            cx.stop_propagation();
                                            this.select_shell(shell_type.clone(), cx);
                                        }),
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(rgb(t.text_primary))
                                                    .child(name),
                                            )
                                            .when(is_current, |d| {
                                                d.child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(rgb(t.success))
                                                        .child("✓"),
                                                )
                                            }),
                                    )
                            })),
                    ),
            )
    }
}

impl_focusable!(ShellSelectorOverlay);
