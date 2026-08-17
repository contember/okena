//! Roles menu: presets, the 11 roles, and the saved filters — spec §7.

use super::super::super::DiffViewer;
use super::super::labels::nav as words;
use super::roles::{self, PresetRow, RoleRow, SavedFilter, SavedRow};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_core::theme::ThemeColors;
use okena_ui::popover::popover_panel;
use okena_ui::tokens::{RADIUS_MD, ui_text_ms, ui_text_sm};

const PANEL_WIDTH: Pixels = px(260.0);
/// Clears the segmented control, the filter box and the Roles button above it.
const PANEL_TOP: Pixels = px(96.0);
const CHECKBOX: Pixels = px(12.0);

impl DiffViewer {
    pub(crate) fn render_roles_menu(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.review_ui.roles_menu_open {
            return None;
        }
        let model = self.review_ui.model.as_ref()?;
        let menu = roles::roles_menu(model, &self.review_ui.role_filter);

        let panel = popover_panel("review-roles-menu", t)
            .absolute()
            .left(px(8.0))
            .top(PANEL_TOP)
            .w(PANEL_WIDTH)
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section_title(words::PRESETS_TITLE, t, cx))
            .children(
                menu.presets
                    .iter()
                    .map(|preset| self.render_preset_row(preset, t, cx)),
            )
            .child(section_title(words::ROLES_TITLE, t, cx))
            .children(
                menu.roles
                    .iter()
                    .map(|role| self.render_role_row(role, t, cx)),
            )
            .child(section_title(words::ALSO_TITLE, t, cx))
            .children(
                menu.saved
                    .iter()
                    .map(|saved| self.render_saved_row(saved, t, cx)),
            );

        // Occludes, so the dismissing click never also lands on a row underneath.
        Some(
            div()
                .id("review-roles-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        this.review_toggle_roles_menu(cx);
                    }),
                )
                .child(panel)
                .into_any_element(),
        )
    }

    fn render_preset_row(
        &self,
        preset: &PresetRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = preset.preset;
        h_flex()
            .id(ElementId::Name(
                format!("review-preset-{}", preset.label).into(),
            ))
            .cursor_pointer()
            .py(px(3.0))
            .px(px(4.0))
            .gap(px(8.0))
            .items_baseline()
            .rounded(RADIUS_MD)
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(if preset.active {
                        t.text_primary
                    } else {
                        t.term_blue
                    }))
                    .child(preset.label),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(preset.hint.clone()),
            )
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.review_apply_preset(target, cx);
            }))
            .into_any_element()
    }

    fn render_role_row(
        &self,
        role: &RoleRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = role.role;
        menu_row(
            ElementId::Name(format!("review-role-{}", role.label).into()),
            role.checked,
            role.label,
            &role.count.to_string(),
            t,
            cx,
        )
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.review_toggle_role(target, cx);
        }))
        .into_any_element()
    }

    fn render_saved_row(
        &self,
        saved: &SavedRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter = saved.filter;
        let next = !saved.checked;
        menu_row(
            ElementId::Name(format!("review-saved-{}", saved.label).into()),
            saved.checked,
            saved.label,
            &saved.note,
            t,
            cx,
        )
        .on_click(cx.listener(move |this, _, _window, cx| match filter {
            SavedFilter::LikelyMechanical => this.review_set_saved_filter(Some(next), None, cx),
            SavedFilter::NotAnalyzed => this.review_set_saved_filter(None, Some(next), cx),
        }))
        .into_any_element()
    }
}

fn section_title(title: &'static str, t: &ThemeColors, cx: &App) -> Div {
    div()
        .pt(px(8.0))
        .pb(px(2.0))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_muted))
        .child(title)
}

/// A checkbox row: the box, the name, and the number it stands for.
fn menu_row(
    id: ElementId,
    checked: bool,
    label: &'static str,
    trailing: &str,
    t: &ThemeColors,
    cx: &App,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .cursor_pointer()
        .py(px(3.0))
        .px(px(4.0))
        .gap(px(8.0))
        .items_center()
        .rounded(RADIUS_MD)
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(
            div()
                .size(CHECKBOX)
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(2.0))
                .border_1()
                .border_color(rgb(if checked { t.border_active } else { t.border }))
                .when(checked, |d| {
                    d.bg(rgb(t.border_active)).child(
                        svg()
                            .path("icons/check.svg")
                            .size(px(10.0))
                            .text_color(rgb(t.selection_fg)),
                    )
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_primary))
                .child(label),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(trailing.to_string()),
        )
}
