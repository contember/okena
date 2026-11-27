use crate::keybindings::{
    format_keystroke, keystroke_to_config_string, start_page_shortcut_error,
    validate_start_page_shortcut,
};
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
const SHORTCUT_DAEMON_UNAVAILABLE: &str = "Connect to a local daemon that supports command shortcuts. Update and restart the daemon if needed.";

pub(super) struct StartPageButtonDraft {
    original: Option<(usize, StartPageButton)>,
    name: Entity<SimpleInputState>,
    command: Entity<SimpleInputState>,
    shell: Option<ShellType>,
    shortcut: Option<String>,
    shortcut_error: Option<String>,
    shortcut_focus: FocusHandle,
    shortcut_recording: Option<StartPageShortcutRecording>,
}

impl StartPageButtonDraft {
    fn matches_saved_button(&self, buttons: &[StartPageButton]) -> bool {
        self.original
            .as_ref()
            .is_none_or(|(index, original)| buttons.get(*index) == Some(original))
    }

    fn accept_recorded_shortcut(
        &mut self,
        candidate: &str,
        buttons: &[StartPageButton],
        cx: &App,
    ) -> Result<(), String> {
        if !self.matches_saved_button(buttons) {
            return Err(
                "This button changed in another window. Close and reopen the editor.".into(),
            );
        }
        let editing_index = self.original.as_ref().map(|(index, _)| *index);
        self.shortcut = Some(validate_start_page_shortcut(
            candidate,
            buttons,
            editing_index,
            cx,
        )?);
        self.shortcut_error = None;
        Ok(())
    }

    fn shortcut_changed(&self) -> bool {
        self.shortcut.as_ref()
            != self
                .original
                .as_ref()
                .and_then(|(_, button)| button.shortcut.as_ref())
    }
}

