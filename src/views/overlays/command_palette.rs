use crate::keybindings::{format_keystroke, get_action_descriptions, get_config, Cancel};
use crate::theme::theme;
use crate::views::components::{
    badge, handle_list_overlay_key, keyboard_hints_footer, modal_backdrop, modal_content,
    search_input_area_selected, substring_filter, ListOverlayAction, ListOverlayConfig, ListOverlayState,
};
use crate::ui::tokens::{ui_text, ui_text_ms};
use gpui::*;
use gpui_component::h_flex;
use gpui::prelude::*;
use okena_ui::empty_state::empty_state;
use okena_ui::selectable_list::selectable_list_item;

/// Remembered state from the last command palette session.
#[derive(Default)]
struct CommandPaletteMemory {
    query: String,
}

impl Global for CommandPaletteMemory {}

/// Command entry for the palette
#[derive(Clone)]
struct CommandEntry {
    /// Display name
    name: String,
    /// Description
    description: String,
    /// Category
    category: String,
    /// Primary keybinding (formatted for display)
    keybinding: Option<String>,
    /// Factory to create the action for dispatch
    factory: fn() -> Box<dyn gpui::Action>,
}

/// Command palette for quick access to all commands
pub struct CommandPalette {
    workspace: Entity<okena_workspace::state::Workspace>,
    focus_handle: FocusHandle,
    state: ListOverlayState<CommandEntry>,
    /// When true, the entire query is "selected" — first keystroke replaces it.
    select_all: bool,
}

impl CommandPalette {
    pub fn new(workspace: Entity<okena_workspace::state::Workspace>, cx: &mut Context<Self>) -> Self {
        // Build command list from action descriptions
        let descriptions = get_action_descriptions();
        let config_data = get_config();

        let mut commands: Vec<CommandEntry> = descriptions
            .iter()
            .map(|(action, desc)| {
                // Get primary keybinding for this action
                let keybinding = config_data
                    .bindings
                    .get(*action)
                    .and_then(|entries| entries.iter().find(|e| e.enabled))
                    .map(|e| format_keystroke(&e.keystroke));

                CommandEntry {
                    name: desc.name.to_string(),
                    description: desc.description.to_string(),
                    category: desc.category.to_string(),
                    keybinding,
                    factory: desc.factory,
                }
            })
            .collect();

        // Sort by category then name
        commands.sort_by(|a, b| {
            a.category.cmp(&b.category).then(a.name.cmp(&b.name))
        });

        let config = ListOverlayConfig::new("Command Palette")
            .searchable("Type to search commands...")
            .size(550.0, 450.0)
            .empty_message("No commands found")
            .keyboard_hints(vec![("Enter", "to select"), ("Esc", "to close")])
            .key_context("CommandPalette");

        // Restore from previous session
        let query = cx.try_global::<CommandPaletteMemory>()
            .map(|m| m.query.clone())
            .unwrap_or_default();
        let select_all = !query.is_empty();

        let state = ListOverlayState::new(commands, config, cx);
        let focus_handle = state.focus_handle.clone();

        let mut palette = Self { workspace, focus_handle, state, select_all };

        if !query.is_empty() {
            palette.state.search_query = query;
            palette.filter_commands();
        }

        palette
    }

    fn save_memory(&self, cx: &mut Context<Self>) {
        cx.set_global(CommandPaletteMemory {
            query: self.state.search_query.clone(),
        });
    }

    fn close(&self, cx: &mut Context<Self>) {
        self.save_memory(cx);
        cx.emit(CommandPaletteEvent::Close);
    }

    fn execute_command(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(filter_result) = self.state.filtered.get(index) {
            let command = &self.state.items[filter_result.index];
            let action = (command.factory)();
            self.save_memory(cx);

            // Restore focus to the terminal pane before dispatching so that
            // context-scoped actions (e.g. CloseTerminal on "TerminalPane")
            // are routed to the correct element.
            let pane_map = okena_views_terminal::layout::navigation::get_pane_map();
            if let Some(focused) = self.workspace.read(cx).focus_manager
                .focused_terminal_state()
            {
                if let Some(pane) = pane_map.find_pane(&focused.project_id, &focused.layout_path) {
                    if let Some(ref fh) = pane.focus_handle {
                        window.focus(fh, cx);
                    }
                }
            }

            window.dispatch_action(action, cx);
            cx.emit(CommandPaletteEvent::Close);
        }
    }

