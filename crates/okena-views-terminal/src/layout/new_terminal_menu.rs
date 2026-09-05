//! "What should this terminal run?" menu for splits and new tabs.
//!
//! Left-clicking a split or new-tab button keeps its existing behaviour — the
//! project's default shell — so the common case costs no extra clicks. A
//! secondary click opens this menu to start the pane directly on a coding
//! agent instead.

use crate::layout::layout_container::{LayoutContainer, NewTerminalMenu, NewTerminalTarget};
use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;
use okena_terminal::shell_config::ShellType;
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_ms;

use crate::ActionDispatch;

/// Agents offered when opening a pane.
///
/// The same two okena can launch and detect elsewhere in the harness — offering
/// one it cannot recognise afterwards would produce sessions that never show up
/// in the Agents view.
const AGENTS: &[&str] = &["claude", "copilot"];

impl<D: ActionDispatch + Send + Sync + 'static> LayoutContainer<D> {
    /// Open the picker for `target`.
    pub(super) fn open_new_terminal_menu(
        &mut self,
        target: NewTerminalTarget,
        cx: &mut Context<Self>,
    ) {
        self.new_terminal_menu = Some(NewTerminalMenu {
            target,
            layout_path: self.layout_path.clone(),
        });
        cx.notify();
    }

    /// Create the pane with `shell`, then close the menu.
    fn create_with_shell(&mut self, shell: Option<ShellType>, cx: &mut Context<Self>) {
        let Some(menu) = self.new_terminal_menu.take() else {
            return;
        };
        let Some(dispatcher) = self.action_dispatcher.clone() else {
            cx.notify();
            return;
        };
        let project_id = self.project_id.clone();

        match menu.target {
            NewTerminalTarget::Split(direction) => {
                dispatcher.dispatch(
                    okena_core::api::ActionRequest::SplitTerminal {
                        project_id,
                        path: menu.layout_path,
                        direction,
                        shell_type: shell,
                    },
                    cx,
                );
            }
            NewTerminalTarget::Tab { in_group } => {
                dispatcher.dispatch(
                    okena_core::api::ActionRequest::AddTab {
                        project_id,
                        path: menu.layout_path,
                        in_group,
                        shell_type: shell,
                    },
                    cx,
                );
            }
        }
        cx.notify();
    }

    pub(super) fn render_new_terminal_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.new_terminal_menu.as_ref()?;
        let t = theme(cx);

        let row = |label: String, shell: Option<ShellType>, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(format!("new-term-{label}")))
                .cursor_pointer()
                .px(px(10.0))
                .py(px(5.0))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_primary))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.create_with_shell(shell.clone(), cx);
                    }),
                )
                .into_any_element()
        };

        let mut items = vec![row("Shell".to_string(), None, cx)];
        for agent in AGENTS {
            items.push(row(
                (*agent).to_string(),
                // A bare custom shell: no args, so the agent starts
                // interactively rather than running a one-shot prompt.
                Some(ShellType::Custom {
                    path: (*agent).to_string(),
                    args: Vec::new(),
                }),
                cx,
            ));
        }

        Some(
            div()
                .absolute()
                .inset_0()
                // Clicking anywhere else dismisses, so the menu can never get
                // stuck open over the terminal.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        this.new_terminal_menu = None;
                        cx.notify();
                    }),
                )
                .child(
                    v_flex()
                        .absolute()
                        .top(px(28.0))
                        .right(px(8.0))
                        .w(px(150.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(rgb(t.border))
                        .bg(rgb(t.bg_secondary))
                        .child(
                            div()
                                .px(px(10.0))
                                .py(px(4.0))
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .child("Open with"),
                        )
                        .children(items),
                )
                .into_any_element(),
        )
    }
}
