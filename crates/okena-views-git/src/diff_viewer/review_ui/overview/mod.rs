//! Overview: change at a glance, the facts, and "Start here" — spec §8.

mod facts;
mod glance;
mod start_here;

use self::facts::{FactLine, FactLink, also_roles, fact_sentences, ledger_rows};
use self::glance::{Headline, headline, is_narrow, legend_rows};
use self::start_here::{ChipTone, caveat, chip_tone, row_tone, start_here};
use super::super::DiffViewer;
use super::labels::facts as words;
use super::labels::status as status_words;
use super::labels::{format_lines, format_signed, glyph, relative_time, role_label};
use super::model::{AttentionItem, AttentionTarget, CommitRow, Reason, ReviewModel, VolumeRow};
use super::state::{NavigatorMode, RoleFilter, RolePreset, RoleSet};
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_core::review::FileRole;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::{ui_text, ui_text_ms, ui_text_sm};

/// The page stops growing here; a wider window gets margin, not longer rows.
const CONTENT_WIDTH: Pixels = px(1080.0);
/// The sidebar holding the composition and the facts; the list takes the rest.
const SIDE_WIDTH: Pixels = px(380.0);
/// The legend stops here even when the block is wider: a role and its numbers
/// belong to each other, and a full-width row pulls them apart.
const LEGEND_WIDTH: Pixels = px(420.0);
const HEADLINE_SIZE: f32 = 17.0;

impl DiffViewer {
    pub(crate) fn render_overview(
        &mut self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(model) = self.review_ui.model.clone() else {
            return div()
                .flex_1()
                .min_h_0()
                .p(px(28.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(status_words::LOADING_INVENTORY)
                .into_any_element();
        };
        let glance = self.render_glance(&model, t, cx);
        let start_here = self.render_start_here(&model, t, cx);
        // The ordered list is the page and reads down the left; the composition
        // is its sidebar. Too narrow for both and the sidebar goes on top.
        let body = if is_narrow(self.review_ui.content_width) {
            v_flex().gap(px(26.0)).child(glance).child(start_here)
        } else {
            h_flex()
                .items_start()
                .gap(px(36.0))
                .child(start_here.flex_1().min_w_0())
                .child(glance.w(SIDE_WIDTH).flex_shrink_0())
        };
        div()
            .id("review-overview")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .child(
                v_flex()
                    // A wide window does not stretch the page: past this the
                    // rows would only grow their empty middle.
                    .max_w(CONTENT_WIDTH)
                    .px(px(28.0))
                    .py(px(18.0))
                    .child(body),
            )
            .into_any_element()
    }

    /// The sidebar: the headline number, the stacked bar, the legend, the facts.
    fn render_glance(&self, model: &ReviewModel, t: &ThemeColors, cx: &mut Context<Self>) -> Div {
        v_flex()
            .gap(px(14.0))
            .child(stacked_header(
                words::GLANCE_HEADER,
                words::GLANCE_HINT,
                t,
                cx,
            ))
            .child(render_headline(&headline(model), t, cx))
            .child(render_bar(model, t))
            .child(
                v_flex().gap(px(1.0)).max_w(LEGEND_WIDTH).children(
                    legend_rows(model)
                        .into_iter()
                        .enumerate()
                        .map(|(index, row)| self.render_legend_row(index, row, t, cx)),
                ),
            )
            // A rule, so the facts read as facts and not as more legend.
            .children(
                self.render_facts(model, t, cx)
                    .map(|facts| facts.pt(px(14.0)).border_t_1().border_color(rgb(t.border))),
            )
    }

    /// One legend row; clicking it narrows the navigator to that role alone.
    fn render_legend_row(
        &self,
        index: usize,
        row: &VolumeRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let role = row.role;
        // Binary-only comparisons have no lines, and a zero cell says nothing.
        let lines = if row.lines > 0 {
            format_lines(row.lines)
        } else {
            String::new()
        };
        h_flex()
            .id(SharedString::from(format!("review-legend-{index}")))
            .cursor_pointer()
            .items_center()
            .gap(px(8.0))
            .px(px(4.0))
            .py(px(2.0))
            .rounded(px(3.0))
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.review_set_role_filter(single_role(role), cx);
            }))
            .child(
                div()
                    .w(px(8.0))
                    .h(px(8.0))
                    .flex_shrink_0()
                    .rounded(px(2.0))
                    .bg(rgb(role_color(role, t))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(role_label(role)),
            )
            .child(numeric(
                words::legend_files(row.files),
                64.0,
                t.text_muted,
                cx,
            ))
            .child(numeric(lines, 60.0, t.text_secondary, cx))
            .child(numeric(
                words::percent_label(row.percent),
                48.0,
                t.text_muted,
                cx,
            ))
            .into_any_element()
    }

