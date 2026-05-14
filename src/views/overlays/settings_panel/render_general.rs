use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_sm, ui_text_md};
use crate::views::components::simple_input::SimpleInput;
use crate::workspace::settings::HeaderDensity;
use gpui::*;
use gpui::prelude::*;
use gpui_component::{h_flex, v_flex};

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_general(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        let section = section_container(&t)
            .child(self.render_toggle(
                "focus-border", "Show Focus Border", s.show_focused_border, true,
                |state, val, cx| state.set_show_focused_border(val, cx), cx,
            ))
            .child(self.render_toggle(
                "color-tinted-bg", "Color Tinted Background", s.color_tinted_background, true,
                |state, val, cx| state.set_color_tinted_background(val, cx), cx,
            ))
            .child(self.render_header_density_row(s.header_density, cx))
            .child(self.render_toggle(
                "detached-by-default", "Detached Overlays by Default", s.detached_overlays_by_default, true,
                |state, val, cx| state.set_detached_overlays_by_default(val, cx), cx,
            ))
            .child(self.render_toggle(
                "remote-server", "Remote Server", s.remote_server_enabled, true,
                |state, val, cx| state.set_remote_server_enabled(val, cx), cx,
            ))
            .when(s.remote_server_enabled, |d| {
                d.child(
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            v_flex()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(ui_text(13.0, cx))
                                        .text_color(rgb(t.text_primary))
                                        .child("Listen Address"),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child("IP address to bind the remote server (e.g. 0.0.0.0 for all interfaces)"),
                                ),
                        )
                        .child(
                            div()
                                .bg(rgb(t.bg_secondary))
                                .border_1()
                                .border_color(rgb(t.border))
                                .rounded(px(4.0))
                                .child(SimpleInput::new(&self.listen_address_input).text_size(ui_text_md(cx))),
                        ),
                )
            })
            .child(self.render_number_stepper(
                "min-col-width", "Min Column Width", s.min_column_width,
                "{}px", 50.0, 60.0, false,
                |state, val, cx| state.set_min_column_width(val, cx), cx,
            ));

        div()
            .child(section_header("Appearance", &t, cx))
            .child(section)
            .child(section_header("File Opener", &t, cx))
            .child(
                section_container(&t)
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                v_flex()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(ui_text(13.0, cx))
                                            .text_color(rgb(t.text_primary))
                                            .child("Editor Command"),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_sm(cx))
                                            .text_color(rgb(t.text_muted))
                                            .child("Command to open file paths (empty = system default)"),
                                    ),
                            )
                            .child(
                                div()
                                    .bg(rgb(t.bg_secondary))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .rounded(px(4.0))
                                    .child(SimpleInput::new(&self.file_opener_input).text_size(ui_text_md(cx))),
                            ),
                    ),
            )
    }

    fn render_header_density_row(
        &self,
        current: HeaderDensity,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);

        settings_row(
            "header-density".to_string(),
            "Project Header Density",
            &t,
            cx,
            true,
        )
        .child(
            h_flex()
                .gap(px(2.0))
                .rounded(px(4.0))
                .bg(rgb(t.bg_secondary))
                .p(px(2.0))
                .children(HeaderDensity::all_variants().iter().map(|&density: &HeaderDensity| {
                    let is_selected = density == current;
                    let hover_bg = t.bg_hover;
                    div()
                        .id(ElementId::Name(
                            format!("header-density-{:?}", density).into(),
                        ))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(3.0))
                        .text_size(ui_text_md(cx))
                        .when(is_selected, |el: Stateful<Div>| {
                            el.bg(rgb(t.border_active))
                                .text_color(rgb(t.text_primary))
                        })
                        .when(!is_selected, |el: Stateful<Div>| {
                            el.text_color(rgb(t.text_muted))
                                .hover(|s: StyleRefinement| s.bg(rgb(hover_bg)))
                        })
                        .child(density.display_name().to_string())
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            settings_entity(cx).update(cx, |state, cx| {
                                state.set_header_density(density, cx);
                            });
                        }))
                })),
        )
    }
}
