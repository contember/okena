use crate::settings::settings_entity;
use crate::theme::theme;
use crate::workspace::settings::CursorShape;
use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::h_flex;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_terminal(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        div()
            .child(section_header("Terminal", &t))
            .child(
                section_container(&t)
                    .child(self.render_shell_dropdown_row(&s.default_shell, cx))
                    .child(self.render_session_backend_dropdown_row(&s.session_backend, cx))
                    .child(self.render_toggle(
                        "show-shell-selector", "Show Shell Selector", s.show_shell_selector, true,
                        |state, val, cx| state.set_show_shell_selector(val, cx), cx,
                    ))
                    .child(self.render_cursor_style_row(s.cursor_style, cx))
                    .child(self.render_toggle(
                        "cursor-blink", "Cursor Blink", s.cursor_blink, true,
                        |state, val, cx| state.set_cursor_blink(val, cx), cx,
                    ))
                    .child(self.render_integer_stepper(
                        "scrollback", "Scrollback Lines", s.scrollback_lines, 1000, 70.0, false,
                        |state, val, cx| state.set_scrollback_lines(val, cx), cx,
                    )),
            )
    }

    fn render_cursor_style_row(&self, current: CursorShape, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        settings_row("cursor-style".to_string(), "Cursor Style", &t, true).child(
            h_flex()
                .gap(px(2.0))
                .rounded(px(4.0))
                .bg(rgb(t.bg_secondary))
                .p(px(2.0))
                .children(CursorShape::all_variants().iter().map(|&style: &CursorShape| {
                    let is_selected = style == current;
                    let hover_bg = t.bg_hover;
                    div()
                        .id(ElementId::Name(format!("cursor-style-{:?}", style).into()))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(3.0))
                        .text_size(px(12.0))
                        .when(is_selected, |el: Stateful<Div>| {
                            el.bg(rgb(t.border_active))
                                .text_color(rgb(t.text_primary))
                        })
                        .when(!is_selected, |el: Stateful<Div>| {
                            el.text_color(rgb(t.text_muted))
                                .hover(|s: StyleRefinement| s.bg(rgb(hover_bg)))
                        })
                        .child(style.display_name().to_string())
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            settings_entity(cx).update(cx, |state, cx| {
                                state.set_cursor_style(style, cx);
                            });
                        }))
                })),
        )
    }
}
