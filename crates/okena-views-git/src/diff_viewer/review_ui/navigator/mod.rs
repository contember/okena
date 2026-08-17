//! Navigator column: Files tree and Attention list — spec §7.
//!
//! `mod.rs` owns the column chrome (segmented control, filter box, Roles button,
//! footer) and the pieces both lists share; `rows` / `items` / `roles` hold the
//! pure view models the two lists render.

mod attention;
mod files;
mod items;
mod roles;
mod roles_menu;
mod rows;

use super::super::DiffViewer;
use super::labels::nav as words;
use super::model::ReasonKind;
use super::state::{FocusRegion, NavRowId, NavigatorMode, RolePreset};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_core::theme::ThemeColors;
use okena_ui::simple_input::SimpleInput;
use okena_ui::tokens::{RADIUS_MD, RADIUS_STD, ui_text_ms, ui_text_sm};

/// One tree row; the list is virtualized, so every row is the same height.
const TREE_ROW_HEIGHT: f32 = 22.0;
/// Attention rows carry two lines of text.
const ITEM_ROW_HEIGHT: f32 = 40.0;
const COLUMN_PADDING: f32 = 8.0;
/// The accent stripe that marks the selected row.
const SELECTION_BAR: f32 = 2.0;

impl DiffViewer {
    pub(crate) fn render_navigator(
        &mut self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.review_init_expanded_dirs();
        let mode = self.review_ui.navigator;
        let (files, items) = self.review_visible_counts();
        let placeholder = match mode {
            NavigatorMode::Files => words::FILTER_PLACEHOLDER_FILES,
            NavigatorMode::Attention => words::FILTER_PLACEHOLDER_ITEMS,
        };
        self.review_ui
            .filter_input
            .update(cx, |input, _cx| input.set_placeholder(placeholder));

        let tabs = self.render_navigator_tabs(files, items, t, cx);
        let filter = self.render_navigator_filter(t, cx);
        let roles = self.render_navigator_roles_row(t, cx);
        let body = match mode {
            NavigatorMode::Files => self.render_files_tree(t, cx),
            NavigatorMode::Attention => self.render_attention_list(t, cx),
        };
        let footer = self.render_navigator_footer(files, items, t, cx);

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(tabs)
            .child(filter)
            .child(roles)
            .child(body)
            .child(footer)
            .into_any_element()
    }

    /// Visible rows of the current navigator mode, in display order.
    pub(crate) fn navigator_row_ids(&self) -> Vec<NavRowId> {
        let Some(model) = self.review_ui.model.as_deref() else {
            return Vec::new();
        };
        let state = &self.review_ui;
        match state.navigator {
            NavigatorMode::Files => rows::nav_rows(
                model,
                &state.role_filter,
                &state.filter_text,
                &state.expanded_dirs,
                state.flatten,
                state.expanded_initialized,
            )
            .into_iter()
            .map(|row| row.id)
            .collect(),
            NavigatorMode::Attention => items::attention_rows(
                model,
                &state.attention_filter,
                &state.role_filter,
                &state.filter_text,
            )
            .into_iter()
            .filter_map(|row| row.id)
            .collect(),
        }
    }

    /// Seed `expanded_dirs` once so the tree the user first sees follows the
    /// default rule and every later toggle is theirs alone — spec §7.
    fn review_init_expanded_dirs(&mut self) {
        if self.review_ui.expanded_initialized {
            return;
        }
        let Some(model) = self.review_ui.model.clone() else {
            return;
        };
        let defaults = rows::default_expanded_dirs(
            &model,
            &self.review_ui.role_filter,
            &self.review_ui.filter_text,
        );
        self.review_ui.expanded_dirs.extend(defaults);
        self.review_ui.expanded_initialized = true;
    }

    /// `(visible files, visible attention items)` — the segmented control counts.
    fn review_visible_counts(&self) -> (usize, usize) {
        let Some(model) = self.review_ui.model.as_deref() else {
            return (0, 0);
        };
        let state = &self.review_ui;
        (
            rows::visible_files(model, &state.role_filter, &state.filter_text).len(),
            items::visible_attention(
                model,
                &state.attention_filter,
                &state.role_filter,
                &state.filter_text,
            )
            .len(),
        )
    }

