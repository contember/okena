//! Sidebar file-tree rendering for the expanded content-search dialog.

use super::{ContentSearchDialog, ResultRow};
use crate::file_tree::{FileTreeNode, build_file_tree, expandable_file_row, expandable_folder_row};
use crate::theme::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_ui::tokens::{ui_text_ms, ui_text_sm};

impl ContentSearchDialog {
    /// Render the sidebar file tree for expanded mode.
    /// Shows only files/folders that have search results.
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);

        // Build tree from matched files only
        let matched_files: Vec<(usize, &str)> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| match row {
                ResultRow::FileHeader { relative_path, .. } => Some((i, relative_path.as_str())),
                _ => None,
            })
            .collect();
        let result_tree = build_file_tree(matched_files.into_iter());
        let tree_elements = self.render_tree_node(&result_tree, 0, "", &t, cx);

        div()
            .w(px(240.0))
            .h_full()
            .border_r_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_primary))
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_secondary))
                            .child("Scope"),
                    )
                    .when_some(self.scope_path.clone(), |d, _| {
                        d.child(
                            div()
                                .id("clear-scope")
                                .cursor_pointer()
                                .text_size(ui_text_sm(cx))
                                .text_color(rgb(t.text_muted))
                                .hover(|s| s.text_color(rgb(t.text_primary)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        this.set_scope(None, cx);
                                    }),
                                )
                                .child("clear"),
                        )
                    }),
            )
            .child(
                div()
                    .id("scope-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.tree_scroll_handle)
                    .py(px(6.0))
                    .children(tree_elements),
            )
    }

    /// Recursively render file tree nodes for the sidebar.
    /// Matches FileViewer's tree style (chevrons, folder icons, sizing).
    fn render_tree_node(
        &self,
        node: &FileTreeNode,
        depth: usize,
        parent_path: &str,
        t: &okena_core::theme::ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut elements = Vec::new();

        for (name, child) in &node.children {
            let folder_path = if parent_path.is_empty() {
                name.clone()
            } else {
                format!("{parent_path}/{name}")
            };
            let is_expanded = self.expanded_folders.contains(&folder_path);
            let is_scoped = self.scope_path.as_ref() == Some(&folder_path);
            let fp_toggle = folder_path.clone();
            let fp_scope = folder_path.clone();

            elements.push(
                expandable_folder_row(name, depth, is_expanded, t, cx)
                    .id(ElementId::Name(format!("cs-folder-{}", folder_path).into()))
                    .when(is_scoped, |d| d.bg(rgb(t.bg_selection)))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.toggle_folder(&fp_toggle, cx);
                    }))
                    // Scope button
                    .child(
                        div()
                            .id(ElementId::Name(format!("scope-folder-{}", folder_path).into()))
                            .cursor_pointer()
                            .px(px(4.0))
                            .py(px(2.0))
                            .rounded(px(3.0))
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(if is_scoped { t.text_primary } else { t.text_muted }))
                            .when(is_scoped, |d| d.bg(rgb(t.border_active)))
                            .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(t.text_primary)))
                            .flex_shrink_0()
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                if this.scope_path.as_ref() == Some(&fp_scope) {
                                    this.set_scope(None, cx);
                                } else {
                                    this.set_scope(Some(fp_scope.clone()), cx);
                                }
                            }))
                            .child(if is_scoped { "scoped" } else { "scope" }),
                    )
                    .into_any_element(),
            );

            if is_expanded {
                elements.extend(self.render_tree_node(child, depth + 1, &folder_path, t, cx));
            }
        }

        for &row_index in &node.files {
            if let Some(ResultRow::FileHeader {
                relative_path,
                match_count,
                ..
            }) = self.rows.get(row_index)
            {
                let filename = std::path::Path::new(relative_path.as_str())
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| relative_path.clone());
                let rel = relative_path.clone();
                let is_scoped = self.scope_path.as_ref() == Some(&rel);
                let count = *match_count;

                elements.push(
                    expandable_file_row(&filename, depth, None, false, t, cx)
                        .id(ElementId::Name(format!("cs-file-{}", row_index).into()))
                        .when(is_scoped, |d| d.bg(rgb(t.bg_selection)))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            if this.scope_path.as_ref() == Some(&rel) {
                                this.set_scope(None, cx);
                            } else {
                                this.set_scope(Some(rel.clone()), cx);
                            }
                        }))
                        .child(
                            div()
                                .text_size(ui_text_sm(cx))
                                .text_color(rgb(t.text_muted))
                                .flex_shrink_0()
                                .ml(px(4.0))
                                .child(count.to_string()),
                        )
                        .into_any_element(),
                );
            }
        }

        elements
    }
}
