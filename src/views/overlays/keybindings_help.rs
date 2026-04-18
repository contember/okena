use crate::keybindings::{
    format_keystroke, get_action_descriptions, get_config, get_keybindings_path,
    keystroke_to_config_string, reset_to_defaults, update_config,
    Cancel, KeybindingEntry, ShowKeybindings,
};
use crate::theme::theme;
use crate::views::components::{modal_backdrop, modal_content, modal_header, search_input_area};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm, ui_text_xl};
use gpui::*;
use gpui_component::{h_flex, v_flex};
use gpui::prelude::*;

/// Characters allowed in the keybinding search query.
const SEARCH_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -_+./";

/// State for the keybinding currently being recorded
#[derive(Clone, Debug)]
struct EditingState {
    /// The action being edited
    action: String,
    /// Index of the binding entry within the action's entries
    entry_index: usize,
    /// First chord keystroke if recording a chord sequence
    first_chord: Option<String>,
    /// Whether we're waiting for a potential second chord keystroke
    waiting_for_chord: bool,
}

/// Keybindings help overlay with inline editing
pub struct KeybindingsHelp {
    focus_handle: FocusHandle,
    show_reset_confirmation: bool,
    /// Current editing/recording state
    editing: Option<EditingState>,
    /// Timer handle for chord timeout
    _chord_timer: Option<async_channel::Sender<()>>,
    /// Conflict warning after recording
    pending_conflict: Option<String>,
    /// Keystroke interceptor subscription (active during recording)
    _interceptor: Option<Subscription>,
    /// Search query for filtering keybindings
    search_query: String,
}