    /// The right column, or nothing at all when this comparison states no facts.
    fn render_facts(
        &self,
        model: &ReviewModel,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let lines = fact_sentences(&model.facts);
        if lines.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .gap(px(10.0))
                .children(
                    lines
                        .into_iter()
                        .enumerate()
                        .map(|(index, line)| self.render_fact_line(index, line, t, cx)),
                )
                .children(self.render_ledger(model, t, cx)),
        )
    }

    /// The commit ledger, inline under the facts while `show ledger` is on.
    fn render_ledger(
        &self,
        model: &ReviewModel,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if !self.review_ui.ledger_open || model.commits.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .pt(px(4.0))
                .gap(px(3.0))
                .border_t_1()
                .border_color(rgb(t.border))
                .children(
                    ledger_rows(&model.commits)
                        .into_iter()
                        .map(|commit| render_ledger_row(commit, t, cx)),
                ),
        )
    }

    fn render_fact_line(
        &self,
        index: usize,
        line: FactLine,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let link = line
            .link
            .map(|link| self.render_fact_link(index, link, t, cx));
        h_flex()
            .items_start()
            .gap(px(12.0))
            .child(
                div()
                    .w(px(76.0))
                    .flex_shrink_0()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(line.label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_wrap()
                    .gap(px(5.0))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(line.text),
                    )
                    .children(link),
            )
            .into_any_element()
    }

    fn render_fact_link(
        &self,
        index: usize,
        link: FactLink,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("review-fact-link-{index}")))
            .cursor_pointer()
            .flex_shrink_0()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.term_blue))
            .hover(|style| style.text_color(rgb(t.text_primary)))
            .child(link.label(self.review_ui.ledger_open))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.review_follow_fact_link(&link, cx);
            }))
            .into_any_element()
    }

    /// Every fact link lands somewhere — spec §2.
    fn review_follow_fact_link(&mut self, link: &FactLink, cx: &mut Context<Self>) {
        match link {
            FactLink::Attention => self.review_set_navigator(NavigatorMode::Attention, cx),
            FactLink::Directory(path) => {
                self.review_open_item(AttentionTarget::Directory(path.clone()), cx);
            }
            FactLink::MechanicalMoves => {
                // Replace the filter rather than layer onto it, as "Also" does.
                self.review_set_navigator(NavigatorMode::Files, cx);
                self.review_set_role_filter(RoleFilter::everything(), cx);
                self.review_set_saved_filter(Some(true), Some(false), cx);
            }
            FactLink::CommitLedger => self.review_toggle_commit_ledger(cx),
            FactLink::Also => {
                let roles = self
                    .review_ui
                    .model
                    .as_ref()
                    .map(|model| also_roles(model))
                    .unwrap_or_else(RoleSet::empty);
                if roles.is_empty() {
                    return;
                }
                self.review_set_navigator(NavigatorMode::Files, cx);
                self.review_set_role_filter(role_set_filter(roles), cx);
            }
        }
    }

    /// The page's own column: the ordered list, two lines to a row.
    fn render_start_here(
        &self,
        model: &ReviewModel,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let all = div()
            .id("review-start-here-all")
            .cursor_pointer()
            .flex_shrink_0()
            .text_size(ui_text_sm(cx))
            .text_color(rgb(t.term_blue))
            .hover(|style| style.text_color(rgb(t.text_primary)))
            .child(words::all_attention(model.attention.len()))
            .on_click(cx.listener(|this, _, _window, cx| {
                this.review_set_navigator(NavigatorMode::Attention, cx);
            }))
            .into_any_element();
        let caveat_line = caveat(&model.coverage).map(|sentence| {
            div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(sentence)
        });
        let items = start_here(model);
        let last = items.len().saturating_sub(1);
        let rows: Vec<AnyElement> = items
            .iter()
            .enumerate()
            .map(|(index, item)| self.render_start_row(index, item, index == last, t, cx))
            .collect();

        v_flex()
            .gap(px(10.0))
            .child(section_header(
                words::START_HERE_HEADER,
                words::START_HERE_HINT,
                Some(all),
                t,
                cx,
            ))
            .children(caveat_line)
            .child(v_flex().children(rows))
            .child(
                div()
                    .pt(px(6.0))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(words::TIERS_FOOTER),
            )
    }

    /// A row is two lines: what changed and by how much, then where it lives
    /// and why it is here.
    fn render_start_row(
        &self,
        index: usize,
        item: &AttentionItem,
        last: bool,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = item.target.clone();
        // Rows structure never reached stay dimmed — spec §8.
        let name_color = if item.dimmed {
            t.text_muted
        } else {
            t.text_primary
        };
        let (added, deleted) = format_signed(item.lines_added, item.lines_deleted);
        let full_path = SharedString::from(item.path.clone());
        let where_text = short_path(&item.path, &item.target);
        h_flex()
            .id(SharedString::from(format!("review-start-here-{index}")))
            .cursor_pointer()
            .items_start()
            .gap(px(10.0))
            .px(px(6.0))
            .py(px(8.0))
            // A hairline, so two-line rows do not run into each other.
            .when(!last, |row| row.border_b_1().border_color(rgb(t.border)))
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.review_open_item(target.clone(), cx);
            }))
            .tooltip(move |window, cx| Tooltip::new(full_path.clone()).build(window, cx))
            .child(
                div()
                    .w(px(16.0))
                    .flex_shrink_0()
                    .text_right()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{}", index.saturating_add(1))),
            )
            .child(
                div()
                    .w(px(12.0))
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(tone_color(row_tone(item), t)))
                    .child(glyph(item.glyph)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.0))
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(name_color))
                                    .child(item.name.clone()),
                            )
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .gap(px(6.0))
                                    .when(item.lines_added > 0, |row| {
                                        row.child(signed_count(added, t.diff_added_fg, cx))
                                    })
                                    .when(item.lines_deleted > 0, |row| {
                                        row.child(signed_count(deleted, t.diff_removed_fg, cx))
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.0))
                            .when(!where_text.is_empty(), |line| {
                                line.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(where_text),
                                )
                            })
                            .children(item.reasons.iter().map(|reason| render_chip(reason, t, cx))),
                    ),
            )
            .into_any_element()
    }
}