    fn render_navigator_tabs(
        &self,
        files: usize,
        items: usize,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode = self.review_ui.navigator;
        h_flex()
            .p(px(COLUMN_PADDING))
            .gap(px(4.0))
            .child(
                nav_tab(
                    "review-nav-files",
                    words::FILES_TAB,
                    files,
                    mode == NavigatorMode::Files,
                    t,
                    cx,
                )
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.review_set_navigator(NavigatorMode::Files, cx);
                })),
            )
            .child(
                nav_tab(
                    "review-nav-attention",
                    words::ATTENTION_TAB,
                    items,
                    mode == NavigatorMode::Attention,
                    t,
                    cx,
                )
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.review_set_navigator(NavigatorMode::Attention, cx);
                })),
            )
            .into_any_element()
    }

    fn render_navigator_filter(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .px(px(COLUMN_PADDING))
            .pb(px(6.0))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h(px(24.0))
                    .px(px(6.0))
                    .gap(px(6.0))
                    .items_center()
                    .rounded(RADIUS_STD)
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .child(div().flex_1().min_w_0().child(
                        SimpleInput::new(&self.review_ui.filter_input).text_size(ui_text_ms(cx)),
                    ))
                    .child(
                        div()
                            .px(px(4.0))
                            .rounded(RADIUS_MD)
                            .bg(rgb(t.bg_primary))
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(words::FILTER_KEY_HINT),
                    ),
            )
            .into_any_element()
    }

    /// The one role control, plus `flatten` while the tree is on screen.
    fn render_navigator_roles_row(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.review_ui.role_filter;
        let active = !filter.is_everything();
        let flatten = self.review_ui.flatten;
        h_flex()
            .px(px(COLUMN_PADDING))
            .pb(px(6.0))
            .gap(px(6.0))
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .min_w_0()
                    .gap(px(2.0))
                    .child(
                        h_flex()
                            .id("review-roles-button")
                            .cursor_pointer()
                            .h(px(20.0))
                            .px(px(6.0))
                            .gap(px(4.0))
                            .items_center()
                            .rounded(RADIUS_STD)
                            .bg(rgb(if active {
                                t.bg_selection
                            } else {
                                t.bg_secondary
                            }))
                            .border_1()
                            .border_color(rgb(if active { t.border_active } else { t.border }))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child(words::roles_button(&filter.label()))
                            .child(
                                div()
                                    .text_color(rgb(t.text_muted))
                                    .child(words::CHEVRON_DOWN),
                            )
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.review_toggle_roles_menu(cx);
                            })),
                    )
                    .when(active, |d| {
                        d.child(
                            div()
                                .id("review-roles-clear")
                                .cursor_pointer()
                                .px(px(4.0))
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .hover(|s| s.text_color(rgb(t.text_primary)))
                                .child(words::CLEAR)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.review_clear_role_filter(cx);
                                })),
                        )
                    }),
            )
            .when(self.review_ui.navigator == NavigatorMode::Files, |d| {
                d.child(
                    div()
                        .id("review-flatten")
                        .cursor_pointer()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(if flatten { t.term_blue } else { t.text_muted }))
                        .hover(|s| s.text_color(rgb(t.text_primary)))
                        .child(words::FLATTEN)
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.review_set_flatten(!flatten, cx);
                        })),
                )
            })
            .into_any_element()
    }

    fn render_navigator_footer(
        &self,
        files: usize,
        items: usize,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode = self.review_ui.navigator;
        let filter = self.review_ui.role_filter;
        let role_label = (!filter.is_everything()).then(|| filter.label());
        let (line, right) = match mode {
            NavigatorMode::Files => (
                words::files_footer(
                    files,
                    self.review_file_total(),
                    role_label.as_deref(),
                    self.review_not_analyzed_total(),
                ),
                None,
            ),
            NavigatorMode::Attention => {
                let chips = self.review_active_chip_words();
                (
                    words::attention_footer(
                        items,
                        self.review_attention_total(),
                        &chips,
                        !self.review_ui.attention_filter.include_tests,
                    ),
                    Some(if self.review_ui.attention_filter.grouped_by_file {
                        words::ORDERED_LIST
                    } else {
                        words::GROUP_BY_FILE
                    }),
                )
            }
        };

        h_flex()
            .h(px(24.0))
            .flex_shrink_0()
            .px(px(COLUMN_PADDING))
            .gap(px(8.0))
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(line.text),
            )
            .when_some(line.action, |d, action| {
                d.child(
                    div()
                        .id("review-nav-show-all")
                        .flex_shrink_0()
                        .cursor_pointer()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.term_blue))
                        .hover(|s| s.text_color(rgb(t.text_primary)))
                        .child(action)
                        .on_click(cx.listener(|this, _, _window, cx| this.review_show_all(cx))),
                )
            })
            .when_some(right, |d, label| {
                d.child(
                    div()
                        .id("review-nav-group-toggle")
                        .flex_shrink_0()
                        .cursor_pointer()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.term_blue))
                        .hover(|s| s.text_color(rgb(t.text_primary)))
                        .child(label)
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.review_toggle_group_by_file(cx);
                        })),
                )
            })
            .into_any_element()
    }

    fn review_file_total(&self) -> usize {
        self.review_ui
            .model
            .as_ref()
            .map_or(0, |model| model.files.len())
    }

    fn review_attention_total(&self) -> usize {
        self.review_ui
            .model
            .as_ref()
            .map_or(0, |model| model.attention.len())
    }

    /// Files structure never reached; the footer explains why rows are dim.
    fn review_not_analyzed_total(&self) -> usize {
        self.review_ui.model.as_ref().map_or(0, |model| {
            model
                .files
                .iter()
                .filter(|entry| {
                    entry
                        .reasons
                        .iter()
                        .any(|reason| reason.kind == ReasonKind::NotAnalyzed)
                })
                .count()
        })
    }

    fn review_active_chip_words(&self) -> Vec<&'static str> {
        self.review_ui
            .model
            .as_ref()
            .map_or_else(Vec::new, |model| {
                items::active_chip_words(&items::reason_chips(
                    model,
                    &self.review_ui.attention_filter,
                ))
            })
    }

    /// `show all` — spec §7 puts every filter behind one way back.
    fn review_show_all(&mut self, cx: &mut Context<Self>) {
        self.review_clear_filter(cx);
        self.review_apply_preset(RolePreset::Everything, cx);
    }

    fn review_clear_role_filter(&mut self, cx: &mut Context<Self>) {
        self.review_apply_preset(RolePreset::Everything, cx);
    }

    /// Scroll the cursor row back into view while the navigator drives the keys.
    ///
    /// Re-asserted every render, so with the navigator focused the wheel cannot
    /// leave the cursor off screen; edge-triggering it would need one bit of
    /// state the frozen `ReviewUiState` does not have.
    fn review_keep_cursor_visible(
        &self,
        rows: &[Option<NavRowId>],
        scroll: &UniformListScrollHandle,
    ) {
        if self.review_ui.focus_region != FocusRegion::Navigator {
            return;
        }
        let Some(cursor) = self.review_ui.nav_cursor.as_ref() else {
            return;
        };
        if let Some(index) = rows
            .iter()
            .position(|row| row.as_ref().is_some_and(|id| id == cursor))
        {
            scroll.scroll_to_item(index, ScrollStrategy::Center);
        }
    }
}

