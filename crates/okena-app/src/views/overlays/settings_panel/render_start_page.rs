use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::tokens::ui_text_md;
use crate::views::components::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use crate::workspace::settings::StartPageButton;
use gpui::prelude::*;
use gpui::*;
use okena_core::shell::ShellType;
use okena_ui::button::{button, button_primary};
use okena_ui::dropdown::{dropdown_button, dropdown_option, dropdown_overlay};

use super::SettingsPanel;
use super::components::{input_box, section_container, section_header, settings_input_row};

const PROJECT_DEFAULT: &str = "Project default";

pub(super) struct StartPageButtonDraft {
    original: Option<(usize, StartPageButton)>,
    name: Entity<SimpleInputState>,
    command: Entity<SimpleInputState>,
    shell: Option<ShellType>,
}

impl SettingsPanel {
    fn edit_start_page_button(
        &mut self,
        original: Option<(usize, StartPageButton)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = original.as_ref().map(|(_, button)| button);
        let name = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Button name")
                .default_value(value.map(|button| button.name.clone()).unwrap_or_default())
        });
        let command = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Command")
                .default_value(
                    value
                        .map(|button| button.command.clone())
                        .unwrap_or_default(),
                )
        });
        for input in [&name, &command] {
            cx.subscribe(input, |_, _, _: &InputChangedEvent, cx| cx.notify())
                .detach();
        }
        let shell = value.and_then(|button| button.shell.clone());
        name.read(cx).focus_handle(cx).focus(window, cx);
        self.start_page_button_draft = Some(StartPageButtonDraft {
            original,
            name,
            command,
            shell,
        });
        self.start_page_shell_dropdown_open = false;
        cx.notify();
    }

    fn save_start_page_button(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.start_page_button_draft.as_ref() else {
            return;
        };
        let name = draft.name.read(cx).value().trim().to_string();
        let command = draft.command.read(cx).value().to_string();
        if name.is_empty() || command.trim().is_empty() {
            return;
        }

        let mut buttons = settings_entity(cx)
            .read(cx)
            .settings
            .start_page_buttons
            .clone();
        let button = StartPageButton {
            name,
            command,
            shell: draft.shell.clone(),
        };
        if let Some((index, original)) = &draft.original {
            if let Some(current) = buttons
                .get_mut(*index)
                .filter(|current| *current == original)
            {
                *current = button;
            } else {
                self.finish_start_page_button_edit(cx);
                return;
            }
        } else {
            buttons.push(button);
        }

        settings_entity(cx).update(cx, |state, cx| state.set_start_page_buttons(buttons, cx));
        self.finish_start_page_button_edit(cx);
    }

    fn finish_start_page_button_edit(&mut self, cx: &mut Context<Self>) {
        self.start_page_button_draft = None;
        self.start_page_shell_dropdown_open = false;
        cx.notify();
    }

    fn render_start_page_button_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let draft = self
            .start_page_button_draft
            .as_ref()
            .expect("active button draft");
        let can_save = !draft.name.read(cx).value().trim().is_empty()
            && !draft.command.read(cx).value().trim().is_empty();
        let shell_label = draft
            .shell
            .as_ref()
            .map_or_else(|| PROJECT_DEFAULT.to_string(), ShellType::display_name);
        let bounds_setter = Self::bounds_setter(cx, |s, b| s.start_page_shell_button_bounds = b);

        div()
            .child(
                settings_input_row("start-page-button-name", "Name", "", &t, cx, false).child(
                    input_box(&t).child(SimpleInput::new(&draft.name).text_size(ui_text_md(cx))),
                ),
            )
            .child(
                settings_input_row("start-page-button-command", "Command", "", &t, cx, false)
                    .child(
                        input_box(&t)
                            .child(SimpleInput::new(&draft.command).text_size(ui_text_md(cx))),
                    ),
            )
            .child(
                super::components::settings_row("start-page-button-shell", "Shell", &t, cx, false)
                    .child(
                        dropdown_button(
                            "start-page-button-shell-picker",
                            &shell_label,
                            self.start_page_shell_dropdown_open,
                            &t,
                            cx,
                            bounds_setter,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                let opening = !this.start_page_shell_dropdown_open;
                                this.close_all_dropdowns();
                                this.start_page_shell_dropdown_open = opening;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .pb(px(10.0))
                    .child(
                        button_primary("start-page-button-save", "Save button", &t)
                            .when(!can_save, |button| button.opacity(0.45))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.save_start_page_button(cx)),
                            ),
                    )
                    .child(button("start-page-button-cancel", "Cancel", &t).on_click(
                        cx.listener(|this, _, _, cx| this.finish_start_page_button_edit(cx)),
                    )),
            )
    }

    pub(super) fn render_start_page_shell_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let selected = self
            .start_page_button_draft
            .as_ref()
            .and_then(|draft| draft.shell.as_ref());

        let choices = std::iter::once((None, PROJECT_DEFAULT.to_string())).chain(
            // "Project default" covers the default shell.
            self.available_shells
                .iter()
                .filter(|shell| shell.available && shell.shell_type != ShellType::Default)
                .map(|shell| (Some(shell.shell_type.clone()), shell.name.clone())),
        );
        dropdown_overlay("start-page-shell-list", &t)
            .min_w(px(180.0))
            .max_h(px(250.0))
            .children(choices.enumerate().map(|(index, (shell, label))| {
                let is_selected = shell.as_ref() == selected;
                dropdown_option(
                    format!("start-page-shell-{index}"),
                    &label,
                    is_selected,
                    &t,
                    cx,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = &mut this.start_page_button_draft {
                            draft.shell = shell.clone();
                        }
                        this.start_page_shell_dropdown_open = false;
                        cx.notify();
                    }),
                )
            }))
    }

    pub(super) fn render_start_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let (show, buttons) = {
            let settings = &settings_entity(cx).read(cx).settings;
            (
                settings.show_start_page,
                settings.start_page_buttons.clone(),
            )
        };
        let editing = self.start_page_button_draft.is_some();
        let count = buttons.len();
        let draft_index = self
            .start_page_button_draft
            .as_ref()
            .and_then(|draft| draft.original.as_ref())
            .map(|(index, _)| *index)
            .filter(|&index| index < count);

        div()
            .child(section_header("Start Page", &t, cx))
            .child(section_container(&t).child(Self::render_toggle_with_desc(
                "show-start-page",
                "Show Start Page for new terminals",
                "Show shortcuts and command buttons before starting a new interactive terminal.",
                show,
                false,
                |state, show, cx| state.set_show_start_page(show, cx),
                cx,
            )))
            .child(section_header("Command Buttons", &t, cx))
            .child(
                section_container(&t)
                    .children(buttons.into_iter().enumerate().map(|(index, item)| {
                        if draft_index == Some(index) {
                            return self.render_start_page_button_editor(cx).into_any_element();
                        }
                        let shell = item
                            .shell
                            .as_ref()
                            .map_or_else(|| PROJECT_DEFAULT.to_string(), ShellType::display_name);
                        let row = settings_input_row(
                            format!("start-page-command-{index}"),
                            &item.name,
                            &shell,
                            &t,
                            cx,
                            index + 1 < count || editing,
                        )
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.text_secondary))
                                .child(item.command.clone()),
                        );
                        row.when(!editing, |row| {
                            row.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(6.0))
                                    .child(
                                        button(format!("start-page-edit-{index}"), "Edit", &t)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.edit_start_page_button(
                                                    Some((index, item.clone())),
                                                    window,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        button(format!("start-page-up-{index}"), "Move up", &t)
                                            .when(index == 0, |button| button.opacity(0.45))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.move_start_page_button(
                                                    index,
                                                    index.checked_sub(1),
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        button(format!("start-page-down-{index}"), "Move down", &t)
                                            .when(index + 1 == count, |button| button.opacity(0.45))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.move_start_page_button(
                                                    index,
                                                    Some(index + 1),
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        button(format!("start-page-delete-{index}"), "Delete", &t)
                                            .on_click(cx.listener(move |_this, _, _, cx| {
                                                let mut buttons = settings_entity(cx)
                                                    .read(cx)
                                                    .settings
                                                    .start_page_buttons
                                                    .clone();
                                                if index < buttons.len() {
                                                    buttons.remove(index);
                                                    settings_entity(cx).update(cx, |state, cx| {
                                                        state.set_start_page_buttons(buttons, cx)
                                                    });
                                                    cx.notify();
                                                }
                                            })),
                                    ),
                            )
                        })
                        .into_any_element()
                    }))
                    .when(editing && draft_index.is_none(), |list| {
                        list.child(self.render_start_page_button_editor(cx))
                    })
                    .when(!editing, |list| {
                        list.child(div().p(px(10.0)).child(
                            button("start-page-add-button", "+ Add button", &t).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.edit_start_page_button(None, window, cx)
                                }),
                            ),
                        ))
                    }),
            )
    }

    fn move_start_page_button(
        &mut self,
        index: usize,
        target: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let mut buttons = settings_entity(cx)
            .read(cx)
            .settings
            .start_page_buttons
            .clone();
        if let Some(target) =
            target.filter(|&target| index < buttons.len() && target < buttons.len())
        {
            buttons.swap(index, target);
            settings_entity(cx).update(cx, |state, cx| state.set_start_page_buttons(buttons, cx));
            cx.notify();
        }
    }
}
