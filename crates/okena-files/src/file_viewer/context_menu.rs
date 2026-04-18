//! Context menu, inline rename, and delete confirmation for the file tree sidebar.

use crate::file_search::Cancel;
use gpui::prelude::*;
use gpui::{ClipboardItem, *};
use gpui_component::h_flex;
use okena_ui::button::button;
use okena_ui::icon_button::icon_button_sized;
use okena_ui::menu::{context_menu_panel, menu_item, menu_item_with_color, menu_separator};
use okena_ui::modal::{modal_backdrop, modal_content};
use okena_ui::rename_state::{start_rename_with_blur, RenameState};
use okena_ui::simple_input::SimpleInput;
use okena_ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_xl};
use std::path::{Path, PathBuf};

use super::FileViewer;

/// What kind of tree node was right-clicked.
pub(crate) enum TreeNodeTarget {
    File { path: PathBuf },
    Folder { folder_path: String, abs_path: PathBuf },
}

impl TreeNodeTarget {
    pub(crate) fn display_name(&self) -> String {
        self.abs_path()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    }

    pub(crate) fn abs_path(&self) -> &Path {
        match self {
            Self::File { path, .. } => path,
            Self::Folder { abs_path, .. } => abs_path,
        }
    }

    pub(crate) fn is_folder(&self) -> bool {
        matches!(self, Self::Folder { .. })
    }

    /// Check if this target matches a file path.
    pub(crate) fn matches_file(&self, file_path: &Path) -> bool {
        matches!(self, Self::File { path, .. } if path == file_path)
    }

    /// Check if this target matches a folder path (relative).
    pub(crate) fn matches_folder(&self, rel_folder_path: &str) -> bool {
        matches!(self, Self::Folder { folder_path, .. } if folder_path == rel_folder_path)
    }
}

/// Open context menu state.
pub(crate) struct FileTreeContextMenu {
    pub position: Point<Pixels>,
    pub target: TreeNodeTarget,
}

/// Tab context menu state (right-click on a file viewer tab).
pub(crate) struct TabContextMenu {
    pub position: Point<Pixels>,
    pub tab_index: usize,
}

/// Inline rename state wrapping the reusable RenameState.
pub(crate) struct FileRenameState {
    pub target: TreeNodeTarget,
    pub rename: RenameState<()>,
}

/// Delete confirmation state.
pub(crate) struct DeleteConfirmState {
    pub target: TreeNodeTarget,
    pub error_message: Option<String>,
}

// ============================================================================
// Context menu actions
// ============================================================================

