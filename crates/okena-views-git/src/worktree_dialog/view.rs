//! Render impl for the WorktreeDialog: tabs (Branches / From PR), the branch
//! search input and list or the PR picker, footer buttons.

use super::{Mode, WorktreeDialog};
use crate::Cancel;
use crate::simple_input::SimpleInput;

use okena_core::theme::ThemeColors;
use okena_files::theme::theme;
use okena_ui::button::{button, button_primary};
use okena_ui::input::input_container;
use okena_ui::tokens::{ui_text_md, ui_text_xl};

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;

impl WorktreeDialog {
    pub(super) fn render_branch_list(
        &self,
        t: ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.loading_branches {
            return div()
                .p(px(12.0))
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_muted))
                .child("Loading branches...")
                .into_any_element();
        }

        let search_empty = self.branch_search_input.read(cx).value().is_empty();

        if self.branch_selection.items().is_empty() {
            return div()
                .p(px(12.0))
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_muted))
                .child(if search_empty {
                    "No available branches for worktree"
                } else {
                    "No branches match — will create new branch"
                })
                .into_any_element();
        }

        div()
            .id("branch-list-scroll")
            .flex()
            .flex_col()
            .max_h(px(200.0))
            .overflow_y_scroll()
            .children(
                self.branch_selection
                    .items()
                    .iter()
                    .enumerate()
                    .map(|(row, branch_name)| {
                        let is_selected = self.branch_selection.is_selected(branch_name);
                        let branch_name = branch_name.clone();
                        let clicked = branch_name.clone();

                        div()
                            .id(ElementId::Name(format!("branch-{row}").into()))
                            .px(px(12.0))
                            .py(px(6.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_primary))
                            .when(is_selected, |d| d.bg(rgb(t.bg_selection)))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .child(
                                svg()
                                    .path("icons/git-branch.svg")
                                    .size(px(14.0))
                                    .text_color(rgb(t.text_secondary)),
                            )
                            .child(branch_name)
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.branch_selection.select(clicked.clone());
                                cx.notify();
                            }))
                    }),
            )
            .into_any_element()
    }
}

impl gpui::Focusable for WorktreeDialog {
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for WorktreeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        // Focus search input on first render
        if !self.initialized {
            self.initialized = true;
            let search_input = self.branch_search_input.clone();
            search_input.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }

        let branch_search_input = self.branch_search_input.clone();
        let search_input_focused = self
            .branch_search_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let pr_mode = self.mode == Mode::Pr;

        div()
            .id("worktree-dialog-backdrop")
            .track_focus(&focus_handle)
            .key_context("WorktreeDialog")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "up" => this.move_selection(false, cx),
                    "down" => this.move_selection(true, cx),
                    "enter" => this.create_worktree(cx),
                    _ => {}
                }
            }))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000080))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                div()
                    .id("worktree-dialog")
                    .w(px(450.0))
                    .max_h(px(550.0))
                    .flex()
                    .flex_col()
                    .bg(rgb(t.bg_primary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    // Header
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .child(
                                        svg()
                                            .path("icons/git-branch.svg")
                                            .size(px(16.0))
                                            .text_color(rgb(t.border_active)),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_xl(cx))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                            .child("Create Worktree"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-dialog-btn")
                                    .cursor_pointer()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .child(
                                        svg()
                                            .path("icons/close.svg")
                                            .size(px(14.0))
                                            .text_color(rgb(t.text_secondary)),
                                    )
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.close(cx);
                                    })),
                            ),
                    )
                    // Content
                    .child(
                        div().flex_1().overflow_hidden().flex().flex_col().child(
                            div()
                                .px(px(16.0))
                                .py(px(12.0))
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                // Mode toggle tabs
                                .child(
                                    h_flex()
                                        .gap(px(0.0))
                                        .border_1()
                                        .border_color(rgb(t.border))
                                        .rounded(px(4.0))
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .id("tab-branches")
                                                .flex_1()
                                                .px(px(12.0))
                                                .py(px(6.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(ui_text_md(cx))
                                                .cursor_pointer()
                                                .when(!pr_mode, |d| {
                                                    d.bg(rgb(t.bg_selection))
                                                        .text_color(rgb(t.text_primary))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                })
                                                .when(pr_mode, |d| {
                                                    d.text_color(rgb(t.text_muted))
                                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                                })
                                                .child("Branches")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.set_mode(Mode::Branch, window, cx);
                                                })),
                                        )
                                        .child(div().w(px(1.0)).h_full().bg(rgb(t.border)))
                                        .child(
                                            div()
                                                .id("tab-from-pr")
                                                .flex_1()
                                                .px(px(12.0))
                                                .py(px(6.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(ui_text_md(cx))
                                                .cursor_pointer()
                                                .when(pr_mode, |d| {
                                                    d.bg(rgb(t.bg_selection))
                                                        .text_color(rgb(t.text_primary))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                })
                                                .when(!pr_mode, |d| {
                                                    d.text_color(rgb(t.text_muted))
                                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                                })
                                                .child("From PR")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.set_mode(Mode::Pr, window, cx);
                                                })),
                                        ),
                                )
                                // Branch mode: search input + branch list
                                .when(!pr_mode, |d| {
                                    d.child(
                                        input_container(&t, Some(search_input_focused)).child(
                                            SimpleInput::new(&branch_search_input)
                                                .text_size(ui_text_md(cx)),
                                        ),
                                    )
                                    .child(self.render_branch_list(t, cx))
                                })
                                // PR mode: the picker owns its input and list
                                .when(pr_mode, |d| d.child(self.pr_picker.clone())),
                        ),
                    )
                    // Error message
                    .when_some(self.error_message.clone(), |d, msg| {
                        d.child(
                            div()
                                .px(px(16.0))
                                .py(px(8.0))
                                .bg(rgba(0xff00001a))
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.error))
                                .child(msg),
                        )
                    })
                    // Footer
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .child(
                                button("cancel-btn", "Cancel", &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.close(cx);
                                    })),
                            )
                            .child(
                                button_primary("create-btn", "Create Worktree", &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.create_worktree(cx);
                                    })),
                            ),
                    ),
            )
    }
}
