//! Top-toolbar toggles (case/regex/fuzzy, file filter, glob input trigger).

use super::{ContentSearchDialog, ResultRow};
use crate::theme::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use okena_ui::tokens::ui_text_sm;

impl ContentSearchDialog {
    pub(super) fn render_toggles(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);

        let glob_value = self.glob_input.read(cx).value().to_string();
        let has_glob = !glob_value.is_empty();

        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(self.render_toggle_button("Aa", self.case_sensitive, "Case Sensitive", "case", cx))
            .child(self.render_toggle_button(".*", self.regex_mode, "Regular Expression", "regex", cx))
            .child(self.render_toggle_button("~", self.fuzzy_mode, "Fuzzy Match", "fuzzy", cx))
            .child(self.render_file_filter_button(cx))
            // Glob filter input
            .child(
                div()
                    .id("glob-filter")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .text_size(ui_text_sm(cx))
                    .bg(rgb(if has_glob { t.border_active } else { t.bg_secondary }))
                    .text_color(rgb(if has_glob { t.text_primary } else { t.text_muted }))
                    .child(if has_glob {
                        format!("filter: {}", glob_value)
                    } else {
                        "filter".to_string()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.glob_editing = !this.glob_editing;
                            if this.glob_editing {
                                this.glob_input.update(cx, |input, cx| input.focus(window, cx));
                            } else {
                                this.search_input.update(cx, |input, cx| input.focus(window, cx));
                            }
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(if self.searching {
                                "Searching...".to_string()
                            } else if self.total_matches > 0 {
                                format!(
                                    "{} match{} in {} file{}",
                                    self.total_matches,
                                    if self.total_matches == 1 { "" } else { "es" },
                                    self.rows.iter().filter(|r| matches!(r, ResultRow::FileHeader { .. })).count(),
                                    if self.rows.iter().filter(|r| matches!(r, ResultRow::FileHeader { .. })).count() == 1 { "" } else { "s" },
                                )
                            } else if !self.search_input.read(cx).value().is_empty() {
                                "No results".to_string()
                            } else {
                                String::new()
                            }),
                    ),
            )
    }

    /// Render a single toggle button with tooltip.
    fn render_toggle_button(
        &self,
        label: &str,
        active: bool,
        tooltip: &str,
        id: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let id_owned = id.to_string();
        let tooltip_text: SharedString = tooltip.to_string().into();

        div()
            .id(ElementId::Name(format!("toggle-{}", id).into()))
            .cursor_pointer()
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .text_size(ui_text_sm(cx))
            .font_weight(FontWeight::MEDIUM)
            .tooltip(move |_window, cx| Tooltip::new(tooltip_text.clone()).build(_window, cx))
            .when(active, |d: Stateful<Div>| {
                d.bg(rgb(t.border_active))
                    .text_color(rgb(t.text_primary))
            })
            .when(!active, |d: Stateful<Div>| {
                d.bg(rgb(t.bg_secondary))
                    .text_color(rgb(t.text_muted))
            })
            .hover(|s: StyleRefinement| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    match id_owned.as_str() {
                        "case" => this.case_sensitive = !this.case_sensitive,
                        "regex" => {
                            this.regex_mode = !this.regex_mode;
                            if this.regex_mode { this.fuzzy_mode = false; }
                        }
                        "fuzzy" => {
                            this.fuzzy_mode = !this.fuzzy_mode;
                            if this.fuzzy_mode { this.regex_mode = false; }
                        }
                        _ => {}
                    }
                    this.trigger_search(cx);
                    cx.notify();
                }),
            )
            .child(label.to_string())
    }

    fn render_file_filter_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme(cx);
        let active_count = self.show_ignored as u8;

        let entity = cx.entity().downgrade();
        let entity2 = entity.clone();

        crate::list_overlay::file_filter_button(
            "cs-filter-btn", active_count, &t, cx,
            move |_, _, cx| {
                if let Some(e) = entity.upgrade() {
                    e.update(cx, |this, cx| {
                        this.filter_popover_open = !this.filter_popover_open;
                        cx.notify();
                    });
                }
            },
            move |bounds, _, cx| {
                if let Some(e) = entity2.upgrade() {
                    e.update(cx, |this, _| this.filter_button_bounds = Some(bounds));
                }
            },
        )
    }
}
