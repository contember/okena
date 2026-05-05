//! Context menu and confirmation modals for file/folder actions in the diff viewer.
//!
//! Reachable via right-click on a file or folder in the file-tree sidebar.
//! Offers stage / unstage / discard / copy-path (plus delete for files), depending
//! on the active [`DiffMode`]. Mutations run asynchronously via the active
//! [`GitProvider`] and trigger a diff reload on success. Folder operations reuse
//! the per-path provider methods — `git` handles folder paths recursively.

use gpui::prelude::*;
use gpui::{ClipboardItem, *};
use gpui_component::h_flex;
use okena_core::theme::ThemeColors;
use okena_git::DiffMode;
use okena_ui::button::button;
use okena_ui::icon_button::icon_button_sized;
use okena_ui::menu::{context_menu_panel, menu_item, menu_item_with_color, menu_separator};
use okena_ui::modal::{modal_backdrop, modal_content};
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_xl};
use okena_workspace::toast::ToastManager;

use super::{Cancel, DiffViewer};

/// What a context-menu action targets.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffTargetKind {
    File,
    Folder,
}

impl DiffTargetKind {
    fn noun(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Folder => "Folder",
        }
    }

    fn noun_lower(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Folder => "folder",
        }
    }
}

/// Right-click context menu state. `path` is repo-relative; for folders it has
/// no trailing slash.
pub(crate) struct DiffContextMenu {
    pub position: Point<Pixels>,
    pub path: String,
    pub kind: DiffTargetKind,
}

/// Delete confirmation modal state. Files only — folder delete is intentionally
/// not offered from this menu.
pub(crate) struct DeleteConfirmState {
    pub file_path: String,
    pub error_message: Option<String>,
}

/// Discard confirmation modal state.
pub(crate) struct DiscardConfirmState {
    pub path: String,
    pub kind: DiffTargetKind,
    pub error_message: Option<String>,
}

impl DiffViewer {
    // ── Menu open/close ─────────────────────────────────────────────────────