impl KeybindingsHelp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            focus_handle,
            show_reset_confirmation: false,
            editing: None,
            _chord_timer: None,
            pending_conflict: None,
            _interceptor: None,
            search_query: String::new(),
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(KeybindingsHelpEvent::Close);
    }

    fn handle_reset_to_defaults(&mut self, cx: &mut Context<Self>) {
        if self.show_reset_confirmation {
            if let Err(e) = reset_to_defaults() {
                log::error!("Failed to reset keybindings: {}", e);
            }
            self.show_reset_confirmation = false;
            cx.notify();
        } else {
            self.show_reset_confirmation = true;
            cx.notify();
        }
    }

    fn cancel_reset(&mut self, cx: &mut Context<Self>) {
        self.show_reset_confirmation = false;
        cx.notify();
    }

    /// Start recording a keystroke for a specific binding
    fn start_recording(&mut self, action: String, entry_index: usize, cx: &mut Context<Self>) {
        self.editing = Some(EditingState {
            action,
            entry_index,
            first_chord: None,
            waiting_for_chord: false,
        });
        self.pending_conflict = None;
        self._chord_timer = None;

        // Install a global keystroke interceptor that fires BEFORE action dispatch.
        // This prevents recorded keystrokes from triggering their bound actions (e.g., Ctrl+B toggling the sidebar).
        let this = cx.entity().downgrade();
        self._interceptor = Some(cx.intercept_keystrokes(move |event, window, cx| {
            let keystroke = &event.keystroke;

            // Escape cancels recording instead of being recorded
            if keystroke.key == "escape" && !keystroke.modifiers.modified() {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |this, cx| {
                        this.cancel_recording(cx);
                    });
                }
                cx.stop_propagation();
                return;
            }

            let keystroke = keystroke.clone();
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    this.handle_recorded_keystroke(&keystroke, window, cx);
                });
            }
            cx.stop_propagation();
        }));

        cx.notify();
    }

    /// Cancel the current recording
    fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        self._chord_timer = None;
        self._interceptor = None;
        self.pending_conflict = None;
        cx.notify();
    }

    /// Handle a keystroke during recording
    fn handle_recorded_keystroke(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.as_mut() else {
            return;
        };

        // Ignore modifier-only keypresses
        let key = keystroke.key.as_str();
        if matches!(key, "shift" | "control" | "alt" | "platform" | "function" | "") {
            return;
        }

        let config_str = keystroke_to_config_string(keystroke);

        if editing.waiting_for_chord {
            // This is the second keystroke of a chord
            let Some(first) = editing.first_chord.take() else {
                return;
            };
            let chord = format!("{} {}", first, config_str);
            self.finalize_recording(chord, window, cx);
        } else {
            // First keystroke — start chord timer
            editing.first_chord = Some(config_str);
            editing.waiting_for_chord = true;

            // Start a 1-second timer for chord completion
            let (cancel_tx, cancel_rx) = async_channel::bounded::<()>(1);
            self._chord_timer = Some(cancel_tx);

            cx.spawn_in(window, async move |this, cx| {
                let timeout = smol::Timer::after(std::time::Duration::from_secs(1));
                smol::future::or(async { timeout.await; true }, async { let _ = cancel_rx.recv().await; false }).await;

                // If we get here and still waiting, finalize with single keystroke
                let _ = cx.update(|window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(editing) = this.editing.as_mut() {
                            if editing.waiting_for_chord {
                                if let Some(single) = editing.first_chord.take() {
                                    this.finalize_recording(single, window, cx);
                                }
                            }
                        }
                    });
                });
            }).detach();

            cx.notify();
        }
    }

    /// Finalize recording: save the new keystroke and check for conflicts
    fn finalize_recording(&mut self, new_keystroke: String, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        self._chord_timer = None;
        self._interceptor = None;

        // Update the config
        update_config(|config| {
            config.update_binding(&editing.action, editing.entry_index, new_keystroke.clone());
        });

        // Check for conflicts
        let conflicts = get_config().detect_conflicts();
        if !conflicts.is_empty() {
            let conflict_msgs: Vec<String> = conflicts.iter().map(|c| c.to_string()).collect();
            self.pending_conflict = Some(conflict_msgs.join("; "));
        } else {
            self.pending_conflict = None;
        }

        // Reload bindings in GPUI
        cx.emit(KeybindingsHelpEvent::ReloadBindings);

        cx.notify();
    }

    /// Add a new empty binding for an action
    fn add_binding_for_action(&mut self, action: &str, context: Option<String>, cx: &mut Context<Self>) {
        let entry = KeybindingEntry::new("unset", context.as_deref());

        update_config(|config| {
            config.add_binding(action, entry);
        });

        // Start recording for the new entry
        let new_index = get_config()
            .bindings
            .get(action)
            .map(|e| e.len().saturating_sub(1))
            .unwrap_or(0);

        self.start_recording(action.to_string(), new_index, cx);
    }

    /// Remove a binding entry
    fn remove_binding_entry(&mut self, action: &str, entry_index: usize, cx: &mut Context<Self>) {
        update_config(|config| {
            config.remove_binding(action, entry_index);
        });

        cx.emit(KeybindingsHelpEvent::ReloadBindings);

        cx.notify();
    }

    /// Toggle enabled/disabled state
    fn toggle_binding_entry(&mut self, action: &str, entry_index: usize, cx: &mut Context<Self>) {
        update_config(|config| {
            config.toggle_binding(action, entry_index);
        });

        cx.emit(KeybindingsHelpEvent::ReloadBindings);

        cx.notify();
    }

    /// Reset a single action to defaults
    fn reset_single_action(&mut self, action: &str, cx: &mut Context<Self>) {
        update_config(|config| {
            config.reset_single_action(action);
        });

        cx.emit(KeybindingsHelpEvent::ReloadBindings);

        cx.notify();
    }

    fn render_category(
        &self,
        category: &str,
        bindings: &[(String, Vec<(String, usize, bool, bool)>)], // (action_name, [(keystroke, entry_idx, is_customized, is_enabled)])
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let descriptions = get_action_descriptions();
        let category_string = category.to_string();

        div()
            .mb(px(16.0))
            .child(
                div()
                    .text_size(ui_text(13.0, cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_primary))
                    .mb(px(8.0))
                    .child(category_string),
            )
            .child(
                div()
                    .bg(rgb(t.bg_secondary))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(t.border))
                    .children(bindings.iter().enumerate().map(
                        |(i, (action, entries))| {
                            let description = descriptions
                                .get(action.as_str())
                                .map(|d| d.description)
                                .unwrap_or("Unknown action");
                            let name = descriptions
                                .get(action.as_str())
                                .map(|d| d.name)
                                .unwrap_or(action.as_str());

                            let is_action_customized = entries.iter().any(|(_, _, c, _)| *c);
                            let action_clone = action.clone();
                            let action_for_reset = action.clone();
                            // Determine context from first entry for "add" button
                            let entry_context = {
                                let config = get_config();
                                config.bindings.get(action.as_str())
                                    .and_then(|entries| entries.first())
                                    .and_then(|e| e.context.clone())
                            };

                            div()
                                .when(i > 0, |d| {
                                    d.border_t_1().border_color(rgb(t.border))
                                })
                                .child(
                                    // Action info row
                                    h_flex()
                                        .justify_between()
                                        .px(px(12.0))
                                        .pt(px(8.0))
                                        .pb(px(4.0))
                                        .child(
                                            v_flex()
                                                .gap(px(2.0))
                                                .child(
                                                    h_flex()
                                                        .gap(px(8.0))
                                                        .child(
                                                            div()
                                                                .text_size(ui_text(13.0, cx))
                                                                .text_color(rgb(t.text_primary))
                                                                .child(name.to_string()),
                                                        )
                                                        .when(is_action_customized, |d| {
                                                            d.child(
                                                                div()
                                                                    .text_size(ui_text_sm(cx))
                                                                    .px(px(4.0))
                                                                    .py(px(1.0))
                                                                    .rounded(px(3.0))
                                                                    .bg(rgb(t.border_active))
                                                                    .text_color(rgb(0xFFFFFF))
                                                                    .child("Custom"),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(ui_text_ms(cx))
                                                        .text_color(rgb(t.text_muted))
                                                        .child(description.to_string()),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap(px(4.0))
                                                // Add binding button
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!("add-{}", action_clone)))
                                                        .cursor_pointer()
                                                        .px(px(6.0))
                                                        .py(px(2.0))
                                                        .rounded(px(3.0))
                                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                                        .text_size(ui_text_sm(cx))
                                                        .text_color(rgb(t.text_muted))
                                                        .child("+")
                                                        .on_mouse_down(MouseButton::Left, {
                                                            let action = action_clone.clone();
                                                            let ctx = entry_context.clone();
                                                            cx.listener(move |this, _, _window, cx| {
                                                                this.add_binding_for_action(&action, ctx.clone(), cx);
                                                            })
                                                        }),
                                                )
                                                // Reset single action button (only if customized)
                                                .when(is_action_customized, |d| {
                                                    d.child(
                                                        div()
                                                            .id(SharedString::from(format!("reset-{}", action_for_reset)))
                                                            .cursor_pointer()
                                                            .px(px(6.0))
                                                            .py(px(2.0))
                                                            .rounded(px(3.0))
                                                            .hover(|s| s.bg(rgb(t.bg_hover)))
                                                            .text_size(ui_text_sm(cx))
                                                            .text_color(rgb(t.text_muted))
                                                            .child("↺")
                                                            .on_mouse_down(MouseButton::Left, {
                                                                let action = action_for_reset.clone();
                                                                cx.listener(move |this, _, _window, cx| {
                                                                    this.reset_single_action(&action, cx);
                                                                })
                                                            }),
                                                    )
                                                }),
                                        ),
                                )
                                // Individual binding entries
                                .children(entries.iter().map(|(keystroke, entry_idx, _is_customized, is_enabled)| {
                                    let action_name = action.clone();
                                    let action_for_toggle = action.clone();
                                    let action_for_remove = action.clone();
                                    let idx = *entry_idx;
                                    let enabled = *is_enabled;
                                    let ks = keystroke.clone();

                                    // Check if this entry is currently being recorded
                                    let is_recording = self.editing.as_ref().map_or(false, |e| {
                                        e.action == action_name && e.entry_index == idx
                                    });
                                    let is_waiting_chord = is_recording && self.editing.as_ref().map_or(false, |e| e.waiting_for_chord);

                                    h_flex()
                                        .justify_between()
                                        .px(px(12.0))
                                        .pl(px(24.0))
                                        .py(px(4.0))
                                        .child(
                                            // Keystroke badge (clickable to record)
                                            div()
                                                .id(SharedString::from(format!("ks-{}-{}", action_name, idx)))
                                                .cursor_pointer()
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .rounded(px(4.0))
                                                .border_1()
                                                .when(is_recording, |d| {
                                                    d.bg(rgb(t.border_active))
                                                        .border_color(rgb(t.border_active))
                                                        .text_color(rgb(0xFFFFFF))
                                                })
                                                .when(!is_recording, |d| {
                                                    d.bg(rgb(t.bg_primary))
                                                        .border_color(rgb(t.border))
                                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                                        .when(!enabled, |d| d.opacity(0.4))
                                                })
                                                .text_size(ui_text_md(cx))
                                                .font_family("monospace")
                                                .text_color(if is_recording { rgb(0xFFFFFF) } else { rgb(t.text_secondary) })
                                                .child(if is_recording {
                                                    if is_waiting_chord {
                                                        let first = self.editing.as_ref()
                                                            .and_then(|e| e.first_chord.as_ref())
                                                            .map(|s| format_keystroke(s))
                                                            .unwrap_or_default();
                                                        format!("{} ...", first)
                                                    } else {
                                                        "Press keys...".to_string()
                                                    }
                                                } else {
                                                    format_keystroke(&ks)
                                                })
                                                .on_mouse_down(MouseButton::Left, {
                                                    let action = action_name.clone();
                                                    cx.listener(move |this, _, _window, cx| {
                                                        if this.editing.as_ref().map_or(false, |e| e.action == action && e.entry_index == idx) {
                                                            this.cancel_recording(cx);
                                                        } else {
                                                            this.start_recording(action.clone(), idx, cx);
                                                        }
                                                    })
                                                }),
                                        )
                                        .child(
                                            h_flex()
                                                .gap(px(4.0))
                                                // Toggle enabled/disabled
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!("toggle-{}-{}", action_for_toggle, idx)))
                                                        .cursor_pointer()
                                                        .px(px(6.0))
                                                        .py(px(2.0))
                                                        .rounded(px(3.0))
                                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                                        .text_size(ui_text_sm(cx))
                                                        .text_color(if enabled { rgb(t.text_muted) } else { rgb(t.error) })
                                                        .child(if enabled { "●" } else { "○" })
                                                        .on_mouse_down(MouseButton::Left, {
                                                            let action = action_for_toggle.clone();
                                                            cx.listener(move |this, _, _window, cx| {
                                                                this.toggle_binding_entry(&action, idx, cx);
                                                            })
                                                        }),
                                                )
                                                // Remove binding
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!("rm-{}-{}", action_for_remove, idx)))
                                                        .cursor_pointer()
                                                        .px(px(6.0))
                                                        .py(px(2.0))
                                                        .rounded(px(3.0))
                                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                                        .text_size(ui_text_sm(cx))
                                                        .text_color(rgb(t.text_muted))
                                                        .child("×")
                                                        .on_mouse_down(MouseButton::Left, {
                                                            let action = action_for_remove.clone();
                                                            cx.listener(move |this, _, _window, cx| {
                                                                this.remove_binding_entry(&action, idx, cx);
                                                            })
                                                        }),
                                                ),
                                        )
                                }))
                        },
                    )),
            )
    }
}

