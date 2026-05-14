//! Render impl for the WorktreeDialog: tabs (Branches / From PR), search
//! input, list of branches or PRs, footer buttons.

use super::WorktreeDialog;
use crate::simple_input::SimpleInput;
use crate::Cancel;

use okena_core::theme::ThemeColors;
use okena_files::theme::theme;
use okena_ui::button::{button, button_primary};
use okena_ui::input::input_container;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_xl};

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;

impl WorktreeDialog {
    pub(super) fn render_pr_list(
        &self,
        t: ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.loading_prs {
            return div()
                .p(px(12.0))
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_muted))
                .child("Loading PRs...")
                .into_any_element();
        }

        if let Some(ref err) = self.pr_error {
            return div()
                .p(px(12.0))
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_muted))
                .child(err.clone())
                .into_any_element();
        }

        if self.pr_list.is_empty() {
            return div()
                .p(px(12.0))
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_muted))
                .child("No open pull requests")
                .into_any_element();
        }

        div()
            .id("pr-list-scroll")
            .flex()
            .flex_col()
            .max_h(px(200.0))
            .overflow_y_scroll()
            .children(
                self.pr_list.iter().enumerate().map(|(idx, pr)| {
                    let is_selected = self.selected_pr_branch.as_deref() == Some(&pr.branch);
                    let branch = pr.branch.clone();

                    div()
                        .id(ElementId::Name(format!("pr-{}", idx).into()))
                        .px(px(12.0))
                        .py(px(6.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .cursor_pointer()
                        .when(is_selected, |d| d.bg(rgb(t.bg_selection)))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.selected_pr_branch = Some(branch.clone());
                            this.selected_branch_index = None;
                            cx.notify();
                        }))
                        .child(
                            h_flex()
                                .gap(px(6.0))
                                .items_center()
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!("#{}", pr.number))
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(t.text_primary))
                                        .flex_1()
                                        .overflow_x_hidden()
                                        .whitespace_nowrap()
                                        .child(pr.title.clone())
                                )
                        )
                        .child(
                            div()
                                .pl(px(28.0))
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .child(pr.branch.clone())
                        )
                })
            )
            .into_any_element()
    }

    pub(super) fn render_branch_list(
        &self,
        t: ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let search_empty = self.branch_search_input.read(cx).value().is_empty();

        if self.filtered_branches.is_empty() {
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
                self.filtered_branches.iter().enumerate().map(|(filtered_idx, &branch_idx)| {
                    let is_selected = self.selected_branch_index == Some(filtered_idx);
                    let branch_name = self.branches[branch_idx].clone();

                    div()
                        .id(ElementId::Name(format!("branch-{}", filtered_idx).into()))
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
                                .text_color(rgb(t.text_secondary))
                        )
                        .child(branch_name)
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.selected_branch_index = Some(filtered_idx);
                            cx.notify();
                        }))
                })
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

        // Filter branches based on search input
        self.filter_branches(cx);

        let branch_search_input = self.branch_search_input.clone();
        let search_input_focused = self.branch_search_input.read(cx).focus_handle(cx).is_focused(window);
        let pr_mode = self.pr_mode;

        div()
            .id("worktree-dialog-backdrop")
            .track_focus(&focus_handle)
            .key_context("WorktreeDialog")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let search_focused = this.branch_search_input.read(cx).focus_handle(cx).is_focused(window);

                match event.keystroke.key.as_str() {
                    "up" => {
                        if search_focused {
                            if let Some(idx) = this.selected_branch_index {
                                if idx > 0 {
                                    this.selected_branch_index = Some(idx - 1);
                                    cx.notify();
                                }
                            }
                        }
                    }
                    "down" => {
                        if search_focused {
                            let max = this.filtered_branches.len().saturating_sub(1);
                            if let Some(idx) = this.selected_branch_index {
                                if idx < max {
                                    this.selected_branch_index = Some(idx + 1);
                                    cx.notify();
                                }
                            } else if !this.filtered_branches.is_empty() {
                                this.selected_branch_index = Some(0);
                                cx.notify();
                            }
                        }
                    }
                    "enter" => {
                        this.create_worktree(cx);
                    }
                    _ => {}
                }
            }))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000080))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
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
                                            .text_color(rgb(t.border_active))
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_xl(cx))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                            .child("Create Worktree")
                                    )
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
                                            .text_color(rgb(t.text_secondary))
                                    )
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.close(cx);
                                    }))
                            )
                    )
                    // Content
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .child(
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
                                                    .on_click(cx.listener(|this, _, _window, cx| {
                                                        this.pr_mode = false;
                                                        this.selected_pr_branch = None;
                                                        cx.notify();
                                                    }))
                                            )
                                            .child(
                                                div()
                                                    .w(px(1.0))
                                                    .h_full()
                                                    .bg(rgb(t.border))
                                            )
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
                                                    .on_click(cx.listener(|this, _, _window, cx| {
                                                        this.pr_mode = true;
                                                        this.selected_branch_index = None;
                                                        if !this.prs_loaded_once {
                                                            this.prs_loaded_once = true;
                                                            this.load_prs(cx);
                                                        }
                                                        cx.notify();
                                                    }))
                                            )
                                    )
                                    // Search input (only in branch mode)
                                    .when(!pr_mode, |d| {
                                        d.child(
                                            input_container(&t, Some(search_input_focused))
                                                .child(SimpleInput::new(&branch_search_input).text_size(ui_text_md(cx))),
                                        )
                                    })
                                    // Branch list or PR list
                                    .when(!pr_mode, |d| d.child(self.render_branch_list(t.clone(), cx)))
                                    .when(pr_mode, |d| d.child(self.render_pr_list(t.clone(), cx)))
                            )
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
                                .child(msg)
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
                    )
            )
    }
}