    pub(super) fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        path: String,
        kind: DiffTargetKind,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some(DiffContextMenu { position, path, kind });
        cx.notify();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu = None;
        cx.notify();
    }

    /// Close the topmost transient UI (confirm modal → menu → selection).
    /// Returns `true` if something was dismissed, so the caller knows not to
    /// close the whole diff viewer.
    pub(super) fn dismiss_transient_ui(&mut self, cx: &mut Context<Self>) -> bool {
        if self.delete_confirm.is_some() {
            self.delete_confirm = None;
            cx.notify();
            return true;
        }
        if self.discard_confirm.is_some() {
            self.discard_confirm = None;
            cx.notify();
            return true;
        }
        if self.context_menu.is_some() {
            self.context_menu = None;
            cx.notify();
            return true;
        }
        false
    }

    // ── Mutation helpers ────────────────────────────────────────────────────

    fn spawn_mutation<F>(&mut self, op: F, success_msg: String, cx: &mut Context<Self>)
    where
        F: FnOnce(&dyn super::provider::GitProvider) -> Result<(), String> + Send + 'static,
    {
        let provider = self.provider.clone();
        let mode = self.diff_mode.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || op(&*provider)).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        ToastManager::success(success_msg, cx);
                        this.load_diff_async(mode, None, cx);
                    }
                    Err(e) => {
                        ToastManager::error(format!("Git operation failed: {}", e), cx);
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn stage_from_menu(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        let path = menu.path.clone();
        let msg = format!("Staged {}", path);
        let path_for_op = path.clone();
        self.spawn_mutation(
            move |provider| provider.stage_file(&path_for_op),
            msg,
            cx,
        );
    }

    pub(super) fn unstage_from_menu(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        let path = menu.path.clone();
        let msg = format!("Unstaged {}", path);
        let path_for_op = path.clone();
        self.spawn_mutation(
            move |provider| provider.unstage_file(&path_for_op),
            msg,
            cx,
        );
    }

    // ── Discard flow ────────────────────────────────────────────────────────

    pub(super) fn start_discard(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        self.discard_confirm = Some(DiscardConfirmState {
            path: menu.path,
            kind: menu.kind,
            error_message: None,
        });
        cx.notify();
    }

    pub(super) fn confirm_discard(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.discard_confirm.take() else {
            return;
        };
        let path = confirm.path.clone();
        let msg = format!("Discarded changes in {}", path);
        let path_for_op = path.clone();
        self.spawn_mutation(
            move |provider| provider.discard_file(&path_for_op),
            msg,
            cx,
        );
    }

    pub(super) fn cancel_discard(&mut self, cx: &mut Context<Self>) {
        self.discard_confirm = None;
        cx.notify();
    }

    // ── Delete flow (files only) ────────────────────────────────────────────

    pub(super) fn start_delete(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        if menu.kind != DiffTargetKind::File {
            return;
        }
        self.delete_confirm = Some(DeleteConfirmState {
            file_path: menu.path,
            error_message: None,
        });
        cx.notify();
    }

    pub(super) fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.delete_confirm.take() else {
            return;
        };
        let file_path = confirm.file_path.clone();
        let msg = format!("Deleted {}", file_path);
        let path_for_op = file_path.clone();
        self.spawn_mutation(
            move |provider| provider.delete_file(&path_for_op),
            msg,
            cx,
        );
    }

    pub(super) fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.delete_confirm = None;
        cx.notify();
    }

    // ── Rendering ───────────────────────────────────────────────────────────

    /// Combine all transient overlays (menu + confirm modals) into one optional
    /// element, so the main render function can add it with a single `.child`.
    pub(super) fn render_context_overlays(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(el) = self.render_delete_confirm(t, cx) {
            return Some(el);
        }
        if let Some(el) = self.render_discard_confirm(t, cx) {
            return Some(el);
        }
        self.render_context_menu(t, cx)
    }

    fn render_context_menu(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.context_menu.as_ref()?;
        let position = menu.position;
        let path = menu.path.clone();
        let kind = menu.kind;

        let abs_path = self.provider.absolute_file_path(&path);

        let mode = self.diff_mode.clone();
        let is_working = matches!(mode, DiffMode::WorkingTree);
        let is_staged = matches!(mode, DiffMode::Staged);

        let mut panel = context_menu_panel("dv-context-menu", t);

        if is_working {
            panel = panel.child(
                menu_item(
                    "dv-ctx-stage",
                    "icons/plus.svg",
                    &format!("Stage {}", kind.noun()),
                    t,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.stage_from_menu(cx);
                })),
            );
            panel = panel.child(
                menu_item_with_color(
                    "dv-ctx-discard",
                    "icons/refresh.svg",
                    "Discard Changes",
                    t.error,
                    t.error,
                    t,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.start_discard(cx);
                })),
            );
            if kind == DiffTargetKind::File {
                panel = panel.child(
                    menu_item_with_color(
                        "dv-ctx-delete",
                        "icons/trash.svg",
                        "Delete File",
                        t.error,
                        t.error,
                        t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_delete(cx);
                    })),
                );
            }
            panel = panel.child(menu_separator(t));
        } else if is_staged {
            panel = panel.child(
                menu_item(
                    "dv-ctx-unstage",
                    "icons/minus.svg",
                    &format!("Unstage {}", kind.noun()),
                    t,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.unstage_from_menu(cx);
                })),
            );
            panel = panel.child(menu_separator(t));
        }

        {
            let rel_for_click = path.clone();
            panel = panel.child(
                menu_item(
                    "dv-ctx-copy-rel-path",
                    "icons/copy.svg",
                    "Copy Relative Path",
                    t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(rel_for_click.clone()));
                    this.close_context_menu(cx);
                })),
            );
        }

        if let Some(abs) = abs_path {
            panel = panel.child(
                menu_item(
                    "dv-ctx-copy-abs-path",
                    "icons/copy.svg",
                    "Copy Absolute Path",
                    t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(abs.clone()));
                    this.close_context_menu(cx);
                })),
            );
        }

        Some(
            div()
                .id("dv-context-menu-backdrop")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_context_menu(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.close_context_menu(cx);
                    }),
                )
                .child(deferred(
                    anchored().position(position).snap_to_window().child(panel),
                ))
                .into_any_element(),
        )
    }

    fn render_delete_confirm(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let confirm = self.delete_confirm.as_ref()?;
        let file_path = confirm.file_path.clone();
        let display_name = file_path
            .rsplit('/')
            .next()
            .unwrap_or(&file_path)
            .to_string();
        let error_msg = confirm.error_message.clone();

        Some(
            modal_backdrop("dv-delete-backdrop", t)
                .items_center()
                .key_context("DiffViewer")
                .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                    this.cancel_delete(cx);
                }))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key.as_str() == "enter" {
                        this.confirm_delete(cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.cancel_delete(cx);
                    }),
                )
                .child(
                    modal_content("dv-delete-dialog", t)
                        .w(px(420.0))
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
                                                .path("icons/trash.svg")
                                                .size(px(16.0))
                                                .text_color(rgb(t.error)),
                                        )
                                        .child(
                                            div()
                                                .text_size(ui_text_xl(cx))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(t.text_primary))
                                                .child("Delete File"),
                                        ),
                                )
                                .child(
                                    icon_button_sized(
                                        "dv-delete-close-btn",
                                        "icons/close.svg",
                                        24.0,
                                        14.0,
                                        t,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_delete(cx);
                                    })),
                                ),
                        )
                        .child(
                            div()
                                .px(px(16.0))
                                .py(px(16.0))
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(t.text_primary))
                                        .child(format!(
                                            "Delete \"{}\" from disk? This cannot be undone.",
                                            display_name
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(file_path.clone()),
                                ),
                        )
                        .when_some(error_msg, |d, msg| {
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
                                    button("dv-delete-cancel", "Cancel", t)
                                        .px(px(16.0))
                                        .py(px(8.0))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_delete(cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id("dv-delete-confirm")
                                        .cursor_pointer()
                                        .px(px(16.0))
                                        .py(px(8.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(t.error))
                                        .hover(|s| s.bg(rgb(t.error)))
                                        .text_size(ui_text_md(cx))
                                        .text_color(gpui::white())
                                        .child("Delete")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_delete(cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_discard_confirm(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let confirm = self.discard_confirm.as_ref()?;
        let path = confirm.path.clone();
        let kind = confirm.kind;
        let display_name = path
            .rsplit('/')
            .next()
            .unwrap_or(&path)
            .to_string();
        let error_msg = confirm.error_message.clone();
        let title = format!("Discard {} Changes", kind.noun());
        let prompt = format!(
            "Discard all working-tree changes in {} \"{}\"? This cannot be undone.",
            kind.noun_lower(),
            display_name,
        );

        Some(
            modal_backdrop("dv-discard-backdrop", t)
                .items_center()
                .key_context("DiffViewer")
                .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                    this.cancel_discard(cx);
                }))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key.as_str() == "enter" {
                        this.confirm_discard(cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.cancel_discard(cx);
                    }),
                )
                .child(
                    modal_content("dv-discard-dialog", t)
                        .w(px(420.0))
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
                                                .path("icons/refresh.svg")
                                                .size(px(16.0))
                                                .text_color(rgb(t.error)),
                                        )
                                        .child(
                                            div()
                                                .text_size(ui_text_xl(cx))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(t.text_primary))
                                                .child(title),
                                        ),
                                )
                                .child(
                                    icon_button_sized(
                                        "dv-discard-close-btn",
                                        "icons/close.svg",
                                        24.0,
                                        14.0,
                                        t,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_discard(cx);
                                    })),
                                ),
                        )
                        .child(
                            div()
                                .px(px(16.0))
                                .py(px(16.0))
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(t.text_primary))
                                        .child(prompt),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(path.clone()),
                                ),
                        )
                        .when_some(error_msg, |d, msg| {
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
                                    button("dv-discard-cancel", "Cancel", t)
                                        .px(px(16.0))
                                        .py(px(8.0))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_discard(cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id("dv-discard-confirm")
                                        .cursor_pointer()
                                        .px(px(16.0))
                                        .py(px(8.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(t.error))
                                        .hover(|s| s.bg(rgb(t.error)))
                                        .text_size(ui_text_md(cx))
                                        .text_color(gpui::white())
                                        .child("Discard")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_discard(cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