impl FileViewer {
    pub(super) fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        target: TreeNodeTarget,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some(FileTreeContextMenu { position, target });
        cx.notify();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu = None;
        cx.notify();
    }

    /// Check if a file row is the context menu target (for highlighting).
    pub(super) fn is_context_menu_target_file(&self, file_path: &Path) -> bool {
        self.context_menu
            .as_ref()
            .map_or(false, |m| m.target.matches_file(file_path))
    }

    /// Check if a folder row is the context menu target (for highlighting).
    pub(super) fn is_context_menu_target_folder(&self, folder_path: &str) -> bool {
        self.context_menu
            .as_ref()
            .map_or(false, |m| m.target.matches_folder(folder_path))
    }

    // ========================================================================
    // Inline rename
    // ========================================================================

    pub(super) fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };

        let current_name = menu.target.display_name();
        let rename = start_rename_with_blur(
            (),
            &current_name,
            "New name...",
            |this: &mut Self, _window, cx| {
                this.finish_rename(cx);
            },
            window,
            cx,
        );

        self.rename_state = Some(FileRenameState {
            target: menu.target,
            rename,
        });
        cx.notify();
    }

    pub(super) fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.rename_state.take() else {
            return;
        };

        let new_name = state.rename.value(cx).trim().to_string();
        if new_name.is_empty() {
            cx.notify();
            return;
        }

        let old_path = state.target.abs_path().to_path_buf();
        let Some(parent) = old_path.parent() else {
            cx.notify();
            return;
        };
        let new_path = parent.join(&new_name);
        let current_name = state.target.display_name();

        if new_name == current_name {
            cx.notify();
            return;
        }

        if new_path.exists() {
            cx.notify();
            return;
        }

        if let Err(_e) = std::fs::rename(&old_path, &new_path) {
            cx.notify();
            return;
        }

        self.update_tabs_after_rename(&old_path, &new_path);
        self.update_expanded_after_rename(&old_path, &new_path);

        self.refresh_file_tree_async(cx);
        cx.notify();
    }

    pub(super) fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.rename_state = None;
        cx.notify();
    }

    /// Check if a file is currently being renamed.
    pub(super) fn is_renaming_file(&self, file_path: &Path) -> bool {
        self.rename_state
            .as_ref()
            .map_or(false, |s| s.target.matches_file(file_path))
    }

    /// Check if a folder is currently being renamed.
    pub(super) fn is_renaming_folder(&self, folder_path: &str) -> bool {
        self.rename_state
            .as_ref()
            .map_or(false, |s| s.target.matches_folder(folder_path))
    }

    /// Render inline rename input for a tree row.
    pub(super) fn render_rename_input(
        &self,
        t: &okena_core::theme::ThemeColors,
        cx: &App,
    ) -> Option<AnyElement> {
        let state = self.rename_state.as_ref()?;
        let input = &state.rename.input;
        Some(
            div()
                .id("fv-rename-input")
                .flex_1()
                .min_w_0()
                .bg(rgb(t.bg_hover))
                .rounded(px(2.0))
                .child(SimpleInput::new(input).text_size(ui_text(13.0, cx)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(|_, _window, cx| {
                    cx.stop_propagation();
                })
                .into_any_element(),
        )
    }

    fn update_tabs_after_rename(&mut self, old_path: &Path, new_path: &Path) {
        for tab in &mut self.tabs {
            if tab.file_path == *old_path {
                tab.file_path = new_path.to_path_buf();
            } else if tab.file_path.starts_with(old_path) {
                if let Ok(relative) = tab.file_path.strip_prefix(old_path) {
                    tab.file_path = new_path.join(relative);
                }
            }
        }
    }

    fn update_expanded_after_rename(&mut self, old_path: &Path, new_path: &Path) {
        let project_path = PathBuf::from(self.project_fs.project_id());
        let old_rel = match old_path.strip_prefix(&project_path) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => return,
        };
        let new_rel = match new_path.strip_prefix(&project_path) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => return,
        };

        let old_prefix = format!("{}/", old_rel);
        let to_update: Vec<String> = self
            .expanded_folders
            .iter()
            .filter(|p| *p == &old_rel || p.starts_with(&old_prefix))
            .cloned()
            .collect();

        for old in to_update {
            self.expanded_folders.remove(&old);
            let updated = old.replacen(&old_rel, &new_rel, 1);
            self.expanded_folders.insert(updated);
        }
    }

    // ========================================================================
    // Delete
    // ========================================================================

    pub(super) fn start_delete(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };

        self.delete_confirm = Some(DeleteConfirmState {
            target: menu.target,
            error_message: None,
        });
        cx.notify();
    }

    pub(super) fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(mut confirm) = self.delete_confirm.take() else {
            return;
        };

        let result = match &confirm.target {
            TreeNodeTarget::File { path, .. } => std::fs::remove_file(path),
            TreeNodeTarget::Folder { abs_path, .. } => std::fs::remove_dir_all(abs_path),
        };

        if let Err(e) = result {
            confirm.error_message = Some(format!("Failed to delete: {}", e));
            self.delete_confirm = Some(confirm);
            cx.notify();
            return;
        }

        self.close_tabs_for_deleted(&confirm.target, cx);

        self.refresh_file_tree_async(cx);
        cx.notify();
    }

    pub(super) fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.delete_confirm = None;
        cx.notify();
    }

    fn close_tabs_for_deleted(&mut self, target: &TreeNodeTarget, _cx: &mut Context<Self>) {
        let target_path = target.abs_path();
        let is_folder = target.is_folder();

        // Remove matching tabs directly without emitting Close event
        // (close_tab emits Close when it's the last tab, which would close the viewer)
        self.tabs.retain(|t| {
            if is_folder {
                !t.file_path.starts_with(target_path)
            } else {
                t.file_path != target_path
            }
        });

        // Ensure there's always at least one tab
        if self.tabs.is_empty() {
            self.tabs.push(super::FileViewerTab::new_empty());
            self.active_tab = 0;
        } else {
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        }
    }

    // ========================================================================
    // Render helpers
    // ========================================================================

    pub(super) fn render_context_menu(
        &self,
        t: &okena_core::theme::ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.context_menu.as_ref()?;
        let position = menu.position;
        let is_folder = menu.target.is_folder();

        let abs_path = menu.target.abs_path().to_string_lossy().to_string();
        let project_root = PathBuf::from(self.project_fs.project_id());
        let rel_path = menu
            .target
            .abs_path()
            .strip_prefix(&project_root)
            .unwrap_or(menu.target.abs_path())
            .to_string_lossy()
            .to_string();

        Some(
            div()
                .id("fv-context-menu-backdrop")
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
                    anchored().position(position).snap_to_window().child(
                        context_menu_panel("fv-tree-context-menu", t)
                            .child(
                                menu_item("fv-ctx-rename", "icons/edit.svg", "Rename", t)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.start_rename(window, cx);
                                    })),
                            )
                            .child(
                                menu_item(
                                    "fv-ctx-copy-rel-path",
                                    "icons/copy.svg",
                                    "Copy Relative Path",
                                    t,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        rel_path.clone(),
                                    ));
                                    this.close_context_menu(cx);
                                })),
                            )
                            .child(
                                menu_item(
                                    "fv-ctx-copy-abs-path",
                                    "icons/copy.svg",
                                    "Copy Absolute Path",
                                    t,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        abs_path.clone(),
                                    ));
                                    this.close_context_menu(cx);
                                })),
                            )
                            .child(menu_separator(t))
                            .child(
                                menu_item_with_color(
                                    "fv-ctx-delete",
                                    "icons/trash.svg",
                                    if is_folder {
                                        "Delete Folder"
                                    } else {
                                        "Delete"
                                    },
                                    t.error,
                                    t.error,
                                    t,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_delete(cx);
                                })),
                            ),
                    ),
                ))
                .into_any_element(),
        )
    }

    pub(super) fn render_tab_context_menu(
        &self,
        t: &okena_core::theme::ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.tab_context_menu.as_ref()?;
        let position = menu.position;
        let tab_index = menu.tab_index;

        Some(
            div()
                .id("fv-tab-context-menu-backdrop")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.tab_context_menu = None;
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.tab_context_menu = None;
                        cx.notify();
                    }),
                )
                .child(deferred(
                    anchored().position(position).snap_to_window().child(
                        context_menu_panel("fv-tab-context-menu", t)
                            .child(
                                menu_item("fv-tab-ctx-close", "icons/close.svg", "Close", t)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.tab_context_menu = None;
                                        this.close_tab(tab_index, cx);
                                    })),
                            )
                            .child(
                                menu_item(
                                    "fv-tab-ctx-close-others",
                                    "icons/close.svg",
                                    "Close Others",
                                    t,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.tab_context_menu = None;
                                    this.close_other_tabs(tab_index, cx);
                                })),
                            )
                            .child(
                                menu_item(
                                    "fv-tab-ctx-close-all",
                                    "icons/close.svg",
                                    "Close All",
                                    t,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.tab_context_menu = None;
                                    this.close_all_tabs(cx);
                                })),
                            ),
                    ),
                ))
                .into_any_element(),
        )
    }

    pub(super) fn render_delete_confirm(
        &self,
        t: &okena_core::theme::ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let confirm = self.delete_confirm.as_ref()?;
        let display_name = confirm.target.display_name();
        let is_directory = confirm.target.is_folder();
        let error_msg = confirm.error_message.clone();

        Some(
            modal_backdrop("fv-delete-backdrop", t)
                .items_center()
                .key_context("FileViewerDelete")
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
                    modal_content("fv-delete-dialog", t)
                        .w(px(400.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
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
                                                .child(if is_directory {
                                                    "Delete Folder"
                                                } else {
                                                    "Delete File"
                                                }),
                                        ),
                                )
                                .child(
                                    icon_button_sized("fv-delete-close-btn", "icons/close.svg", 24.0, 14.0, t)
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
                                        .child(format!("Are you sure you want to delete \"{}\"?", display_name)),
                                )
                                .when(is_directory, |d| {
                                    d.child(
                                        div()
                                            .text_size(ui_text_ms(cx))
                                            .text_color(rgb(t.error))
                                            .child("This will permanently delete the folder and all its contents."),
                                    )
                                }),
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
                                    button("fv-delete-cancel", "Cancel", t)
                                        .px(px(16.0))
                                        .py(px(8.0))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_delete(cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id("fv-delete-confirm")
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
}