    fn filter_commands(&mut self) {
        let filtered = substring_filter(&self.state.items, &self.state.search_query, |cmd| {
            vec![
                cmd.name.clone(),
                cmd.description.clone(),
                cmd.category.clone(),
            ]
        });
        self.state.set_filtered(filtered);
    }

    fn render_command_row(&self, filtered_index: usize, cmd_index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let command = &self.state.items[cmd_index];
        let is_selected = filtered_index == self.state.selected_index;

        let name = command.name.clone();
        let description = command.description.clone();
        let category = command.category.clone();
        let keybinding = command.keybinding.clone();

        selectable_list_item(
                ElementId::Name(format!("command-{}", filtered_index).into()),
                is_selected,
                &t,
            )
            .justify_between()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.execute_command(filtered_index, window, cx);
                }),
            )
            .child(
                // Left side: name + description
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(ui_text(13.0, cx))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_primary))
                                    .child(name),
                            )
                            .child(badge(category, &t)),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(description),
                    ),
            )
            .child(
                // Right side: keybinding
                h_flex()
                    .children(keybinding.map(|kb| {
                        div()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .text_size(ui_text_ms(cx))
                            .font_family("monospace")
                            .text_color(rgb(t.text_secondary))
                            .child(kb)
                    })),
            )
    }
}

pub enum CommandPaletteEvent {
    Close,
}

impl EventEmitter<CommandPaletteEvent> for CommandPalette {}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let search_query = self.state.search_query.clone();
        let config_width = self.state.config.width;
        let config_max_height = self.state.config.max_height;
        let search_placeholder = self.state.config.search_placeholder.clone().unwrap_or_default();
        let empty_message = self.state.config.empty_message.clone();

        // Focus on first render
        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("command-palette-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("CommandPalette")
            .items_start()
            .pt(px(80.0))
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                // Handle select_all: on typing or backspace, clear query first
                if this.select_all {
                    match key {
                        "backspace" => {
                            this.state.search_query.clear();
                            this.select_all = false;
                            this.filter_commands();
                            cx.notify();
                            return;
                        }
                        k if k.len() == 1 => {
                            let ch = k.chars().next().unwrap();
                            if "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -_./".contains(ch) {
                                this.state.search_query.clear();
                                this.select_all = false;
                                // Fall through to handle_list_overlay_key which will push the char
                            }
                        }
                        "up" | "down" => {
                            this.select_all = false;
                        }
                        _ => {}
                    }
                }
                match handle_list_overlay_key(&mut this.state, event, &[]) {
                    ListOverlayAction::Close => this.close(cx),
                    ListOverlayAction::SelectPrev | ListOverlayAction::SelectNext => {
                        this.state.scroll_to_selected();
                        cx.notify();
                    }
                    ListOverlayAction::Confirm => {
                        let index = this.state.selected_index;
                        this.execute_command(index, window, cx);
                    }
                    ListOverlayAction::QueryChanged => {
                        this.filter_commands();
                        cx.notify();
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("command-palette-modal", &t)
                    .w(px(config_width))
                    .max_h(px(config_max_height))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(search_input_area_selected(&search_query, &search_placeholder, self.select_all, &t))
                    .child(
                        // Command list
                        div()
                            .id("command-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.state.scroll_handle)
                            .children(
                                self.state.filtered
                                    .iter()
                                    .enumerate()
                                    .map(|(i, filter_result)| self.render_command_row(i, filter_result.index, cx)),
                            )
                            .when(self.state.is_empty(), |d| {
                                d.child(empty_state(empty_message.clone(), &t, cx))
                            }),
                    )
                    .child(keyboard_hints_footer(&[("Enter", "to select"), ("Esc", "to close")], &t)),
            )
    }
}

impl_focusable!(CommandPalette);
