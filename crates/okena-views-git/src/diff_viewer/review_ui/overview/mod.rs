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
use gpui_component::{h_flex, v_flex};
use okena_core::review::FileRole;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::{ui_text, ui_text_ms, ui_text_sm};

/// Right column of the two-column layout; the facts never grow past it.
const FACTS_WIDTH: Pixels = px(360.0);
/// Widest a "Start here" name may get before it is ellipsized.
const NAME_WIDTH: Pixels = px(240.0);
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
        let narrow = is_narrow(self.review_ui.content_width);
        div()
            .id("review-overview")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .px(px(28.0))
                    .py(px(18.0))
                    .gap(px(26.0))
                    .child(self.render_glance(&model, narrow, t, cx))
                    .child(self.render_start_here(&model, t, cx)),
            )
            .into_any_element()
    }

    /// Block 1: the headline number, the stacked bar, the legend, the facts.
    fn render_glance(
        &self,
        model: &ReviewModel,
        narrow: bool,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let volume = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(12.0))
            .child(render_headline(&headline(model), t, cx))
            .child(render_bar(model, t))
            .child(
                v_flex().gap(px(1.0)).children(
                    legend_rows(model)
                        .into_iter()
                        .enumerate()
                        .map(|(index, row)| self.render_legend_row(index, row, t, cx)),
                ),
            );
        // A comparison with no facts gives the volume the whole width.
        let body = match self.render_facts(model, t, cx) {
            None => volume.into_any_element(),
            Some(facts) if narrow => v_flex()
                .gap(px(20.0))
                .child(volume)
                .child(facts)
                .into_any_element(),
            Some(facts) => h_flex()
                .items_start()
                .gap(px(40.0))
                .child(volume)
                .child(facts.w(FACTS_WIDTH).flex_shrink_0())
                .into_any_element(),
        };
        v_flex()
            .gap(px(12.0))
            .child(section_header(
                words::GLANCE_HEADER,
                words::GLANCE_HINT,
                None,
                t,
                cx,
            ))
            .child(body)
            .into_any_element()
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
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(role_label(role)),
            )
            .child(numeric(
                words::legend_files(row.files),
                72.0,
                t.text_muted,
                cx,
            ))
            .child(numeric(lines, 72.0, t.text_secondary, cx))
            .child(numeric(
                words::percent_label(row.percent),
                56.0,
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

    /// Block 2: the ordered list, ten rows of it.
    fn render_start_here(
        &self,
        model: &ReviewModel,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let rows: Vec<AnyElement> = start_here(model)
            .iter()
            .enumerate()
            .map(|(index, item)| self.render_start_row(index, item, t, cx))
            .collect();

        v_flex()
            .gap(px(8.0))
            .child(section_header(
                words::START_HERE_HEADER,
                words::START_HERE_HINT,
                Some(all),
                t,
                cx,
            ))
            .children(caveat_line)
            .child(v_flex().gap(px(1.0)).children(rows))
            .child(
                div()
                    .pt(px(4.0))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(words::TIERS_FOOTER),
            )
            .into_any_element()
    }

    fn render_start_row(
        &self,
        index: usize,
        item: &AttentionItem,
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
        h_flex()
            .id(SharedString::from(format!("review-start-here-{index}")))
            .cursor_pointer()
            .items_center()
            .gap(px(8.0))
            .px(px(4.0))
            .py(px(3.0))
            .rounded(px(3.0))
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.review_open_item(target.clone(), cx);
            }))
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
                div()
                    .flex_shrink_0()
                    // A qualified symbol name may be long; it still may not push
                    // the counts off the row.
                    .max_w(NAME_WIDTH)
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(name_color))
                    .child(item.name.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(item.path.clone()),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .flex_wrap()
                    .gap(px(4.0))
                    .children(item.reasons.iter().map(|reason| render_chip(reason, t, cx))),
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
            )
            .into_any_element()
    }
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
                .overflow_hidden()
                .text_ellipsis()
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
