//! Branch switcher popover — filter/select a local or remote branch, or
//! create a new one from the current HEAD.

use super::{BranchKind, BranchNavItem, BranchPickerStatus, GitHeader};

use std::cmp::Reverse;

use okena_core::theme::ThemeColors;
use okena_git::{BranchDetail, BranchList, UpstreamState};
use okena_ui::simple_input::SimpleInput;
use okena_ui::theme::with_alpha;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
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
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.clear_focused_terminal(fm, cx));
        });
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
        self.recompute_branch_filtered(cx);
        cx.notify();

        let provider = self.git_provider.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = smol::unblock(move || provider.list_branches_classified()).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(list) => {
                        this.branch_picker_list = list;
                        if matches!(this.branch_picker_status, BranchPickerStatus::Loading) {
                            this.branch_picker_status = BranchPickerStatus::Idle;
                        }
                    }
                    Err(error) => {
                        this.branch_picker_list = BranchList::default();
                        this.branch_picker_status = BranchPickerStatus::Error(error);
                    }
                }
                this.recompute_branch_filtered(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Rebuild the flat, ordered list of selectable branches from the loaded
    /// branch list and the current filter text, and reset the keyboard
    /// selection to the top. Called on open, after the async load completes,
    /// and on every filter-input change.
    pub(super) fn recompute_branch_filtered(&mut self, cx: &mut Context<Self>) {
        let filter = self.branch_picker_filter.read(cx).value().to_lowercase();
        self.branch_picker_filtered = branch_nav_items(&self.branch_picker_list, &filter);
        self.branch_picker_selected = 0;
        self.branch_picker_scroll.scroll_to_item(0);
    }

    /// Move the keyboard selection up one row.
    fn select_prev_branch(&mut self, cx: &mut Context<Self>) {
        if self.branch_picker_selected > 0 {
            self.branch_picker_selected -= 1;
            self.scroll_branch_into_view();
            cx.notify();
        }
    }

    /// Move the keyboard selection down one row.
    fn select_next_branch(&mut self, cx: &mut Context<Self>) {
        if self.branch_picker_selected + 1 < self.branch_picker_filtered.len() {
            self.branch_picker_selected += 1;
            self.scroll_branch_into_view();
            cx.notify();
        }
    }

    /// Scroll the list so the keyboard-selected row stays visible, accounting
    /// for the interleaved section headers.
    fn scroll_branch_into_view(&self) {
        let local_count = self
            .branch_picker_filtered
            .iter()
            .filter(|b| b.kind == BranchKind::Local)
            .count();
        let child = branch_row_child_index(local_count, self.branch_picker_selected);
        self.branch_picker_scroll.scroll_to_item(child);
    }

    /// Check out the currently keyboard-selected branch (Enter handler).
    fn confirm_branch_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self
            .branch_picker_filtered
            .get(self.branch_picker_selected)
            .cloned()
        {
            self.checkout_branch(item.name, item.kind, cx);
        }
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
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
        });
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
                Ok(()) => {
                    this.hide_branch_picker(cx);
                    this.request_git_refresh(cx);
                }
                Err(e) => {
                    this.branch_picker_status = BranchPickerStatus::Error(e);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Kick the centralized git watcher to repoll this project immediately,
    /// so the branch chip / diff stats update without the usual 5s lag.
    fn request_git_refresh(&self, cx: &mut Context<Self>) {
        if let Some(watcher) = self.git_watcher.as_ref() {
            let project_id = self.project_id.clone();
            watcher.update(cx, |w, cx| w.refresh_project(project_id, cx));
        }
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
            let result =
                smol::unblock(move || provider.create_and_checkout_branch(&name, None)).await;

            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.hide_branch_picker(cx);
                    this.request_git_refresh(cx);
                }
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

        let filter_text = self.branch_picker_filter.read(cx).value().to_string();
        let current = self.branch_picker_list.current.clone();
        let selected = self.branch_picker_selected;
        let scroll = self.branch_picker_scroll.clone();
        // Flat, display-ordered nav list (local-first). Cloned up-front so row
        // building can borrow `cx` mutably without also holding a borrow on
        // `self.branch_picker_filtered`.
        let nav: Vec<(usize, BranchNavItem)> = self
            .branch_picker_filtered
            .iter()
            .cloned()
            .enumerate()
            .collect();
        let local: Vec<(usize, BranchNavItem)> = nav
            .iter()
            .filter(|(_, b)| b.kind == BranchKind::Local)
            .cloned()
            .collect();
        let remote: Vec<(usize, BranchNavItem)> = nav
            .iter()
            .filter(|(_, b)| b.kind == BranchKind::Remote)
            .cloned()
            .collect();
        let is_create = self.branch_picker_create_mode;
        let is_working = matches!(self.branch_picker_status, BranchPickerStatus::Working);
        let is_loading = matches!(self.branch_picker_status, BranchPickerStatus::Loading);
        let error = match &self.branch_picker_status {
            BranchPickerStatus::Error(msg) => Some(msg.clone()),
            _ => None,
        };

        let row = |item: &BranchNavItem,
                   is_selected: bool,
                   key: String,
                   cx: &mut Context<Self>|
         -> AnyElement {
            let BranchNavItem {
                name,
                kind,
                is_current,
                detail,
            } = item.clone();
            let name_for_click = name.clone();
            let is_remote = kind == BranchKind::Remote;
            h_flex()
                .id(ElementId::Name(key.clone().into()))
                .px(px(10.0))
                .py(px(4.0))
                .gap(px(6.0))
                .items_center()
                .cursor_pointer()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(if is_current {
                    t.text_primary
                } else {
                    t.text_secondary
                }))
                .when(is_current, |d| d.font_weight(FontWeight::SEMIBOLD))
                .when(is_selected, |d| d.bg(with_alpha(t.border_active, 0.15)))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .child(
                    svg()
                        .path("icons/git-branch.svg")
                        .size(px(10.0))
                        .text_color(rgb(if is_remote {
                            t.term_green
                        } else {
                            t.text_muted
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .overflow_hidden()
                        // Without this a name with a `/` wraps to a second line
                        // instead of truncating, and the meta column jumps.
                        .whitespace_nowrap()
                        .child(name),
                )
                .when(is_current, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.term_cyan))
                            .child("HEAD"),
                    )
                })
                .children(branch_meta(&detail, kind, &key, t, cx))
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
            anchored().position(position).snap_to_window().child(
                v_flex()
                    .id("branch-picker-popover")
                    .occlude()
                    .w(px(420.0))
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
                    // Keyboard navigation. The focused filter/create input
                    // leaves arrows, Enter and Escape unhandled (it returns
                    // `KeyHandled::Ignored`/`NotHandled` without stopping
                    // propagation), so they bubble up to this popover.
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        let key = event.keystroke.key.as_str();
                        if this.branch_picker_create_mode {
                            match key {
                                "enter" => {
                                    this.create_branch_from_current(cx);
                                    cx.stop_propagation();
                                }
                                "escape" => {
                                    this.hide_branch_picker(cx);
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }
                            return;
                        }
                        match key {
                            "up" => {
                                this.select_prev_branch(cx);
                                cx.stop_propagation();
                            }
                            "down" => {
                                this.select_next_branch(cx);
                                cx.stop_propagation();
                            }
                            "enter" => {
                                this.confirm_branch_selection(cx);
                                cx.stop_propagation();
                            }
                            "escape" => {
                                this.hide_branch_picker(cx);
                                cx.stop_propagation();
                            }
                            _ => {}
                        }
                    }))
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
                                                current
                                                    .clone()
                                                    .unwrap_or_else(|| "HEAD".to_string())
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
                        let total = nav.len();
                        let local_rows: Vec<AnyElement> = local
                            .iter()
                            .map(|(flat, b)| {
                                row(
                                    b,
                                    *flat == selected,
                                    format!("branch-picker-row-{}", flat),
                                    cx,
                                )
                            })
                            .collect();
                        let remote_rows: Vec<AnyElement> = remote
                            .iter()
                            .map(|(flat, b)| {
                                row(
                                    b,
                                    *flat == selected,
                                    format!("branch-picker-row-{}", flat),
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
                                .track_scroll(&scroll)
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
                                    d.child(section_header("LOCAL", cx)).children(local_rows)
                                })
                                .when(!remote_rows.is_empty(), |d| {
                                    d.child(section_header("REMOTE", cx)).children(remote_rows)
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

/// Build the flat, display-ordered nav list from a loaded branch list and a
/// lowercased filter string: locals first, then remotes.
///
/// Within each section the most recently committed branch comes first, which
/// is the order people actually work in; branches with no reported tip time
/// (older hosts, metadata unavailable) sink to the bottom of their section.
/// The current branch overrides that and leads the LOCAL section — it is what
/// users scan for, and putting it on top also makes it the default keyboard
/// selection.
fn branch_nav_items(list: &BranchList, filter: &str) -> Vec<BranchNavItem> {
    let is_current = |b: &str| list.current.as_deref() == Some(b);
    let matches = |b: &str| filter.is_empty() || b.to_lowercase().contains(filter);
    let detail = |b: &str| list.details.get(b).cloned().unwrap_or_default();
    let tip_time = |b: &str| {
        list.details
            .get(b)
            .and_then(|d| d.committed_at)
            .unwrap_or(i64::MIN)
    };

    let mut local: Vec<&String> = list.local.iter().collect();
    local.sort_by_key(|b| (!is_current(b), Reverse(tip_time(b))));
    let mut remote: Vec<&String> = list.remote.iter().collect();
    remote.sort_by_key(|b| Reverse(tip_time(b)));

    local
        .into_iter()
        .filter(|b| matches(b))
        .map(|b| BranchNavItem {
            name: b.clone(),
            kind: BranchKind::Local,
            is_current: is_current(b),
            detail: detail(b),
        })
        .chain(
            remote
                .into_iter()
                .filter(|b| matches(b))
                .map(|b| BranchNavItem {
                    name: b.clone(),
                    kind: BranchKind::Remote,
                    is_current: false,
                    detail: detail(b),
                }),
        )
        .collect()
}

/// Right-hand metadata cluster for a branch row: the worktree holding the
/// branch, how it sits against its upstream, and how recently it moved.
///
/// Each part is omitted when it has nothing to say, so an ordinary in-sync
/// branch shows only its age. Upstream state is local-only — a remote ref does
/// not track anything.
fn branch_meta(
    detail: &BranchDetail,
    kind: BranchKind,
    key: &str,
    t: &ThemeColors,
    cx: &App,
) -> Vec<AnyElement> {
    let mut parts: Vec<AnyElement> = Vec::new();

    // A branch checked out elsewhere cannot be checked out here — git refuses
    // it — so name the worktree before the user tries.
    if let Some(path) = detail.worktree.as_deref() {
        let tooltip = format!("Checked out in {path}");
        let label = path
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or(path)
            .to_string();
        parts.push(
            h_flex()
                .id(ElementId::Name(format!("{key}-worktree").into()))
                .flex_shrink_0()
                .gap(px(3.0))
                .items_center()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.term_yellow))
                .child(
                    svg()
                        .path("icons/folder.svg")
                        .size(px(9.0))
                        .text_color(rgb(t.term_yellow)),
                )
                .child(
                    div()
                        .max_w(px(70.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                )
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .into_any_element(),
        );
    }

    if kind == BranchKind::Local {
        match &detail.upstream {
            UpstreamState::Untracked => parts.push(
                div()
                    .id(ElementId::Name(format!("{key}-untracked").into()))
                    .flex_shrink_0()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child("local")
                    .tooltip(|window, cx| {
                        Tooltip::new("No upstream — never pushed").build(window, cx)
                    })
                    .into_any_element(),
            ),
            UpstreamState::Gone => parts.push(
                div()
                    .id(ElementId::Name(format!("{key}-gone").into()))
                    .flex_shrink_0()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.term_red))
                    .child("gone")
                    .tooltip(|window, cx| {
                        Tooltip::new("Upstream branch no longer exists on the remote")
                            .build(window, cx)
                    })
                    .into_any_element(),
            ),
            UpstreamState::Tracked {
                name,
                ahead,
                behind,
            } => {
                let (ahead, behind) = (*ahead, *behind);
                if ahead > 0 || behind > 0 {
                    let tooltip = {
                        let mut lines = Vec::new();
                        if ahead > 0 {
                            lines.push(format!("{ahead} ahead of {name}"));
                        }
                        if behind > 0 {
                            lines.push(format!("{behind} behind {name}"));
                        }
                        lines.join("\n")
                    };
                    parts.push(
                        h_flex()
                            .id(ElementId::Name(format!("{key}-track").into()))
                            .flex_shrink_0()
                            .gap(px(4.0))
                            .items_center()
                            .text_size(ui_text_sm(cx))
                            .when(ahead > 0, |d| {
                                d.child(
                                    div()
                                        .text_color(rgb(t.term_green))
                                        .child(format!("\u{2191}{ahead}")),
                                )
                            })
                            .when(behind > 0, |d| {
                                d.child(
                                    div()
                                        .text_color(rgb(t.term_yellow))
                                        .child(format!("\u{2193}{behind}")),
                                )
                            })
                            .tooltip(move |window, cx| {
                                Tooltip::new(tooltip.clone()).build(window, cx)
                            })
                            .into_any_element(),
                    );
                }
            }
        }
    }

    if let Some(timestamp) = detail.committed_at {
        parts.push(
            div()
                .flex_shrink_0()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(okena_git::format_relative_time(timestamp))
                .into_any_element(),
        );
    }

    parts
}

/// Map a flat selection index (local-first) to its child position within the
/// scroll container, so `ScrollHandle::scroll_to_item` lands on the right row.
///
/// The list interleaves section headers with rows: a "LOCAL" header (present
/// only when there are local rows) followed by the local rows, then a "REMOTE"
/// header followed by the remote rows. `local_count` is the number of local
/// rows currently shown.
fn branch_row_child_index(local_count: usize, selected: usize) -> usize {
    if selected < local_count {
        // [LOCAL header][local 0..local_count] — header occupies child 0.
        1 + selected
    } else {
        // [..local block..][REMOTE header][remote 0..] — header precedes rows.
        let remote_offset = selected - local_count;
        let local_block = if local_count > 0 { local_count + 1 } else { 0 };
        local_block + 1 + remote_offset
    }
}

#[cfg(test)]
mod tests {
    use super::{BranchDetail, BranchKind, BranchList, branch_nav_items, branch_row_child_index};

    /// Build a list whose branches carry no metadata — the pre-metadata host
    /// case, where display order falls back to git's ref order.
    fn list(current: Option<&str>, local: &[&str], remote: &[&str]) -> BranchList {
        BranchList {
            local: local.iter().map(|s| s.to_string()).collect(),
            remote: remote.iter().map(|s| s.to_string()).collect(),
            current: current.map(|s| s.to_string()),
            details: Default::default(),
        }
    }

    /// Same, with a tip timestamp per branch name.
    fn list_with_times(
        current: Option<&str>,
        local: &[(&str, i64)],
        remote: &[(&str, i64)],
    ) -> BranchList {
        let mut list = list(
            current,
            &local.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            &remote.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        );
        list.details = local
            .iter()
            .chain(remote)
            .map(|(name, at)| {
                (
                    name.to_string(),
                    BranchDetail {
                        committed_at: Some(*at),
                        ..Default::default()
                    },
                )
            })
            .collect();
        list
    }

    fn rows(items: &[super::BranchNavItem]) -> Vec<(&str, BranchKind, bool)> {
        items
            .iter()
            .map(|b| (b.name.as_str(), b.kind, b.is_current))
            .collect()
    }

    #[test]
    fn current_branch_leads_the_local_section() {
        let items = branch_nav_items(&list(Some("feature"), &["main", "feature", "wip"], &[]), "");
        assert_eq!(
            rows(&items),
            vec![
                ("feature", BranchKind::Local, true),
                ("main", BranchKind::Local, false),
                ("wip", BranchKind::Local, false),
            ]
        );
    }

    #[test]
    fn remotes_follow_locals_and_detached_head_keeps_order() {
        let items = branch_nav_items(&list(None, &["main", "wip"], &["origin/release"]), "");
        assert_eq!(
            rows(&items),
            vec![
                ("main", BranchKind::Local, false),
                ("wip", BranchKind::Local, false),
                ("origin/release", BranchKind::Remote, false),
            ]
        );
    }

    #[test]
    fn filter_matches_case_insensitively_and_drops_the_current_branch() {
        let items = branch_nav_items(
            &list(Some("feature"), &["main", "feature"], &["origin/Main-2"]),
            "main",
        );
        assert_eq!(
            rows(&items),
            vec![
                ("main", BranchKind::Local, false),
                ("origin/Main-2", BranchKind::Remote, false),
            ]
        );
    }

    #[test]
    fn sections_are_ordered_by_recency_behind_the_current_branch() {
        let items = branch_nav_items(
            &list_with_times(
                Some("feature"),
                &[("main", 300), ("feature", 100), ("wip", 200)],
                &[("origin/old", 50), ("origin/new", 400)],
            ),
            "",
        );
        assert_eq!(
            rows(&items),
            vec![
                // Current branch wins over its older tip time.
                ("feature", BranchKind::Local, true),
                ("main", BranchKind::Local, false),
                ("wip", BranchKind::Local, false),
                ("origin/new", BranchKind::Remote, false),
                ("origin/old", BranchKind::Remote, false),
            ]
        );
    }

    #[test]
    fn branches_without_a_tip_time_sink_below_dated_ones() {
        let mut list = list_with_times(None, &[("dated", 100)], &[]);
        list.local.push("undated".to_string());
        // Git's ref order puts `undated` last anyway, so check the reverse too.
        list.local.reverse();

        let items = branch_nav_items(&list, "");

        assert_eq!(
            rows(&items),
            vec![
                ("dated", BranchKind::Local, false),
                ("undated", BranchKind::Local, false),
            ]
        );
    }

    #[test]
    fn child_index_within_local_section() {
        // 3 local rows: LOCAL header at child 0, rows at children 1, 2, 3.
        assert_eq!(branch_row_child_index(3, 0), 1);
        assert_eq!(branch_row_child_index(3, 2), 3);
    }

    #[test]
    fn child_index_remote_section_after_local() {
        // 3 local rows occupy children 0..=3 (header + 3 rows); the REMOTE
        // header sits at child 4 and remote rows start at child 5.
        assert_eq!(branch_row_child_index(3, 3), 5); // first remote
        assert_eq!(branch_row_child_index(3, 5), 7);
    }

    #[test]
    fn child_index_remote_only() {
        // No local rows → no LOCAL header. REMOTE header at child 0, rows at 1+.
        assert_eq!(branch_row_child_index(0, 0), 1);
        assert_eq!(branch_row_child_index(0, 3), 4);
    }
}