fn nav_tab(
    id: &'static str,
    label: &'static str,
    count: usize,
    active: bool,
    t: &ThemeColors,
    cx: &App,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_1()
        .h(px(24.0))
        .gap(px(6.0))
        .items_center()
        .justify_center()
        .rounded(RADIUS_STD)
        .cursor_pointer()
        .bg(rgb(if active { t.bg_header } else { t.bg_secondary }))
        .border_1()
        .border_color(rgb(if active { t.border_active } else { t.border }))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(ui_text_ms(cx))
        .text_color(rgb(if active { t.text_primary } else { t.text_muted }))
        .child(label)
        .child(
            div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(count.to_string()),
        )
}

/// How loud a reason chip reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChipTone {
    Neutral,
    Added,
    Removed,
    Warn,
}

fn chip_tone(kind: ReasonKind) -> ChipTone {
    match kind {
        ReasonKind::PublicRemoved | ReasonKind::Removed | ReasonKind::DeletedImpl => {
            ChipTone::Removed
        }
        ReasonKind::New | ReasonKind::NewPublic => ChipTone::Added,
        ReasonKind::PublicSignature | ReasonKind::ExportedSignature | ReasonKind::NoTestChanges => {
            ChipTone::Warn
        }
        _ => ChipTone::Neutral,
    }
}

fn chip(label: impl Into<SharedString>, tone: ChipTone, t: &ThemeColors, cx: &App) -> Div {
    let color = match tone {
        ChipTone::Neutral => t.text_secondary,
        ChipTone::Added => t.diff_added_fg,
        ChipTone::Removed => t.diff_removed_fg,
        ChipTone::Warn => t.warning,
    };
    div()
        .flex_shrink_0()
        .px(px(4.0))
        .rounded(RADIUS_MD)
        .bg(rgb(t.bg_secondary))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(color))
        .child(label.into())
}

/// `+A −D`; a zero side is left out — spec §2 has no zero cells.
fn churn_cell(added: u64, deleted: u64, t: &ThemeColors, cx: &App) -> Div {
    let (plus, minus) = super::labels::format_signed(added, deleted);
    h_flex()
        .flex_shrink_0()
        .gap(px(4.0))
        .text_size(ui_text_sm(cx))
        .when(added > 0, |d| {
            d.child(div().text_color(rgb(t.diff_added_fg)).child(plus))
        })
        .when(deleted > 0, |d| {
            d.child(div().text_color(rgb(t.diff_removed_fg)).child(minus))
        })
}

/// The selection accent is its own child, never a border: a later
/// `border_color` on the row would repaint a left border away.
fn selection_bar(selected: bool, t: &ThemeColors) -> Div {
    div()
        .w(px(SELECTION_BAR))
        .flex_shrink_0()
        .h_full()
        .when(selected, |d| d.bg(rgb(t.border_active)))
}