pub enum KeybindingsHelpEvent {
    Close,
    ReloadBindings,
}

impl EventEmitter<KeybindingsHelpEvent> for KeybindingsHelp {}

impl Render for KeybindingsHelp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let config = get_config();
        let customized = config.get_customized_actions();
        let conflicts = config.detect_conflicts();

        // Group bindings by category, with per-action entry details
        let descriptions = get_action_descriptions();
        let query = self.search_query.to_lowercase();
        let mut categories: std::collections::HashMap<&str, Vec<(String, Vec<(String, usize, bool, bool)>)>> =
            std::collections::HashMap::new();

        // Track which actions we've already processed
        let mut seen_actions: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (action, entries) in &config.bindings {
            if seen_actions.contains(action) {
                continue;
            }
            seen_actions.insert(action.clone());

            let desc = descriptions.get(action.as_str());
            let category = desc.map(|d| d.category).unwrap_or("Other");

            // Filter by search query: match against name, description, category, or keystroke
            if !query.is_empty() {
                let name = desc.map(|d| d.name).unwrap_or("");
                let description = desc.map(|d| d.description).unwrap_or("");
                let keystrokes_match = entries.iter().any(|e| e.keystroke.to_lowercase().contains(&query));
                if !name.to_lowercase().contains(&query)
                    && !description.to_lowercase().contains(&query)
                    && !category.to_lowercase().contains(&query)
                    && !keystrokes_match
                {
                    continue;
                }
            }

            let is_customized = customized.contains(action);
            let entry_details: Vec<(String, usize, bool, bool)> = entries
                .iter()
                .enumerate()
                .map(|(idx, entry)| {
                    (entry.keystroke.clone(), idx, is_customized, entry.enabled)
                })
                .collect();

            if !entry_details.is_empty() {
                categories
                    .entry(category)
                    .or_insert_with(Vec::new)
                    .push((action.clone(), entry_details));
            }
        }

        let category_order = ["Global", "Terminal", "Navigation", "Search", "Fullscreen", "Layout", "Project", "Services", "Git", "Other"];

        let focus_handle = self.focus_handle.clone();
        let is_editing = self.editing.is_some();

        modal_backdrop("keybindings-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("KeybindingsHelp")
            .items_center()
            .on_action(cx.listener(|this, _: &ShowKeybindings, _window, cx| {
                this.close(cx);
            }))
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                if this.editing.is_some() {
                    this.cancel_recording(cx);
                } else if !this.search_query.is_empty() {
                    this.search_query.clear();
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                // Don't handle typing while recording a keybinding — the interceptor handles that
                if this.editing.is_some() {
                    return;
                }
                let key = event.keystroke.key.as_str();
                match key {
                    "backspace" => {
                        if this.search_query.pop().is_some() {
                            cx.notify();
                        }
                    }
                    k if k.len() == 1 && !event.keystroke.modifiers.modified() => {
                        let Some(ch) = k.chars().next() else {
                            return;
                        };
                        if SEARCH_CHARS.contains(ch) {
                            this.search_query.push(ch);
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                if this.editing.is_some() {
                    this.cancel_recording(cx);
                } else {
                    this.close(cx);
                }
            }))
            .child(
                modal_content("keybindings-modal", &t)
                    .w(px(650.0))
                    .max_h(px(700.0))
                    // Stop click propagation on modal content
                    .on_mouse_down(MouseButton::Left, |_, _, _| {})
                    .child(modal_header(
                        "Keyboard Shortcuts",
                        Some(if is_editing { "Press keys to record, ESC to cancel" } else { "Click a binding to change it" }),
                        &t,
                        cx,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(search_input_area(&self.search_query, "Search keybindings…", &t))
                    // Conflict/info banners
                    .child(
                        div()
                            .when(!conflicts.is_empty(), |d| {
                                d.px(px(16.0))
                                    .py(px(8.0))
                                    .bg(rgb(t.warning))
                                    .border_b_1()
                                    .border_color(rgb(t.border))
                                    .child(
                                        h_flex()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(ui_text_xl(cx))
                                                    .child("⚠️"),
                                            )
                                            .child(
                                                v_flex()
                                                    .gap(px(2.0))
                                                    .child(
                                                        div()
                                                            .text_size(ui_text_md(cx))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(rgb(t.text_primary))
                                                            .child(format!(
                                                                "{} keybinding conflict{}",
                                                                conflicts.len(),
                                                                if conflicts.len() == 1 { "" } else { "s" }
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(ui_text_ms(cx))
                                                            .text_color(rgb(t.text_secondary))
                                                            .child(
                                                                conflicts
                                                                    .iter()
                                                                    .map(|c| c.to_string())
                                                                    .collect::<Vec<_>>()
                                                                    .join("; "),
                                                            ),
                                                    ),
                                            ),
                                    )
                            }),
                    )
                    .child(
                        // Scrollable content
                        div()
                            .id("keybindings-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .px(px(16.0))
                            .py(px(12.0))
                            .children(category_order.iter().filter_map(|category| {
                                categories.get(category).map(|bindings| {
                                    self.render_category(category, bindings, cx)
                                })
                            })),
                    )
                    .child(
                        // Footer
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                v_flex()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(ui_text_ms(cx))
                                            .text_color(rgb(t.text_muted))
                                            .child("Configuration file:"),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_sm(cx))
                                            .font_family("monospace")
                                            .text_color(rgb(t.text_secondary))
                                            .child(get_keybindings_path().display().to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .when(self.show_reset_confirmation, |d| {
                                        d.flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(ui_text_md(cx))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("Reset all?"),
                                            )
                                            .child(
                                                div()
                                                    .id("reset-confirm-btn")
                                                    .cursor_pointer()
                                                    .px(px(10.0))
                                                    .py(px(6.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(t.error))
                                                    .text_size(ui_text_md(cx))
                                                    .text_color(rgb(0xFFFFFF))
                                                    .child("Confirm")
                                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                                                        this.handle_reset_to_defaults(cx);
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("reset-cancel-btn")
                                                    .cursor_pointer()
                                                    .px(px(10.0))
                                                    .py(px(6.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(t.bg_secondary))
                                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                                    .text_size(ui_text_md(cx))
                                                    .text_color(rgb(t.text_primary))
                                                    .child("Cancel")
                                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                                                        this.cancel_reset(cx);
                                                    })),
                                            )
                                    })
                                    .when(!self.show_reset_confirmation, |d| {
                                        d.child(
                                            div()
                                                .id("reset-defaults-btn")
                                                .cursor_pointer()
                                                .px(px(10.0))
                                                .py(px(6.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(t.bg_secondary))
                                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                                .text_size(ui_text_md(cx))
                                                .text_color(rgb(t.text_primary))
                                                .child("Reset to Defaults")
                                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                                                    this.handle_reset_to_defaults(cx);
                                                })),
                                        )
                                    }),
                            ),
                    ),
            )
    }
}

impl_focusable!(KeybindingsHelp);
