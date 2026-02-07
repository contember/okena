use crate::keybindings::{
    format_keystroke, get_action_descriptions, get_config, Cancel,
    ShowKeybindings, ShowSessionManager, ShowThemeSelector, ShowSettings, OpenSettingsFile,
    ShowFileSearch, ShowDiffViewer, ToggleSidebar, ToggleSidebarAutoHide, ClearFocus,
    SplitVertical, SplitHorizontal, AddTab, CloseTerminal, MinimizeTerminal,
    FocusNextTerminal, FocusPrevTerminal, FocusLeft, FocusRight, FocusUp, FocusDown,
    Copy, Paste, ScrollUp, ScrollDown, Search, CreateWorktree, CheckForUpdates, InstallUpdate,
};
use crate::theme::{theme, with_alpha};
use crate::views::components::{
    badge, handle_list_overlay_key, keyboard_hints_footer, modal_backdrop, modal_content,
    search_input_area, substring_filter, ListOverlayAction, ListOverlayConfig, ListOverlayState,
};
use gpui::*;
use gpui_component::h_flex;
use gpui::prelude::*;

/// Command entry for the palette
#[derive(Clone)]
struct CommandEntry {
    /// Action name (internal identifier)
    action: String,
    /// Display name
    name: String,
    /// Description
    description: String,
    /// Category
    category: String,
    /// Primary keybinding (formatted for display)
    keybinding: Option<String>,
}

/// Command palette for quick access to all commands
pub struct CommandPalette {
    focus_handle: FocusHandle,
    state: ListOverlayState<CommandEntry>,
}

impl CommandPalette {
    pub fn new(cx: &mut Context<Self>) -> Self {
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
                    action: action.to_string(),
                    name: desc.name.to_string(),
                    description: desc.description.to_string(),
                    category: desc.category.to_string(),
                    keybinding,
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

        let state = ListOverlayState::new(commands, config, cx);
        let focus_handle = state.focus_handle.clone();

        Self { focus_handle, state }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(CommandPaletteEvent::Close);
    }

    fn execute_command(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(filter_result) = self.state.filtered.get(index) {
            let command = &self.state.items[filter_result.index];
            let action = command.action.as_str();

            // Dispatch the appropriate action
            match action {
                "ToggleSidebar" => window.dispatch_action(Box::new(ToggleSidebar), cx),
                "ToggleSidebarAutoHide" => window.dispatch_action(Box::new(ToggleSidebarAutoHide), cx),
                "ClearFocus" => window.dispatch_action(Box::new(ClearFocus), cx),
                "ShowKeybindings" => window.dispatch_action(Box::new(ShowKeybindings), cx),
                "ShowSessionManager" => window.dispatch_action(Box::new(ShowSessionManager), cx),
                "ShowThemeSelector" => window.dispatch_action(Box::new(ShowThemeSelector), cx),
                "SplitVertical" => window.dispatch_action(Box::new(SplitVertical), cx),
                "SplitHorizontal" => window.dispatch_action(Box::new(SplitHorizontal), cx),
                "AddTab" => window.dispatch_action(Box::new(AddTab), cx),
                "CloseTerminal" => window.dispatch_action(Box::new(CloseTerminal), cx),
                "MinimizeTerminal" => window.dispatch_action(Box::new(MinimizeTerminal), cx),
                "FocusNextTerminal" => window.dispatch_action(Box::new(FocusNextTerminal), cx),
                "FocusPrevTerminal" => window.dispatch_action(Box::new(FocusPrevTerminal), cx),
                "FocusLeft" => window.dispatch_action(Box::new(FocusLeft), cx),
                "FocusRight" => window.dispatch_action(Box::new(FocusRight), cx),
                "FocusUp" => window.dispatch_action(Box::new(FocusUp), cx),
                "FocusDown" => window.dispatch_action(Box::new(FocusDown), cx),
                "Copy" => window.dispatch_action(Box::new(Copy), cx),
                "Paste" => window.dispatch_action(Box::new(Paste), cx),
                "ScrollUp" => window.dispatch_action(Box::new(ScrollUp), cx),
                "ScrollDown" => window.dispatch_action(Box::new(ScrollDown), cx),
                "Search" => window.dispatch_action(Box::new(Search), cx),
                "CreateWorktree" => window.dispatch_action(Box::new(CreateWorktree), cx),
                "ShowSettings" => window.dispatch_action(Box::new(ShowSettings), cx),
                "OpenSettingsFile" => window.dispatch_action(Box::new(OpenSettingsFile), cx),
                "ShowFileSearch" => window.dispatch_action(Box::new(ShowFileSearch), cx),
                "ShowDiffViewer" => window.dispatch_action(Box::new(ShowDiffViewer), cx),
                "CheckForUpdates" => window.dispatch_action(Box::new(CheckForUpdates), cx),
                "InstallUpdate" => window.dispatch_action(Box::new(InstallUpdate), cx),
                _ => {
                    log::warn!("Unknown action in command palette: {}", action);
                }
            }

            // Close the palette after executing
            cx.emit(CommandPaletteEvent::Close);
        }
    }

    fn filter_commands(&mut self) {
        let filtered = substring_filter(&self.state.items, &self.state.search_query, |cmd| {
            vec![
                cmd.name.clone(),
                cmd.description.clone(),
                cmd.category.clone(),
                cmd.action.clone(),
            ]
        });
        self.state.set_filtered(filtered);
    }

    fn render_command_row(&self, filtered_index: usize, cmd_index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let command = &self.state.items[cmd_index];
        let is_selected = filtered_index == self.state.selected_index;

        let name = command.name.clone();
        let description = command.description.clone();
        let category = command.category.clone();
        let keybinding = command.keybinding.clone();

        div()
            .id(ElementId::Name(format!("command-{}", filtered_index).into()))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.0))
            .py(px(8.0))
            .when(is_selected, |d| d.bg(with_alpha(t.border_active, 0.15)))
            .hover(|s| s.bg(rgb(t.bg_hover)))
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
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_primary))
                                    .child(name),
                            )
                            .child(badge(category, &t)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
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
                            .text_size(px(11.0))
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

        div()
            .track_focus(&focus_handle)
            .key_context("CommandPalette")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
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
            .child(
                modal_backdrop("command-palette-backdrop", &t)
                    .items_start()
                    .pt(px(80.0))
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
                            .child(search_input_area(&search_query, &search_placeholder, &t))
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
                                        d.child(
                                            div()
                                                .px(px(12.0))
                                                .py(px(20.0))
                                                .text_size(px(13.0))
                                                .text_color(rgb(t.text_muted))
                                                .child(empty_message.clone()),
                                        )
                                    }),
                            )
                            .child(keyboard_hints_footer(&[("Enter", "to select"), ("Esc", "to close")], &t)),
                    ),
            )
    }
}

impl_focusable!(CommandPalette);