/// What the muted column after the name says about where the row lives.
///
/// A symbol row: its file, cut to `…/dir/file.rs`. A file row: its directory,
/// cut the same way — the name is the basename already. A directory row's
/// `path` is its "n implementation files" text and stays whole. Every row's
/// tooltip carries the full path.
fn short_path(path: &str, target: &AttentionTarget) -> String {
    match target {
        AttentionTarget::Symbol { .. } => tail(path, 2),
        AttentionTarget::File(_) => match path.rfind('/') {
            Some(index) => tail(&path[..index], 2),
            None => String::new(),
        },
        AttentionTarget::Directory(_) => path.to_string(),
    }
}

/// The last `keep` segments of a path, `…/` marking what was cut.
fn tail(path: &str, keep: usize) -> String {
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() <= keep {
        return path.to_string();
    }
    format!("\u{2026}/{}", segments[segments.len() - keep..].join("/"))
}

fn render_headline(head: &Headline, t: &ThemeColors, cx: &App) -> AnyElement {
    h_flex()
        .items_baseline()
        .flex_wrap()
        .gap(px(10.0))
        .child(
            div()
                .text_size(ui_text(HEADLINE_SIZE, cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(t.term_bright_blue))
                .child(head.main.clone()),
        )
        .when(!head.sub.is_empty(), |row| {
            row.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(head.sub.clone()),
            )
        })
        .into_any_element()
}

/// One segment per role that changed something; the shares already add up to 100 %.
fn render_bar(model: &ReviewModel, t: &ThemeColors) -> AnyElement {
    let rows = legend_rows(model);
    let total: f32 = rows.iter().map(|row| row.percent).sum();
    let bar = h_flex()
        .h(px(8.0))
        .w_full()
        .rounded(px(2.0))
        .overflow_hidden()
        .bg(rgb(t.bg_secondary));
    if total <= 0.0 {
        return bar.into_any_element();
    }
    bar.children(rows.into_iter().map(|row| {
        div()
            .h_full()
            .w(relative(row.percent / total))
            .bg(rgb(role_color(row.role, t)))
    }))
    .into_any_element()
}

