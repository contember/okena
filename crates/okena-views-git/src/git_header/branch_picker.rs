//! Branch switcher popover — filter/select a local or remote branch, or
//! create a new one from the current HEAD.

use super::{BranchKind, BranchPickerStatus, GitHeader};

use okena_core::theme::ThemeColors;
use okena_git::BranchList;
use okena_ui::simple_input::SimpleInput;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};

impl GitHeader {
    /// Open the branch switcher popover and load branches asynchronously.
    /// No-op when the provider is read-only (remote-mirrored project).
    pub fn show_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_provider.supports_mutations() {
            return;
        }
        if self.branch_picker_visible {
            // Already open — just refocus filter so a second hotkey press is harmless.
            let filter = self.branch_picker_filter.clone();
            filter.update(cx, |inp, cx| inp.focus(window, cx));
            return;
        }

        // Hide other popovers
        self.diff_popover_visible = false;
        self.commit_log_visible = false;

        self.branch_picker_visible = true;
        // Enter modal context so the project's terminal pane stops re-grabbing
        // window focus on each render (which would route keystrokes there
        // even though the filter input still shows a blinking cursor).
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        // Clear stale list so the previous repo's branches don't flash before
        // the async load completes.
        self.branch_picker_list = BranchList::default();
        self.branch_picker_status = BranchPickerStatus::Loading;
        self.branch_picker_create_mode = false;
        let filter = self.branch_picker_filter.clone();
        filter.update(cx, |inp, cx| {
            inp.set_value("", cx);
            inp.focus(window, cx);
        });
        let create_input = self.branch_picker_create_name.clone();
        create_input.update(cx, |inp, cx| inp.set_value("", cx));
        cx.notify();

