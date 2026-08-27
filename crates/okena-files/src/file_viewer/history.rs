//! Per-file commit history loading, revision navigation, and rail rendering.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::theme::ThemeColors;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};

use super::loading::LoadedContent;
use super::{BlameLoadState, FileHistoryLoadState, FileViewer, FileViewerEvent};
use crate::history::FileHistoryEntry;

const FILE_HISTORY_LIMIT: usize = 200;

impl FileViewer {
    pub(super) fn toggle_history(&mut self, cx: &mut Context<Self>) {
        if self.history_provider.is_none() || self.active_tab().is_empty() {
            return;
        }
        if self.history_visible && self.active_tab().revision.is_some() {
            self.show_working_tree(cx);
        }
        self.history_visible = !self.history_visible;
        if self.history_visible {
            self.spawn_history_load_for_active(cx);
        }
        cx.notify();
    }

    pub(super) fn reload_history(&mut self, cx: &mut Context<Self>) {
        self.active_tab_mut().history = FileHistoryLoadState::NotLoaded;
        self.spawn_history_load_for_active(cx);
    }

    pub(super) fn spawn_history_load_for_active(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.history_provider.clone() else {
            return;
        };
        let tab = self.active_tab();
        if tab.is_empty()
            || tab.is_image
            || tab.is_font
            || matches!(
                tab.history,
                FileHistoryLoadState::Loading | FileHistoryLoadState::Loaded(_)
            )
        {
            return;
        }

        self.next_history_generation = self.next_history_generation.wrapping_add(1);
        let request_generation = self.next_history_generation;
        let scope_generation = self.scope_generation;
        let relative_path = self.active_tab().relative_path.clone();
        let tab = self.active_tab_mut();
        tab.history_generation = request_generation;
        tab.history = FileHistoryLoadState::Loading;
        cx.notify();

        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let path_for_request = relative_path.clone();
            let result = cx
                .background_executor()
                .spawn(
                    async move { provider.get_file_history(&path_for_request, FILE_HISTORY_LIMIT) },
                )
                .await;
            let _ = entity.update(cx, |this, cx| {
                if this.scope_generation != scope_generation {
                    return;
                }
                let Some(tab) = this
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.relative_path == relative_path)
                else {
                    return;
                };
                if tab.history_generation != request_generation {
                    return;
                }
                tab.history = match result {
                    Ok(entries) => FileHistoryLoadState::Loaded(Arc::new(entries)),
                    Err(error) => FileHistoryLoadState::Error(error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn show_revision(&mut self, hash: String, cx: &mut Context<Self>) {
        let Some(provider) = self.history_provider.clone() else {
            return;
        };
        let Some(entry) = self
            .history_entries()
            .and_then(|entries| entries.iter().find(|entry| entry.hash == hash).cloned())
        else {
            return;
        };

        self.next_load_generation = self.next_load_generation.wrapping_add(1);
        let request_generation = self.next_load_generation;
        let scope_generation = self.scope_generation;
        let relative_path = self.active_tab().relative_path.clone();
        let revision_path = entry.path.clone();
        let revision_hash = entry.hash.clone();
        let revision_hash_for_request = revision_hash.clone();
        let tab = self.active_tab_mut();
        tab.load_generation = request_generation;
        tab.revision = Some(entry);
        tab.loading = true;
        tab.error_message = None;
        tab.selection.clear();
        tab.markdown_selection.clear();
        tab.blame = BlameLoadState::NotLoaded;
        cx.notify();

        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    provider.get_file_at_revision(&revision_path, &revision_hash_for_request)
                })
                .await;
            let _ = entity.update(cx, |this, cx| {
                if this.scope_generation != scope_generation {
                    return;
                }
                let Some(tab) = this
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.relative_path == relative_path)
                else {
                    return;
                };
                if tab.load_generation != request_generation
                    || tab.revision.as_ref().map(|entry| entry.hash.as_str())
                        != Some(revision_hash.as_str())
                {
                    return;
                }
                let content = match result {
                    Ok(Some(content)) => Ok(LoadedContent::Text(content)),
                    Ok(None) => Err("File does not exist in this revision".to_string()),
                    Err(error) => Err(error),
                };
                tab.apply_loaded_content(content, None, &this.syntax_set, this.is_dark);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn show_working_tree(&mut self, cx: &mut Context<Self>) {
        if self.active_tab().revision.is_none() {
            return;
        }
        let relative_path = self.active_tab().relative_path.clone();
        self.spawn_tab_load(relative_path, cx);
    }

    pub(super) fn navigate_newer_revision(&mut self, cx: &mut Context<Self>) {
        let Some(entries) = self.history_entries() else {
            return;
        };
        let Some(revision) = self.active_tab().revision.as_ref() else {
            return;
        };
        let Some(index) = entries.iter().position(|entry| entry.hash == revision.hash) else {
            return;
        };
        if index == 0 {
            self.show_working_tree(cx);
        } else {
            self.show_revision(entries[index - 1].hash.clone(), cx);
        }
    }

    pub(super) fn navigate_older_revision(&mut self, cx: &mut Context<Self>) {
        let Some(entries) = self.history_entries() else {
            return;
        };
        let target = match self.active_tab().revision.as_ref() {
            None => entries.first(),
            Some(revision) => entries
                .iter()
                .position(|entry| entry.hash == revision.hash)
                .and_then(|index| entries.get(index + 1)),
        };
        if let Some(entry) = target {
            self.show_revision(entry.hash.clone(), cx);
        }
    }

    fn history_entries(&self) -> Option<Arc<Vec<FileHistoryEntry>>> {
        match &self.active_tab().history {
            FileHistoryLoadState::Loaded(entries) => Some(entries.clone()),
            _ => None,
        }
    }

    fn can_navigate_newer_revision(&self) -> bool {
        let Some(entries) = self.history_entries() else {
            return false;
        };
        self.active_tab()
            .revision
            .as_ref()
            .is_some_and(|revision| entries.iter().any(|entry| entry.hash == revision.hash))
    }

    fn can_navigate_older_revision(&self) -> bool {
        let Some(entries) = self.history_entries() else {
            return false;
        };
        match self.active_tab().revision.as_ref() {
            None => !entries.is_empty(),
            Some(revision) => entries
                .iter()
                .position(|entry| entry.hash == revision.hash)
                .is_some_and(|index| index + 1 < entries.len()),
        }
    }

    pub(super) fn render_revision_bar(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_component::tooltip::Tooltip;

        let revision = self.active_tab().revision.clone();
        let entries = self.history_entries();
        let can_newer = self.can_navigate_newer_revision();
        let can_older = self.can_navigate_older_revision();
        let is_revision = revision.is_some();
        let position = revision
            .as_ref()
            .and_then(|revision| {
                entries.as_ref().and_then(|entries| {
                    entries
                        .iter()
                        .position(|entry| entry.hash == revision.hash)
                        .map(|index| format!("{} / {}", index + 1, entries.len()))
                })
            })
            .unwrap_or_else(|| {
                if is_revision {
                    "Revision".to_string()
                } else {
                    "Working tree".to_string()
                }
            });

        h_flex()
            .px(px(16.0))
            .py(px(6.0))
            .gap(px(10.0))
            .items_center()
            .min_w_0()
            .border_b_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .child(
                h_flex()
                    .flex_shrink_0()
                    .h(px(26.0))
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(rgb(t.border))
                    .child(revision_nav_button(
                        "file-revision-newer",
                        "icons/chevron-left.svg",
                        can_newer,
                        if can_newer {
                            "Newer version"
                        } else {
                            "No newer version"
                        },
                        true,
                        t,
                        cx,
                    ))
                    .child(
                        div()
                            .h_full()
                            .min_w(px(96.0))
                            .px(px(8.0))
                            .border_l_1()
                            .border_r_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(ui_text_sm(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_secondary))
                            .whitespace_nowrap()
                            .child(position),
                    )
                    .child(revision_nav_button(
                        "file-revision-older",
                        "icons/chevron-right.svg",
                        can_older,
                        if can_older {
                            "Older version"
                        } else {
                            "No older version"
                        },
                        false,
                        t,
                        cx,
                    )),
            )
            .when_some(revision, |d, revision| {
                let diff_hash = revision.hash.clone();
                let diff_path = revision.path.clone();
                d.child(div().w(px(1.0)).h(px(16.0)).bg(rgb(t.border)))
                    .child(
                        svg()
                            .path("icons/git-commit.svg")
                            .size(px(13.0))
                            .flex_shrink_0()
                            .text_color(rgb(t.text_muted)),
                    )
                    .child(
                        div()
                            .font_family("monospace")
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.term_yellow))
                            .child(revision.short_hash),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(revision.summary),
                    )
                    .child(
                        div()
                            .id("file-revision-view-diff")
                            .flex_shrink_0()
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_primary))
                            .hover(|style| style.bg(rgb(t.bg_hover)))
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_secondary))
                            .tooltip(|window, cx| {
                                Tooltip::new("View this file's diff").build(window, cx)
                            })
                            .on_click(cx.listener(move |_this, _, _window, cx| {
                                cx.emit(FileViewerEvent::OpenFileDiff {
                                    hash: diff_hash.clone(),
                                    relative_path: diff_path.clone(),
                                });
                            }))
                            .child("View diff"),
                    )
            })
    }

    pub(super) fn render_history_panel(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_component::tooltip::Tooltip;

        let state = self.active_tab().history.clone();
        let selected_hash = self
            .active_tab()
            .revision
            .as_ref()
            .map(|entry| entry.hash.clone());

        v_flex()
            .w(px(292.0))
            .h_full()
            .flex_shrink_0()
            .min_h_0()
            .border_l_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_primary))
            .child(
                h_flex()
                    .h(px(40.0))
                    .px(px(12.0))
                    .justify_between()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(
                        h_flex()
                            .gap(px(6.0))
                            .child(
                                svg()
                                    .path("icons/git-commit.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_muted)),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_ms(cx))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_secondary))
                                    .child("File history"),
                            ),
                    )
                    .child(
                        div()
                            .id("file-history-refresh")
                            .cursor_pointer()
                            .w(px(24.0))
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .hover(|style| style.bg(rgb(t.bg_hover)))
                            .tooltip(|window, cx| Tooltip::new("Refresh history").build(window, cx))
                            .on_click(cx.listener(|this, _, _window, cx| this.reload_history(cx)))
                            .child(
                                svg()
                                    .path("icons/refresh.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_muted)),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("file-history-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(8.0))
                    .child(self.render_working_tree_history_row(selected_hash.is_none(), t, cx))
                    .child(match state {
                        FileHistoryLoadState::NotLoaded | FileHistoryLoadState::Loading => div()
                            .px(px(10.0))
                            .py(px(14.0))
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Loading history…")
                            .into_any_element(),
                        FileHistoryLoadState::Error(error) => div()
                            .px(px(10.0))
                            .py(px(14.0))
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.error))
                            .child(error)
                            .into_any_element(),
                        FileHistoryLoadState::Loaded(entries) if entries.is_empty() => div()
                            .px(px(10.0))
                            .py(px(14.0))
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child("No commits for this file")
                            .into_any_element(),
                        FileHistoryLoadState::Loaded(entries) => v_flex()
                            .children(entries.iter().map(|entry| {
                                self.render_history_row(
                                    entry,
                                    selected_hash.as_deref() == Some(entry.hash.as_str()),
                                    t,
                                    cx,
                                )
                                .into_any_element()
                            }))
                            .into_any_element(),
                    }),
            )
    }

    fn render_working_tree_history_row(
        &self,
        selected: bool,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("file-history-working-tree")
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(8.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .bg(rgb(if selected {
                t.bg_selection
            } else {
                t.bg_primary
            }))
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(|this, _, _window, cx| this.show_working_tree(cx)))
            .child(
                div()
                    .w(px(7.0))
                    .h(px(7.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.term_green)),
            )
            .child(
                v_flex()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .child("Working tree"),
                    )
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Current file on disk"),
                    ),
            )
    }

    fn render_history_row(
        &self,
        entry: &FileHistoryEntry,
        selected: bool,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let revision_hash = entry.hash.clone();
        let diff_hash = entry.hash.clone();
        let diff_path = entry.path.clone();

        h_flex()
            .id(ElementId::Name(
                format!("file-history-{}", entry.hash).into(),
            ))
            .items_start()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(8.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .bg(rgb(if selected {
                t.bg_selection
            } else {
                t.bg_primary
            }))
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.show_revision(revision_hash.clone(), cx);
            }))
            .child(
                svg()
                    .path("icons/git-commit.svg")
                    .mt(px(2.0))
                    .size(px(11.0))
                    .flex_shrink_0()
                    .text_color(rgb(if selected {
                        t.term_yellow
                    } else {
                        t.text_muted
                    })),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_primary))
                            .line_clamp(2)
                            .child(entry.summary.clone()),
                    )
                    .child(
                        h_flex()
                            .gap(px(6.0))
                            .min_w_0()
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.term_yellow))
                                    .child(entry.short_hash.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(entry.author.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(format_relative_time(entry.timestamp)),
                            ),
                    ),
            )
            .child(
                div()
                    .id(ElementId::Name(
                        format!("file-history-diff-{}", entry.hash).into(),
                    ))
                    .flex_shrink_0()
                    .cursor_pointer()
                    .px(px(5.0))
                    .py(px(3.0))
                    .rounded(px(3.0))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .hover(|style| {
                        style
                            .bg(rgb(t.bg_selection))
                            .text_color(rgb(t.text_primary))
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |_this, _, _window, cx| {
                        cx.stop_propagation();
                        cx.emit(FileViewerEvent::OpenFileDiff {
                            hash: diff_hash.clone(),
                            relative_path: diff_path.clone(),
                        });
                    }))
                    .child("Diff"),
            )
    }
}