fn section_header(
    title: &'static str,
    hint: &'static str,
    right: Option<AnyElement>,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    h_flex()
        .items_center()
        .gap(px(10.0))
        .child(
            div()
                .flex_shrink_0()
                .text_size(ui_text_sm(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(t.text_secondary))
                .child(title),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(hint),
        )
        .children(right)
}

/// The sidebar has no room for a title and its hint side by side.
fn stacked_header(title: &'static str, hint: &'static str, t: &ThemeColors, cx: &App) -> Div {
    v_flex()
        .gap(px(2.0))
        .child(
            div()
                .text_size(ui_text_sm(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(t.text_secondary))
                .child(title),
        )
        .child(
            div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(hint),
        )
}

/// A right-aligned number column; the digits line up across rows.
fn numeric(text: String, width: f32, color: u32, cx: &App) -> Div {
    div()
        .w(px(width))
        .flex_shrink_0()
        .text_right()
        .font_family("monospace")
        .text_size(ui_text_sm(cx))
        .text_color(rgb(color))
        .child(text)
}

fn signed_count(text: String, color: u32, cx: &App) -> Div {
    div()
        .font_family("monospace")
        .text_size(ui_text_sm(cx))
        .text_color(rgb(color))
        .child(text)
}

/// `a1b2c3d · subject · Ada · 6d ago`; no per-commit diff to open yet.
fn render_ledger_row(commit: &CommitRow, t: &ThemeColors, cx: &App) -> AnyElement {
    h_flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .flex_shrink_0()
                .font_family("monospace")
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(commit.short_sha.clone()),
        )
        .when(commit.is_merge, |row| {
            row.child(
                div()
                    .flex_shrink_0()
                    .px(px(4.0))
                    .rounded(px(3.0))
                    .bg(rgb(t.bg_secondary))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(words::MERGE_BADGE),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_secondary))
                .child(commit.subject.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(commit.author.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(relative_time(commit.timestamp)),
        )
        .into_any_element()
}

fn render_chip(reason: &Reason, t: &ThemeColors, cx: &App) -> AnyElement {
    div()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(rgb(t.bg_secondary))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(tone_color(chip_tone(reason.kind), t)))
        .child(reason.label.clone())
        .into_any_element()
}

fn tone_color(tone: ChipTone, t: &ThemeColors) -> u32 {
    match tone {
        ChipTone::Contract => t.error,
        ChipTone::Behaviour => t.term_blue,
        ChipTone::Addition => t.success,
        ChipTone::Caution => t.warning,
        ChipTone::Muted => t.text_muted,
    }
}

/// Roles keep one colour across the bar and the legend — spec §8.
fn role_color(role: FileRole, t: &ThemeColors) -> u32 {
    match role {
        FileRole::Implementation => t.term_bright_blue,
        FileRole::Test => t.success,
        FileRole::Fixture | FileRole::Snapshot | FileRole::Example => t.text_secondary,
        FileRole::Documentation => t.term_magenta,
        FileRole::Configuration => t.warning,
        FileRole::Lockfile | FileRole::Generated | FileRole::Vendored | FileRole::Unclassified => {
            t.term_bright_black
        }
    }
}

fn single_role(role: FileRole) -> RoleFilter {
    role_set_filter(RoleSet::from_roles([role]))
}

fn role_set_filter(roles: RoleSet) -> RoleFilter {
    RoleFilter {
        roles,
        preset: RolePreset::Custom,
        likely_mechanical_only: false,
        not_analyzed_only: false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::review::ReviewFileKey;
    use super::super::model::AttentionTarget;
    use super::{short_path, tail};

    #[test]
    fn paths_keep_their_tail_and_mark_the_cut() {
        assert_eq!(tail("a/b/c/d.rs", 2), "\u{2026}/c/d.rs");
        assert_eq!(tail("c/d.rs", 2), "c/d.rs");
        assert_eq!(tail("d.rs", 2), "d.rs");
    }

    #[test]
    fn the_where_column_depends_on_what_the_row_is() {
        let key = ReviewFileKey {
            old_path: None,
            new_path: Some("x".into()),
        };
        let symbol = AttentionTarget::Symbol {
            file: key.clone(),
            change_index: 0,
        };
        assert_eq!(
            short_path("packages/worker/src/storage/repo.ts", &symbol),
            "\u{2026}/storage/repo.ts"
        );
        assert_eq!(
            short_path(
                "packages/worker/src/storage/repo.ts",
                &AttentionTarget::File(key)
            ),
            "\u{2026}/src/storage"
        );
        assert_eq!(
            short_path(
                "26 implementation files",
                &AttentionTarget::Directory("a".into())
            ),
            "26 implementation files"
        );
    }
}