        let provider = self.git_provider.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let list = smol::unblock(move || provider.list_branches_classified()).await;
            let _ = this.update(cx, |this, cx| {
                this.branch_picker_list = list;
                if matches!(this.branch_picker_status, BranchPickerStatus::Loading) {
                    this.branch_picker_status = BranchPickerStatus::Idle;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Close the branch switcher popover.
    pub fn hide_branch_picker(&mut self, cx: &mut Context<Self>) {
        if !self.branch_picker_visible {
            return;
        }
        self.branch_picker_visible = false;
        self.branch_picker_create_mode = false;
        self.branch_picker_status = BranchPickerStatus::Idle;
        // Restore the previously-focused terminal so typing resumes there.
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    /// Record the on-screen bounds of the branch chip so the popover can
    /// anchor underneath it. Caller-side change detection avoids re-running
    /// this every frame.
    pub fn set_branch_chip_bounds(&mut self, bounds: Bounds<Pixels>) {
        if self.branch_picker_bounds != bounds {
            self.branch_picker_bounds = bounds;
        }
    }

    fn toggle_branch_create_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.branch_picker_create_mode = !self.branch_picker_create_mode;
        self.branch_picker_status = BranchPickerStatus::Idle;
        if self.branch_picker_create_mode {
            let input = self.branch_picker_create_name.clone();
            input.update(cx, |inp, cx| {
                inp.set_value("", cx);
                inp.focus(window, cx);
            });
        } else {
            let filter = self.branch_picker_filter.clone();
            filter.update(cx, |inp, cx| inp.focus(window, cx));
        }
        cx.notify();
    }

    fn checkout_branch(&mut self, branch: String, kind: BranchKind, cx: &mut Context<Self>) {
        if matches!(self.branch_picker_status, BranchPickerStatus::Working) {
            return;
        }
        self.branch_picker_status = BranchPickerStatus::Working;
        cx.notify();

        let provider = self.git_provider.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = smol::unblock(move || match kind {
                BranchKind::Local => provider.checkout_local_branch(&branch),
                BranchKind::Remote => provider.checkout_remote_branch(&branch),
            })
            .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.hide_branch_picker(cx),
                Err(e) => {
                    this.branch_picker_status = BranchPickerStatus::Error(e);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn create_branch_from_current(&mut self, cx: &mut Context<Self>) {
        if matches!(self.branch_picker_status, BranchPickerStatus::Working) {
            return;
        }
        let raw = self
            .branch_picker_create_name
            .read(cx)
            .value()
            .trim()
            .to_string();
        if raw.is_empty() {
            self.branch_picker_status =
                BranchPickerStatus::Error("Branch name cannot be empty".to_string());
            cx.notify();
            return;
        }
        if okena_git::validate_git_ref(&raw).is_err() {
            self.branch_picker_status =
                BranchPickerStatus::Error(format!("Invalid branch name: {}", raw));
            cx.notify();
            return;
        }

        self.branch_picker_status = BranchPickerStatus::Working;
        cx.notify();

        let provider = self.git_provider.clone();
        let name = raw.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = smol::unblock(move || {
                provider.create_and_checkout_branch(&name, None)
            })
            .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.hide_branch_picker(cx),
                Err(e) => {
                    this.branch_picker_status = BranchPickerStatus::Error(e);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Render the branch switcher popover anchored under the branch chip.
    /// Returns a zero-size element when the popover is hidden.
    pub fn render_branch_picker(
        &mut self,
        window: &mut Window,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.branch_picker_visible {
            return div().size_0().into_any_element();
        }

        // Keep the active input focused while the popover is open. This handles
        // the first render after `show_branch_picker` (which can't observe its
        // own popover) and any focus loss from re-rendering parents.
        let active = if self.branch_picker_create_mode {
            &self.branch_picker_create_name
        } else {
            &self.branch_picker_filter
        };
        let active_handle = active.read(cx).focus_handle(cx);
        if !active_handle.is_focused(window) {
            let active = active.clone();
            active.update(cx, |inp, cx| inp.focus(window, cx));
        }

        let bounds = self.branch_picker_bounds;
        let position = point(
            bounds.origin.x,
            bounds.origin.y + bounds.size.height + px(6.0),
        );

        let filter_text = self.branch_picker_filter.read(cx).value().to_lowercase();
        let current = self.branch_picker_list.current.clone();
        let local: Vec<&String> = self
            .branch_picker_list
            .local
            .iter()
            .filter(|b| filter_text.is_empty() || b.to_lowercase().contains(&filter_text))
            .collect();
        let remote: Vec<&String> = self
            .branch_picker_list
            .remote
            .iter()
            .filter(|b| filter_text.is_empty() || b.to_lowercase().contains(&filter_text))
            .collect();
        let is_create = self.branch_picker_create_mode;
        let is_working =
            matches!(self.branch_picker_status, BranchPickerStatus::Working);
        let is_loading =
            matches!(self.branch_picker_status, BranchPickerStatus::Loading);
        let error = match &self.branch_picker_status {
            BranchPickerStatus::Error(msg) => Some(msg.clone()),
            _ => None,
        };

        let row = |name: String,
                   is_current: bool,
                   kind: BranchKind,
                   key: String,
                   cx: &mut Context<Self>|
         -> AnyElement {
            let name_for_click = name.clone();
            let is_remote = kind == BranchKind::Remote;
            h_flex()
                .id(ElementId::Name(key.into()))
                .px(px(10.0))
                .py(px(4.0))
                .gap(px(6.0))
                .items_center()
                .cursor_pointer()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(if is_current { t.text_primary } else { t.text_secondary }))
                .when(is_current, |d| d.font_weight(FontWeight::SEMIBOLD))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .child(
                    svg()
                        .path("icons/git-branch.svg")
                        .size(px(10.0))
                        .text_color(rgb(if is_remote { t.term_green } else { t.text_muted })),
                )
                .child(div().flex_1().min_w_0().text_ellipsis().overflow_hidden().child(name))
                .when(is_current, |d| {
                    d.child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.term_cyan))
                            .child("HEAD"),
                    )
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.checkout_branch(name_for_click.clone(), kind, cx);
                }))
                .into_any_element()
        };

        let section_header = |label: &'static str, cx: &App| -> Div {
            div()
                .px(px(10.0))
                .py(px(4.0))
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(label)
        };

        deferred(
            anchored()
                .position(position)
                .snap_to_window()
                .child(
                    v_flex()
                        .id("branch-picker-popover")
                        .occlude()
                        .w(px(320.0))
                        .max_h(px(420.0))
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(t.border))
                        .rounded(px(8.0))
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.hide_branch_picker(cx);
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        })
                        // Filter / create input
                        .child(
                            div()
                                .px(px(10.0))
                                .py(px(8.0))
                                .border_b_1()
                                .border_color(rgb(t.border))
                                .child(if is_create {
                                    v_flex()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .text_size(ui_text_sm(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child(format!(
                                                    "New branch from {}",
                                                    current.clone().unwrap_or_else(|| "HEAD".to_string())
                                                )),
                                        )
                                        .child(
                                            SimpleInput::new(&self.branch_picker_create_name)
                                                .text_size(ui_text_md(cx)),
                                        )
                                        .into_any_element()
                                } else {
                                    SimpleInput::new(&self.branch_picker_filter)
                                        .text_size(ui_text_md(cx))
                                        .into_any_element()
                                }),
                        )
                        // Error banner
                        .when_some(error, |d, msg| {
                            d.child(
                                div()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.term_red))
                                    .child(msg),
                            )
                        })
                        .when(!is_create, |d| {
                            let total = local.len() + remote.len();
                            let local_rows: Vec<AnyElement> = local
                                .iter()
                                .enumerate()
                                .map(|(i, b)| {
                                    let is_current = current.as_deref() == Some(b.as_str());
                                    row(
                                        (*b).clone(),
                                        is_current,
                                        BranchKind::Local,
                                        format!("branch-picker-local-{}", i),
                                        cx,
                                    )
                                })
                                .collect();
                            let remote_rows: Vec<AnyElement> = remote
                                .iter()
                                .enumerate()
                                .map(|(i, b)| {
                                    row(
                                        (*b).clone(),
                                        false,
                                        BranchKind::Remote,
                                        format!("branch-picker-remote-{}", i),
                                        cx,
                                    )
                                })
                                .collect();
                            d.child(
                                v_flex()
                                    .id("branch-picker-list")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .py(px(4.0))
                                    .when(is_loading && total == 0, |d| {
                                        d.child(
                                            div()
                                                .px(px(10.0))
                                                .py(px(8.0))
                                                .text_size(ui_text_sm(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child("Loading\u{2026}"),
                                        )
                                    })
                                    .when(!is_loading && total == 0, |d| {
                                        d.child(
                                            div()
                                                .px(px(10.0))
                                                .py(px(8.0))
                                                .text_size(ui_text_sm(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child(if filter_text.is_empty() {
                                                    "No branches".to_string()
                                                } else {
                                                    format!("No matches for \"{}\"", filter_text)
                                                }),
                                        )
                                    })
                                    .when(!local_rows.is_empty(), |d| {
                                        d.child(section_header("LOCAL", cx))
                                            .children(local_rows)
                                    })
                                    .when(!remote_rows.is_empty(), |d| {
                                        d.child(section_header("REMOTE", cx))
                                            .children(remote_rows)
                                    }),
                            )
                        })
                        .child(
                            h_flex()
                                .px(px(10.0))
                                .py(px(6.0))
                                .gap(px(8.0))
                                .border_t_1()
                                .border_color(rgb(t.border))
                                .items_center()
                                .child({
                                    let label = if is_create { "Cancel" } else { "+ New branch" };
                                    div()
                                        .id("branch-picker-toggle-create")
                                        .cursor_pointer()
                                        .px(px(6.0))
                                        .py(px(3.0))
                                        .rounded(px(4.0))
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.text_secondary))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.toggle_branch_create_mode(window, cx);
                                        }))
                                        .child(label)
                                })
                                .when(is_create, |d| {
                                    d.child(
                                        div()
                                            .id("branch-picker-create-confirm")
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(3.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(t.term_cyan))
                                            .text_size(ui_text_sm(cx))
                                            .text_color(rgb(t.bg_primary))
                                            .opacity(if is_working { 0.5 } else { 1.0 })
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                this.create_branch_from_current(cx);
                                            }))
                                            .child("Create & checkout"),
                                    )
                                })
                                .when(is_working, |d| {
                                    d.child(
                                        div()
                                            .text_size(ui_text_sm(cx))
                                            .text_color(rgb(t.text_muted))
                                            .child("Working\u{2026}"),
                                    )
                                }),
                        ),
                ),
        )
        .into_any_element()
    }

}