#[allow(clippy::too_many_arguments)]
fn revision_nav_button(
    id: &'static str,
    icon: &'static str,
    enabled: bool,
    tooltip: &'static str,
    newer: bool,
    t: &ThemeColors,
    cx: &mut Context<FileViewer>,
) -> impl IntoElement {
    use gpui_component::tooltip::Tooltip;

    div()
        .id(id)
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::default()
        })
        .w(px(28.0))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |d| d.hover(|style| style.bg(rgb(t.bg_hover))))
        .when(enabled, |d| {
            if newer {
                d.on_click(cx.listener(|this, _, _window, cx| this.navigate_newer_revision(cx)))
            } else {
                d.on_click(cx.listener(|this, _, _window, cx| this.navigate_older_revision(cx)))
            }
        })
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(
            svg()
                .path(icon)
                .size(px(13.0))
                .text_color(rgb(t.text_secondary))
                .opacity(if enabled { 1.0 } else { 0.35 }),
        )
}

fn format_relative_time(timestamp: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let seconds = (now - timestamp).max(0) as u64;
    match seconds {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        86400..604800 => format!("{}d", seconds / 86400),
        _ => format!("{}w", seconds / 604800),
    }
}

#[cfg(test)]
mod tests {
    use super::format_relative_time;

    #[test]
    fn relative_time_is_compact() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(format_relative_time(now - 90), "1m");
        assert_eq!(format_relative_time(now - 7200), "2h");
    }
}
