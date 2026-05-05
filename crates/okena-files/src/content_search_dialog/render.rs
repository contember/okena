//! Top-level `Render` impl: shell layout, keybindings, modal vs. fullscreen.

use super::{Cancel, ContentSearchDialog, ResultRow};
use crate::code_view::extract_selected_text;
use crate::selection::copy_to_clipboard;
use crate::theme::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;
use okena_ui::badge::keyboard_hint;
use okena_ui::empty_state::empty_state;
use okena_ui::modal::{fullscreen_overlay, modal_backdrop, modal_content, modal_header};
use okena_ui::simple_input::SimpleInput;
use okena_ui::tokens::ui_text_sm;

impl Render for ContentSearchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let project_name = self.project_fs.project_name();

        // Focus search input on first render
        let search_input_focus = self.search_input.read(cx).focus_handle(cx);
        if !search_input_focus.is_focused(window) && !self.glob_editing {
            self.search_input.update(cx, |input, cx| input.focus(window, cx));
        }

        // Shared key handler for both modes
        let key_handler = cx.listener(|this, event: &KeyDownEvent, _window, cx| {
            match event.keystroke.key.as_str() {
                "up" => {
                    if this.select_prev() {
                        cx.notify();
                    }
                }
                "down" => {
                    if this.select_next() {
                        cx.notify();
                    }
                }
                "enter" => this.open_selected(cx),
                "tab" if !event.keystroke.modifiers.shift => {
                    this.expanded = !this.expanded;
                    if !this.search_input.read(cx).value().is_empty() {
                        this.trigger_search(cx);
                    }
                    cx.notify();
                }
                "escape" => this.close(cx),
                "c" if event.keystroke.modifiers.platform => {
                    if let Some(file_path) = &this.preview_file {
                        if let Some(lines) = this.highlight_cache.get(file_path) {
                            let text = extract_selected_text(
                                &this.preview_selection,
                                lines.len(),
                                |i| &lines[i].plain_text,
                            );
                            copy_to_clipboard(cx, text);
                        }
                    }
                }
                _ => {}
            }
        });

        let search_row = crate::list_overlay::search_input_row(&self.search_input, &t, cx);

        // Toggles row
        let toggles = self.render_toggles(cx);

        // Glob filter row
        let glob_row = if self.glob_editing {
            Some(
                div()
                    .px(px(12.0))
                    .py(px(4.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Filter:"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(SimpleInput::new(&self.glob_input).text_size(ui_text_sm(cx))),
                    ),
            )
        } else {
            None
        };

        // Results list
        let results_area: AnyElement = if self.rows.is_empty() {
            div()
                .flex_1()
                .child(empty_state(
                    if self.searching {
                        "Searching..."
                    } else if self.search_input.read(cx).value().is_empty() {
                        "Type to search file contents"
                    } else {
                        "No matching results"
                    },
                    &t,
                    cx,
                ))
                .into_any_element()
        } else {
            let rows = self.rows.clone();
            let _has_context = self.expanded;
            let view = cx.entity().clone();

            uniform_list("content-search-list", rows.len(), move |range, _window, cx| {
                view.update(cx, |this, cx| {
                    range
                        .map(|i| {
                            let row = &rows[i];
                            match row {
                                ResultRow::FileHeader {
                                    relative_path,
                                    match_count,
                                    ..
                                } => this
                                    .render_file_header(i, relative_path, *match_count, cx)
                                    .into_any_element(),
                                ResultRow::Match {
                                    file_path,
                                    line_number,
                                    line_content,
                                    match_ranges,
                                    ..
                                } => this.render_match_row(
                                    i, file_path, *line_number, line_content, match_ranges, cx,
                                ),
                            }
                        })
                        .collect()
                })
            })
            .flex_1()
            .track_scroll(&self.scroll_handle)
            .into_any_element()
        };

        // Footer
        let footer = div()
            .px(px(12.0))
            .py(px(8.0))
            .border_t_1()
            .border_color(rgb(t.border))
            .flex()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap(px(16.0))
                    .child(keyboard_hint("Enter", "to open", &t))
                    .child(keyboard_hint(
                        "Tab",
                        if self.expanded { "compact" } else { "expand" },
                        &t,
                    ))
                    .child(keyboard_hint("Esc", "to close", &t)),
            )
            .child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(if self.total_matches > 0 {
                        format!("{} results", self.total_matches)
                    } else {
                        String::new()
                    }),
            );

        // Shared content children
        let header = modal_header(
            &self.config.title,
            Some(format!("Searching in {}", project_name)),
            &t,
            cx,
            cx.listener(|this, _, _window, cx| this.close(cx)),
        );

        if self.expanded {
            // Fullscreen mode: file tree | results | file preview
            let sidebar = self.render_sidebar(cx);
            let preview = self.render_preview_panel(cx);

            fullscreen_overlay("content-search-fullscreen", &t)
                .track_focus(&focus_handle)
                .key_context(self.config.key_context.as_str())
                .on_action(cx.listener(|this, _: &Cancel, _window, cx| this.close(cx)))
                .on_key_down(key_handler)
                .child(header)
                .child(
                    // 3-column layout: sidebar | search+results | preview
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .child(sidebar)
                        .child(
                            div()
                                .w(px(450.0))
                                .flex()
                                .flex_col()
                                .h_full()
                                .min_w_0()
                                .child(search_row)
                                .child(toggles)
                                .children(glob_row)
                                .child(results_area)
                                .child(footer),
                        )
                        .child(preview),
                )
                .when(self.filter_popover_open, |d| {
                    d.child(
                        div()
                            .id("cs-filter-popover-backdrop")
                            .absolute()
                            .inset_0()
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.filter_popover_open = false;
                                cx.notify();
                            }))
                    )
                })
                .when_some(
                    self.filter_popover_open
                        .then_some(self.filter_button_bounds)
                        .flatten(),
                    |d, bounds| {
                        let entity = cx.entity().downgrade();
                        d.child(crate::list_overlay::file_filter_popover(
                            bounds, self.show_ignored, &t, cx,
                            move |filter, _, cx| {
                                if let Some(e) = entity.upgrade() {
                                    e.update(cx, |this, cx| {
                                        if filter == "ignored" {
                                            this.show_ignored = !this.show_ignored;
                                        }
                                        this.trigger_search(cx);
                                        cx.notify();
                                    });
                                }
                            },
                        ))
                    },
                )
                .into_any_element()
        } else {
            // Compact modal mode
            modal_backdrop("content-search-backdrop", &t)
                .track_focus(&focus_handle)
                .key_context(self.config.key_context.as_str())
                .items_start()
                .pt(px(80.0))
                .on_action(cx.listener(|this, _: &Cancel, _window, cx| this.close(cx)))
                .on_key_down(key_handler)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.close(cx)),
                )
                .child(
                    modal_content("content-search-modal", &t)
                        .relative()
                        .w(px(self.config.width))
                        .h(px(self.config.max_height))
                        .child(header)
                        .child(search_row)
                        .child(toggles)
                        .children(glob_row)
                        .child(results_area)
                        .child(footer)
                        .when(self.filter_popover_open, |modal| {
                            modal.child(
                                div()
                                    .id("cs-filter-popover-backdrop-compact")
                                    .absolute()
                                    .inset_0()
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.filter_popover_open = false;
                                        cx.notify();
                                    }))
                            )
                        })
                        .when_some(
                            self.filter_popover_open
                                .then_some(self.filter_button_bounds)
                                .flatten(),
                            |modal, bounds| {
                            let entity = cx.entity().downgrade();
                            modal.child(crate::list_overlay::file_filter_popover(
                                bounds, self.show_ignored, &t, cx,
                                move |filter, _, cx| {
                                    if let Some(e) = entity.upgrade() {
                                        e.update(cx, |this, cx| {
                                            if filter == "ignored" {
                                                this.show_ignored = !this.show_ignored;
                                            }
                                            this.trigger_search(cx);
                                            cx.notify();
                                        });
                                    }
                                },
                            ))
                        }),
                )
                .into_any_element()
        }
    }
}