struct StartPageShortcutRecording {
    _keystrokes: Subscription,
    _blur: Subscription,
    error: Option<String>,
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
        let shortcut = value.and_then(|button| button.shortcut.clone());
        let shortcut_error = original.as_ref().and_then(|(index, _)| {
            let buttons = &settings_entity(cx).read(cx).settings.start_page_buttons;
            start_page_shortcut_error(*index, buttons, cx)
        });
        name.read(cx).focus_handle(cx).focus(window, cx);
        self.start_page_button_draft = Some(StartPageButtonDraft {
            original,
            name,
            command,
            shell,
            shortcut,
            shortcut_error,
            shortcut_focus: cx.focus_handle(),
            shortcut_recording: None,
        });
        self.start_page_shell_dropdown_open = false;
        cx.notify();
    }

    fn record_start_page_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.supports_start_page_shortcuts(cx) {
            return;
        }
        self.close_all_dropdowns();
        let Some(draft) = self.start_page_button_draft.as_mut() else {
            return;
        };
        if draft.shortcut_recording.is_some() {
            self.stop_start_page_shortcut_recording(cx);
            return;
        }

        let focus = draft.shortcut_focus.clone();
        focus.focus(window, cx);
        let blur = cx.on_blur(&focus, window, |this, _, cx| {
            this.stop_start_page_shortcut_recording(cx);
        });
        let entity = cx.entity().downgrade();
        let target_window = window.window_handle();
        let keystrokes = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != target_window || !focus.is_focused(window) {
                return;
            }
            let Some(entity) = entity.upgrade() else {
                return;
            };
            entity.update(cx, |this, cx| {
                let Some(draft) = this.start_page_button_draft.as_mut() else {
                    return;
                };
                let keystroke = &event.keystroke;
                cx.stop_propagation();
                match keystroke.key.as_str() {
                    "shift" | "control" | "alt" | "platform" | "function" | "" => return,
                    "escape" if !keystroke.modifiers.modified() => {}
                    _ => {
                        let candidate =
                            keystroke_to_config_string(keystroke, cx.platform_info().os);
                        let settings = settings_entity(cx);
                        let buttons = &settings.read(cx).settings.start_page_buttons;
                        if let Err(error) = draft.accept_recorded_shortcut(&candidate, buttons, cx)
                        {
                            if let Some(recording) = &mut draft.shortcut_recording {
                                recording.error = Some(error);
                            }
                            cx.notify();
                            return;
                        }
                    }
                }
                this.stop_start_page_shortcut_recording(cx);
            });
        });
        draft.shortcut_recording = Some(StartPageShortcutRecording {
            _keystrokes: keystrokes,
            _blur: blur,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn stop_start_page_shortcut_recording(&mut self, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.start_page_button_draft
            && draft.shortcut_recording.take().is_some()
        {
            cx.notify();
        }
    }

    fn save_start_page_button(&mut self, cx: &mut Context<Self>) {
        self.stop_start_page_shortcut_recording(cx);
        let supports_shortcuts = self.supports_start_page_shortcuts(cx);
        let Some(draft) = self.start_page_button_draft.as_mut() else {
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
        let editing_index = draft.original.as_ref().map(|(index, _)| *index);
        if !draft.matches_saved_button(&buttons) {
            self.finish_start_page_button_edit(cx);
            return;
        }

        let shortcut = if draft.shortcut_changed() {
            if !supports_shortcuts {
                return;
            }
            match draft
                .shortcut
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(value) => {
                    match validate_start_page_shortcut(value, &buttons, editing_index, cx) {
                        Ok(normalized) => Some(normalized),
                        Err(error) => {
                            draft.shortcut_error = Some(error);
                            cx.notify();
                            return;
                        }
                    }
                }
                None => None,
            }
        } else {
            draft.shortcut.clone()
        };
        let button = StartPageButton {
            name,
            command,
            shell: draft.shell.clone(),
            shortcut,
        };
        if let Some(index) = editing_index {
            buttons[index] = button;
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
        let supports_shortcuts = self.supports_start_page_shortcuts(cx);
        let can_save = !draft.name.read(cx).value().trim().is_empty()
            && !draft.command.read(cx).value().trim().is_empty()
            && (supports_shortcuts || !draft.shortcut_changed());
        let shell_label = draft
            .shell
            .as_ref()
            .map_or_else(|| PROJECT_DEFAULT.to_string(), ShellType::display_name);
        let bounds_setter = Self::bounds_setter(cx, |s, b| s.start_page_shell_button_bounds = b);
        let recording = draft.shortcut_recording.is_some();
        let shortcut_error = draft
            .shortcut_recording
            .as_ref()
            .and_then(|recording| recording.error.clone())
            .or_else(|| draft.shortcut_error.clone());
        let shortcut_label = match draft.shortcut.as_deref() {
            Some(shortcut) => format_keystroke(shortcut),
            None => "None".to_string(),
        };

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
                                this.stop_start_page_shortcut_recording(cx);
                                this.close_all_dropdowns();
                                this.start_page_shell_dropdown_open = opening;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                settings_input_row(
                    "start-page-button-shortcut",
                    "Shortcut (optional)",
                    if supports_shortcuts {
                        "For example, Ctrl+Shift+X or F8."
                    } else {
                        SHORTCUT_DAEMON_UNAVAILABLE
                    },
                    &t,
                    cx,
                    false,
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(shortcut_label)
                        .when(supports_shortcuts, |row| {
                            row.child(
                                button(
                                    "start-page-shortcut-record",
                                    if recording {
                                        "Press a key (Esc cancels)"
                                    } else if draft.shortcut.is_some() {
                                        "Change shortcut"
                                    } else {
                                        "Record shortcut"
                                    },
                                    &t,
                                )
                                .track_focus(&draft.shortcut_focus)
                                .tab_stop(true)
                                .focus_visible(|style| style.underline())
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.record_start_page_shortcut(window, cx);
                                    },
                                )),
                            )
                        })
                        .when(supports_shortcuts && draft.shortcut.is_some(), |row| {
                            row.child(
                                button("start-page-shortcut-clear", "Clear", &t)
                                    .focusable()
                                    .tab_stop(true)
                                    .focus_visible(|style| style.underline())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.supports_start_page_shortcuts(cx) {
                                            return;
                                        }
                                        this.stop_start_page_shortcut_recording(cx);
                                        if let Some(draft) = &mut this.start_page_button_draft {
                                            draft.shortcut = None;
                                            draft.shortcut_error = None;
                                            cx.notify();
                                        }
                                    })),
                            )
                        }),
                )
                .when_some(shortcut_error, |row, error| {
                    row.child(
                        div()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.error))
                            .child(error),
                    )
                }),
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
                    .children(buttons.iter().cloned().enumerate().map(|(index, item)| {
                        if draft_index == Some(index) {
                            return self.render_start_page_button_editor(cx).into_any_element();
                        }
                        let shell = item
                            .shell
                            .as_ref()
                            .map_or_else(|| PROJECT_DEFAULT.to_string(), ShellType::display_name);
                        let details = match item.shortcut.as_deref() {
                            Some(shortcut) => format!("{shell} · {}", format_keystroke(shortcut)),
                            None => shell,
                        };
                        let shortcut_error = start_page_shortcut_error(index, &buttons, cx);
                        let row = settings_input_row(
                            format!("start-page-command-{index}"),
                            &item.name,
                            &details,
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
                        )
                        .when_some(shortcut_error, |row, error| {
                            row.child(
                                div()
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.error))
                                    .child(error),
                            )
                        });
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

#[cfg(test)]
mod shortcut_recording_tests {
    use super::{SimpleInputState, StartPageButton, StartPageButtonDraft};
    use gpui::{AppContext, KeyBinding, TestAppContext};

    #[gpui::test]
    fn recorder_keeps_assignment_after_duplicate_prefix_or_stale_draft(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([KeyBinding::new(
                "ctrl-k ctrl-s",
                crate::keybindings::ShowKeybindings,
                None,
            )]);
            let original = StartPageButton {
                name: "Codex".into(),
                shortcut: Some("ctrl-alt-shift-x".into()),
                ..Default::default()
            };
            let other = StartPageButton {
                name: "Other".into(),
                shortcut: Some("ctrl-alt-shift-y".into()),
                ..Default::default()
            };
            let mut draft = StartPageButtonDraft {
                original: Some((0, original.clone())),
                name: cx.new(SimpleInputState::new),
                command: cx.new(SimpleInputState::new),
                shell: None,
                shortcut: original.shortcut.clone(),
                shortcut_error: None,
                shortcut_focus: cx.focus_handle(),
                shortcut_recording: None,
            };
            let buttons = [original.clone(), other.clone()];
            for rejected in ["ctrl-k", "ctrl-alt-shift-y"] {
                assert!(
                    draft
                        .accept_recorded_shortcut(rejected, &buttons, cx)
                        .is_err()
                );
                assert_eq!(draft.shortcut, original.shortcut);
            }
            let reordered = [other, original.clone()];
            assert!(
                draft
                    .accept_recorded_shortcut("f8", &reordered, cx)
                    .is_err()
            );
            assert_eq!(draft.shortcut, original.shortcut);
            assert!(
                draft
                    .accept_recorded_shortcut("ctrl-alt-shift-x", &buttons, cx)
                    .is_ok()
            );
            assert!(draft.accept_recorded_shortcut("f8", &buttons, cx).is_ok());
            assert_eq!(draft.shortcut.as_deref(), Some("f8"));
        });
    }
}
