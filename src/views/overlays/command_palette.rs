use crate::keybindings::{
    format_keystroke, get_action_descriptions, get_config,
    ShowKeybindings, ShowSessionManager, ShowThemeSelector, ShowSettings, OpenSettingsFile,
    ToggleSidebar, ToggleSidebarAutoHide, ClearFocus,
    SplitVertical, SplitHorizontal, AddTab, CloseTerminal, MinimizeTerminal,
    FocusNextTerminal, FocusPrevTerminal, FocusLeft, FocusRight, FocusUp, FocusDown,
    Copy, Paste, ScrollUp, ScrollDown, Search, CreateWorktree,
};
use crate::theme::{theme, with_alpha};
use gpui::*;
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
    scroll_handle: ScrollHandle,
    commands: Vec<CommandEntry>,
    filtered_commands: Vec<usize>,
    selected_index: usize,
    search_query: String,
}

impl CommandPalette {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let scroll_handle = ScrollHandle::new();

        // Build command list from action descriptions
        let descriptions = get_action_descriptions();
        let config = get_config();

        let mut commands: Vec<CommandEntry> = descriptions
            .iter()
            .map(|(action, desc)| {
                // Get primary keybinding for this action
                let keybinding = config
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

        let filtered_commands: Vec<usize> = (0..commands.len()).collect();

        Self {
            focus_handle,
            scroll_handle,
            commands,
            filtered_commands,
            selected_index: 0,
            search_query: String::new(),
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(CommandPaletteEvent::Close);
    }

    fn scroll_to_selected(&self) {
        if !self.filtered_commands.is_empty() {
            self.scroll_handle.scroll_to_item(self.selected_index);
        }
    }

    fn execute_command(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&cmd_index) = self.filtered_commands.get(index) {
            let command = &self.commands[cmd_index];
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
                _ => {
                    log::warn!("Unknown action in command palette: {}", action);
                }
            }

            // Close the palette after executing
            cx.emit(CommandPaletteEvent::Close);
        }
    }

    fn filter_commands(&mut self) {
        let query = self.search_query.to_lowercase();

        if query.is_empty() {
            self.filtered_commands = (0..self.commands.len()).collect();
        } else {
            self.filtered_commands = self.commands
                .iter()
                .enumerate()
                .filter(|(_, cmd)| {
                    cmd.name.to_lowercase().contains(&query)
                        || cmd.description.to_lowercase().contains(&query)
                        || cmd.category.to_lowercase().contains(&query)
                        || cmd.action.to_lowercase().contains(&query)
                })
                .map(|(i, _)| i)
                .collect();
        }

        // Reset selection to first item
        self.selected_index = 0;
    }

    fn render_command_row(&self, filtered_index: usize, cmd_index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let command = &self.commands[cmd_index];
        let is_selected = filtered_index == self.selected_index;

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
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(t.text_primary))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(rgb(t.bg_secondary))
                                    .text_size(px(9.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(category),
                            ),
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
                div()
                    .flex()
                    .items_center()
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
        let filtered_commands = self.filtered_commands.clone();
        let search_query = self.search_query.clone();

        // Focus on first render
        window.focus(&focus_handle, cx);

        div()
            .track_focus(&focus_handle)
            .key_context("CommandPalette")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        this.close(cx);
                    }
                    "up" => {
                        if this.selected_index > 0 {
                            this.selected_index -= 1;
                            this.scroll_to_selected();
                            cx.notify();
                        }
                    }
                    "down" => {
                        if this.selected_index < this.filtered_commands.len().saturating_sub(1) {
                            this.selected_index += 1;
                            this.scroll_to_selected();
                            cx.notify();
                        }
                    }
                    "enter" => {
                        let index = this.selected_index;
                        this.execute_command(index, window, cx);
                    }
                    "backspace" => {
                        if !this.search_query.is_empty() {
                            this.search_query.pop();
                            this.filter_commands();
                            cx.notify();
                        }
                    }
                    key if key.len() == 1 => {
                        // Single character - add to search
                        let ch = key.chars().next().unwrap();
                        if ch.is_alphanumeric() || ch == ' ' || ch == '-' || ch == '_' {
                            this.search_query.push(ch);
                            this.filter_commands();
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            }))
            .absolute()
            .inset_0()
            .bg(hsla(0.0, 0.0, 0.0, 0.5))
            .flex()
            .items_start()
            .justify_center()
            .pt(px(80.0))
            .id("command-palette-backdrop")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                // Modal content
                div()
                    .id("command-palette-modal")
                    .w(px(550.0))
                    .max_h(px(450.0))
                    .bg(rgb(t.bg_primary))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(t.border))
                    .shadow_xl()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _window, _cx| {})
                    .child(
                        // Search input area
                        div()
                            .px(px(12.0))
                            .py(px(10.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(">"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(14.0))
                                    .text_color(if search_query.is_empty() {
                                        rgb(t.text_muted)
                                    } else {
                                        rgb(t.text_primary)
                                    })
                                    .child(if search_query.is_empty() {
                                        "Type to search commands...".to_string()
                                    } else {
                                        search_query
                                    }),
                            ),
                    )
                    .child(
                        // Command list
                        div()
                            .id("command-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .children(
                                filtered_commands
                                    .iter()
                                    .enumerate()
                                    .map(|(i, &cmd_index)| self.render_command_row(i, cmd_index, cx)),
                            )
                            .when(filtered_commands.is_empty(), |d| {
                                d.child(
                                    div()
                                        .px(px(12.0))
                                        .py(px(20.0))
                                        .text_size(px(13.0))
                                        .text_color(rgb(t.text_muted))
                                        .child("No commands found"),
                                )
                            }),
                    )
                    .child(
                        // Footer with hints
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .items_center()
                            .gap(px(16.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .px(px(4.0))
                                            .py(px(1.0))
                                            .rounded(px(3.0))
                                            .bg(rgb(t.bg_secondary))
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Enter"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("to select"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .px(px(4.0))
                                            .py(px(1.0))
                                            .rounded(px(3.0))
                                            .bg(rgb(t.bg_secondary))
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Esc"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("to close"),
                                    ),
                            ),
                    ),
            )
    }
}

impl_focusable!(CommandPalette);
