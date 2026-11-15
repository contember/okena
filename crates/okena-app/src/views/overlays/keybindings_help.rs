mod search;
use crate::keybindings::{
    Cancel, KeybindingEntry, ShowKeybindings, format_keystroke, get_action_descriptions,
    get_config, get_keybindings_path, keystroke_to_config_string, reset_to_defaults, update_config,
};
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm, ui_text_xl};
use crate::views::components::list_overlay::{focus_input, search_input_row};
use crate::views::components::{
    SimpleInputState, dropdown_anchored_below, dropdown_button, dropdown_option, dropdown_overlay,
    modal_backdrop, modal_content, modal_header,
};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_ui::shortcuts::ShortcutPlatform;
use okena_ui::simple_input::InputChangedEvent;
use search::{CaptureQuery, build_rows};

/// One displayed binding entry: `(keystroke, entry_idx, is_customized, is_enabled)`.
type BindingEntry = (String, usize, bool, bool);
/// All binding entries for an action: `(action_name, entries)`.
type ActionBindings = (String, Vec<BindingEntry>);

/// State for the keybinding currently being recorded
#[derive(Clone, Debug)]
struct EditingState {
    /// The action being edited
    action: String,
    /// Index of the binding entry within the action's entries
    entry_index: usize,
    /// First chord keystroke if recording a chord sequence
    first_chord: Option<String>,
    draft_context: Option<Option<String>>,
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
    recording_generation: u64,
    /// Conflict warning after recording
    pending_conflict: Option<String>,
    /// Keystroke interceptor subscription (active during recording)
    _interceptor: Option<Subscription>,
    /// Search input for filtering keybindings. While a binding is being
    /// recorded the keystroke interceptor swallows every key, so the input
    /// never sees the recorded keys.
    search_input: Entity<SimpleInputState>,
    capture: Option<CaptureQuery>,
    assigned_only: bool,
    filter_open: Option<usize>,
    filter_bounds: [Option<Bounds<Pixels>>; 2],
    all_platforms: bool,
    capture_window: Option<AnyWindowHandle>,
    activation_subscription: Option<Subscription>,
    held_keys: std::collections::HashSet<String>,
    disk_snapshot: Option<String>,
    file_error: Option<String>,
    rollback: Option<crate::keybindings::KeybindingConfig>,
}

impl KeybindingsHelp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (disk_snapshot, file_error) =
            match crate::keybindings::refresh_keybindings_from_disk(cx) {
                Ok(snapshot) => (snapshot, None),
                Err(error) => (None, Some(error.to_string())),
            };
        Self::with_snapshot(cx, disk_snapshot, file_error)
    }

    fn with_snapshot(
        cx: &mut Context<Self>,
        disk_snapshot: Option<String>,
        file_error: Option<String>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let search_input =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("Search keybindings…"));
        cx.subscribe(&search_input, |_, _, _: &InputChangedEvent, cx| cx.notify())
            .detach();
        Self {
            focus_handle,
            show_reset_confirmation: false,
            editing: None,
            _chord_timer: None,
            recording_generation: 0,
            pending_conflict: None,
            _interceptor: None,
            search_input,
            capture: None,
            assigned_only: false,
            filter_open: None,
            filter_bounds: [None, None],
            all_platforms: false,
            capture_window: None,
            activation_subscription: None,
            held_keys: Default::default(),
            disk_snapshot,
            file_error,
            rollback: None,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.cancel_recording(cx);
        self.filter_open = None;
        self.capture = None;
        cx.emit(KeybindingsHelpEvent::Close);
    }

    fn handle_reset_to_defaults(&mut self, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        if self.show_reset_confirmation {
            let snapshot = match reset_to_defaults(self.disk_snapshot.as_deref()) {
                Ok(snapshot) => snapshot,
                Err(e) => {
                    self.file_error = Some(e.to_string());
                    cx.notify();
                    return;
                }
            };
            self.cancel_recording(cx);
            self.stop_search(cx);
            self.disk_snapshot = Some(snapshot);
            cx.emit(KeybindingsHelpEvent::ReloadBindings);
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
        self.stop_search(cx);
        self.recording_generation += 1;
        self.editing = Some(EditingState {
            action,
            entry_index,
            first_chord: None,
            draft_context: None,
            waiting_for_chord: false,
        });
        self.pending_conflict = None;
        self._chord_timer = None;

        self.install_interceptor(cx);

        cx.notify();
    }

    fn install_interceptor(&mut self, cx: &mut Context<Self>) {
        self.held_keys.clear();
        let this = cx.entity().downgrade();
        self._interceptor = Some(cx.intercept_keystrokes(move |event, window, cx| {
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.capture_window != Some(window.window_handle())
                    || (this.capture.is_none() && this.editing.is_none())
                {
                    return;
                }
                let key = &event.keystroke;
                // Key-up clears this set. Repeated key-down events are swallowed.
                if !this.held_keys.insert(search::held_key(key, cx)) {
                    cx.stop_propagation();
                    return;
                }
                if key.key == "escape" && !key.modifiers.modified() {
                    if this.filter_open.take().is_some() {
                        cx.notify();
                    } else if this.capture.is_some() {
                        this.capture = Some(CaptureQuery::default());
                        cx.notify();
                    } else {
                        this.cancel_recording(cx);
                    }
                } else if let Some(query) = this.capture.as_mut() {
                    query.push(key, &get_config(), this.all_platforms);
                    cx.notify();
                } else {
                    this.handle_recorded_keystroke(key, window, cx);
                }
                cx.stop_propagation();
            });
        }));
    }

    fn stop_search(&mut self, cx: &mut Context<Self>) {
        self.capture = None;
        self._interceptor = None;
        self.held_keys.clear();
        cx.notify();
    }

    fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.capture.is_some() {
            self.stop_search(cx);
        } else {
            self.cancel_recording(cx);
            self.capture = Some(CaptureQuery::default());
            self.capture_window = Some(window.window_handle());
            self.focus_handle.focus(window, cx);
            self.install_interceptor(cx);
        }
        cx.notify();
    }

    fn can_save(&mut self, cx: &mut Context<Self>) -> bool {
        let disk = match std::fs::read_to_string(get_keybindings_path()) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                self.file_error = Some(error.to_string());
                cx.notify();
                return false;
            }
        };
        let invalid = disk
            .as_deref()
            .is_some_and(|text| crate::keybindings::parse_keybindings(text).is_err());
        if disk != self.disk_snapshot || invalid {
            self.file_error = Some(
                "Configuration changed on disk or is invalid. Reload from disk before editing."
                    .into(),
            );
            cx.notify();
            return false;
        }
        self.rollback = Some(get_config().clone());
        true
    }

    fn save_changes(&mut self, cx: &mut Context<Self>) {
        let config = get_config().clone();
        match crate::keybindings::save_keybindings_if_unchanged(
            &config,
            self.disk_snapshot.as_deref(),
        ) {
            Ok(snapshot) => {
                self.disk_snapshot = Some(snapshot);
                self.file_error = None;
                self.rollback = None;
                cx.emit(KeybindingsHelpEvent::ReloadBindings);
            }
            Err(error) => {
                if let Some(previous) = self.rollback.take() {
                    update_config(|config| *config = previous);
                }
                self.file_error = Some(error.to_string());
            }
        }
        cx.notify();
    }

    fn reload_from_disk(&mut self, cx: &mut Context<Self>) {
        self.cancel_recording(cx);
        self.stop_search(cx);
        match crate::keybindings::refresh_keybindings_from_disk(cx) {
            Ok(snapshot) => {
                self.disk_snapshot = snapshot;
                self.file_error = None;
            }
            Err(error) => self.file_error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn open_json(&mut self, cx: &mut Context<Self>) {
        self.cancel_recording(cx);
        self.stop_search(cx);
        let result = (|| -> anyhow::Result<()> {
            let path = get_keybindings_path();
            if !path.exists() {
                self.disk_snapshot = Some(crate::keybindings::save_keybindings_if_unchanged(
                    &get_config(),
                    None,
                )?);
            }
            #[cfg(target_os = "windows")]
            okena_core::process::spawn_and_reap(
                okena_core::process::command("notepad").arg(&path),
            )?;
            #[cfg(target_os = "macos")]
            okena_core::process::spawn_and_reap(
                okena_core::process::command("open").arg("-t").arg(&path),
            )?;
            #[cfg(target_os = "linux")]
            okena_core::process::spawn_and_reap(
                okena_core::process::command("xdg-open").arg(&path),
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => self.close(cx),
            Err(error) => {
                self.file_error = Some(error.to_string());
                cx.notify();
            }
        }
    }

    /// Cancel the current recording
    fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        self.recording_generation += 1;
        self.editing = None;
        self._chord_timer = None;
        self._interceptor = None;
        self.pending_conflict = None;
        cx.notify();
    }

    /// Handle a keystroke during recording
    fn handle_recorded_keystroke(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editing) = self.editing.as_mut() else {
            return;
        };

        // Ignore modifier-only keypresses
        let key = keystroke.key.as_str();
        if matches!(
            key,
            "shift" | "control" | "alt" | "platform" | "function" | ""
        ) {
            return;
        }

        let config_str = keystroke_to_config_string(keystroke);
        if !KeybindingEntry::new(&config_str, None).is_supported_on(ShortcutPlatform::current()) {
            self._chord_timer = None;
            self.recording_generation += 1;
            if let Some(editing) = self.editing.as_mut() {
                editing.first_chord = None;
                editing.waiting_for_chord = false;
            }
            self.file_error =
                Some("The Win key is reserved by Windows. Choose a shortcut without Win.".into());
            cx.notify();
            return;
        }
        self.file_error = None;

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

            let generation = self.recording_generation;
            cx.spawn_in(window, async move |this, cx| {
                let timeout = smol::Timer::after(std::time::Duration::from_secs(1));
                let timed_out = smol::future::or(
                    async {
                        timeout.await;
                        true
                    },
                    async {
                        let _ = cancel_rx.recv().await;
                        false
                    },
                )
                .await;

                if !timed_out {
                    return;
                }
                // Finalize only the recording that created this timer.
                let _ = cx.update(|window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        if this.recording_generation == generation
                            && let Some(editing) = this.editing.as_mut()
                            && editing.waiting_for_chord
                            && let Some(single) = editing.first_chord.take()
                        {
                            this.finalize_recording(single, window, cx);
                        }
                    });
                });
            })
            .detach();

            cx.notify();
        }
    }

    /// Finalize recording: save the new keystroke and check for conflicts
    fn finalize_recording(
        &mut self,
        new_keystroke: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_save(cx) {
            self.cancel_recording(cx);
            return;
        }
        let Some(editing) = self.editing.take() else {
            return;
        };
        self._chord_timer = None;
        self._interceptor = None;

        // Update the config
        update_config(|config| {
            if let Some(context) = editing.draft_context {
                let mut entry = KeybindingEntry::new(new_keystroke.clone(), context.as_deref());
                entry.platforms = Some(vec![ShortcutPlatform::current()]);
                config.add_binding(&editing.action, entry);
            } else {
                config.update_binding(&editing.action, editing.entry_index, new_keystroke.clone());
            }
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
        self.save_changes(cx);

        cx.notify();
    }

    /// Add a new empty binding for an action
    fn add_binding_for_action(
        &mut self,
        action: &str,
        context: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if crate::keybindings::create_keybinding(action, "ctrl-f12", context.as_deref()).is_none()
            || !self.can_save(cx)
        {
            return;
        }
        let new_index = get_config().bindings.get(action).map_or(0, Vec::len);
        self.start_recording(action.to_string(), new_index, cx);
        if let Some(editing) = self.editing.as_mut() {
            editing.draft_context = Some(context);
        }
    }

    /// Remove a binding entry
    fn remove_binding_entry(&mut self, action: &str, entry_index: usize, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        update_config(|config| {
            config.remove_binding(action, entry_index);
        });

        self.save_changes(cx);

        cx.notify();
    }

    /// Toggle enabled/disabled state
    fn toggle_binding_entry(&mut self, action: &str, entry_index: usize, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        update_config(|config| {
            config.toggle_binding(action, entry_index);
        });

        self.save_changes(cx);

        cx.notify();
    }

    /// Reset a single action to defaults
    fn reset_single_action(&mut self, action: &str, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        update_config(|config| {
            config.reset_single_action(action);
        });

        self.save_changes(cx);

        cx.notify();
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let capture = self.capture.is_some();
        let assigned = self.assigned_only;
        let platforms = self.all_platforms;
        let button_text_size = ui_text_ms(cx);
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .cursor_pointer()
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(5.0))
                .border_1()
                .border_color(rgb(t.border))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .text_size(button_text_size)
                .text_color(rgb(t.text_secondary))
                .child(label)
        };
        v_flex()
            .gap(px(8.0))
            .px(px(16.0))
            .py(px(10.0))
            .flex_shrink_0()
            .child(
                h_flex()
                    .gap(px(8.0))
                    .child(
                        button("edit-keybindings-json", "Edit in keybindings.json").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.open_json(cx)),
                        ),
                    )
                    .child(
                        button("reload-keybindings-json", "Reload from disk").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.reload_from_disk(cx)),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.0))
                    .child(
                        button(
                            "shortcut-search-mode",
                            if capture {
                                "Text search"
                            } else {
                                "Record shortcut"
                            },
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.toggle_search(window, cx)),
                        ),
                    )
                    .child(self.render_filter(
                        0,
                        if assigned {
                            "Assigned commands"
                        } else {
                            "All commands"
                        },
                        cx,
                    ))
                    .child(self.render_filter(
                        1,
                        if platforms {
                            "All platforms"
                        } else {
                            "Current OS only"
                        },
                        cx,
                    ))
                    .when(capture, |d| {
                        d.child(button("reset-shortcut-query", "Clear").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.capture = Some(CaptureQuery::default());
                                this.held_keys.clear();
                                cx.notify();
                            }),
                        ))
                    }),
            )
    }

    fn render_filter(
        &self,
        index: usize,
        label: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let this = cx.entity().downgrade();
        dropdown_button(
            if index == 0 {
                "shortcut-assignment-filter"
            } else {
                "shortcut-platform-filter"
            },
            label,
            self.filter_open == Some(index),
            &t,
            cx,
            move |bounds, _, cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |this, _| this.filter_bounds[index] = Some(bounds));
                }
            },
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.filter_open = if this.filter_open == Some(index) {
                    None
                } else {
                    Some(index)
                };
                cx.stop_propagation();
                cx.notify();
            }),
        )
    }

    fn render_filter_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let index = self.filter_open?;
        let bounds = self.filter_bounds[index]?;
        let t = theme(cx);
        let options = if index == 0 {
            ["All commands", "Assigned commands"]
        } else {
            ["Current OS only", "All platforms"]
        };
        let current = if index == 0 {
            self.assigned_only
        } else {
            self.all_platforms
        };
        Some(dropdown_anchored_below(
            bounds,
            dropdown_overlay("shortcut-filter-popup", &t)
                .min_w(bounds.size.width)
                .children(options.into_iter().enumerate().map(|(option, label)| {
                    let value = option == 1;
                    dropdown_option(
                        format!("shortcut-filter-option-{index}-{option}"),
                        label,
                        current == value,
                        &t,
                        cx,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if index == 0 {
                                this.assigned_only = value;
                            } else if this.all_platforms != value {
                                this.all_platforms = value;
                                if let Some(q) = this.capture.as_mut() {
                                    *q = CaptureQuery::default();
                                }
                            }
                            this.filter_open = None;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                })),
        ))
    }

    fn render_category(
        &self,
        category: &str,
        bindings: &[ActionBindings],
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let descriptions = get_action_descriptions();
        let category_string = category.to_string();
        let customized = get_config().get_customized_actions();

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
                    .children(bindings.iter().enumerate().map(|(i, (action, entries))| {
                        let description = descriptions
                            .get(action.as_str())
                            .map(|d| d.description)
                            .unwrap_or("Unknown action");
                        let name = descriptions
                            .get(action.as_str())
                            .map(|d| d.name)
                            .unwrap_or(action.as_str());

                        let is_action_customized = customized.contains(action);
                        let action_clone = action.clone();
                        let action_for_reset = action.clone();
                        // Determine context from first entry for "add" button
                        let entry_context = search::default_context(action, &get_config());
                        let can_bind =
                            crate::keybindings::create_keybinding(action, "ctrl-f12", None)
                                .is_some();

                        div()
                            .when(i > 0, |d| d.border_t_1().border_color(rgb(t.border)))
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
                                                    .id(SharedString::from(format!(
                                                        "add-{}",
                                                        action_clone
                                                    )))
                                                    .cursor_pointer()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(3.0))
                                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                                    .text_size(ui_text_sm(cx))
                                                    .text_color(rgb(t.text_muted))
                                                    .child(if can_bind {
                                                        "+"
                                                    } else {
                                                        "Unavailable"
                                                    })
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let action = action_clone.clone();
                                                        let ctx = entry_context.clone();
                                                        cx.listener(move |this, _, _window, cx| {
                                                            this.add_binding_for_action(
                                                                &action,
                                                                ctx.clone(),
                                                                cx,
                                                            );
                                                        })
                                                    }),
                                            )
                                            // Reset single action button (only if customized)
                                            .when(is_action_customized, |d| {
                                                d.child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "reset-{}",
                                                            action_for_reset
                                                        )))
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
                                                            cx.listener(
                                                                move |this, _, _window, cx| {
                                                                    this.reset_single_action(
                                                                        &action, cx,
                                                                    );
                                                                },
                                                            )
                                                        }),
                                                )
                                            }),
                                    ),
                            )
                            .when(
                                self.editing.as_ref().is_some_and(|e| {
                                    e.action == *action && e.draft_context.is_some()
                                }),
                                |d| {
                                    d.child(
                                        div().px(px(24.0)).py(px(4.0)).child(
                                            self.editing
                                                .as_ref()
                                                .and_then(|e| e.first_chord.as_ref())
                                                .map(|s| {
                                                    format!(
                                                        "{} — Waiting for next key…",
                                                        format_keystroke(s)
                                                    )
                                                })
                                                .unwrap_or_else(|| {
                                                    "Press keys to add a binding, Esc to cancel"
                                                        .into()
                                                }),
                                        ),
                                    )
                                },
                            )
                            // Individual binding entries
                            .children(entries.iter().map(
                                |(keystroke, entry_idx, _is_customized, is_enabled)| {
                                    let action_name = action.clone();
                                    let action_for_toggle = action.clone();
                                    let action_for_remove = action.clone();
                                    let idx = *entry_idx;
                                    let enabled = *is_enabled;
                                    let ks = keystroke.clone();
                                    let platform = get_config().bindings.get(action).and_then(|v| v.get(idx))
                                        .map(|entry| search::display_platform(entry, ShortcutPlatform::current()))
                                        .unwrap_or_else(ShortcutPlatform::current);

                                    let reserved = get_config().bindings.get(action).and_then(|v| v.get(idx))
                                        .is_some_and(|entry| entry.applies_to(ShortcutPlatform::current()) && !entry.is_supported_on(ShortcutPlatform::current()));
                                    let mut binding_context = get_config()
                                        .bindings
                                        .get(action)
                                        .and_then(|v| v.get(idx))
                                        .and_then(|e| e.context.clone())
                                        .unwrap_or_else(|| "Global".into());
                                    if reserved { binding_context.push_str(" · Win reserved (inactive)"); }

                                    // Check if this entry is currently being recorded
                                    let is_recording = self.editing.as_ref().is_some_and(|e| {
                                        e.action == action_name && e.entry_index == idx
                                    });
                                    let is_waiting_chord = is_recording
                                        && self
                                            .editing
                                            .as_ref()
                                            .is_some_and(|e| e.waiting_for_chord);

                                    h_flex()
                                        .justify_between()
                                        .px(px(12.0))
                                        .pl(px(24.0))
                                        .py(px(4.0))
                                        .child(
                                            // Keystroke badge (clickable to record)
                                            div()
                                                .id(SharedString::from(format!(
                                                    "ks-{}-{}",
                                                    action_name, idx
                                                )))
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
                                                .text_color(if is_recording {
                                                    rgb(0xFFFFFF)
                                                } else {
                                                    rgb(t.text_secondary)
                                                })
                                                .child(if is_recording {
                                                    if is_waiting_chord {
                                                        let first = self
                                                            .editing
                                                            .as_ref()
                                                            .and_then(|e| e.first_chord.as_ref())
                                                            .map(|s| format_keystroke(s))
                                                            .unwrap_or_default();
                                                        format!("{} ...", first)
                                                    } else {
                                                        "Press keys...".to_string()
                                                    }
                                                } else {
                                                    okena_ui::shortcuts::format_keystroke_for_platform(&ks, platform)
                                                })
                                                .on_mouse_down(MouseButton::Left, {
                                                    let action = action_name.clone();
                                                    cx.listener(move |this, _, _window, cx| {
                                                        if this.editing.as_ref().is_some_and(|e| {
                                                            e.action == action
                                                                && e.entry_index == idx
                                                        }) {
                                                            this.cancel_recording(cx);
                                                        } else {
                                                            this.start_recording(
                                                                action.clone(),
                                                                idx,
                                                                cx,
                                                            );
                                                        }
                                                    })
                                                }),
                                        )
                                        .child(
                                            h_flex()
                                                .gap(px(4.0))
                                                .child(
                                                    div()
                                                        .text_size(ui_text_sm(cx))
                                                        .text_color(rgb(t.text_muted))
                                                        .child(binding_context),
                                                )
                                                // Toggle enabled/disabled
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "toggle-{}-{}",
                                                            action_for_toggle, idx
                                                        )))
                                                        .cursor_pointer()
                                                        .px(px(6.0))
                                                        .py(px(2.0))
                                                        .rounded(px(3.0))
                                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                                        .text_size(ui_text_sm(cx))
                                                        .text_color(if enabled {
                                                            rgb(t.text_muted)
                                                        } else {
                                                            rgb(t.error)
                                                        })
                                                        .child(if enabled { "●" } else { "○" })
                                                        .on_mouse_down(MouseButton::Left, {
                                                            let action = action_for_toggle.clone();
                                                            cx.listener(
                                                                move |this, _, _window, cx| {
                                                                    this.toggle_binding_entry(
                                                                        &action, idx, cx,
                                                                    );
                                                                },
                                                            )
                                                        }),
                                                )
                                                // Remove binding
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "rm-{}-{}",
                                                            action_for_remove, idx
                                                        )))
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
                                                            cx.listener(
                                                                move |this, _, _window, cx| {
                                                                    this.remove_binding_entry(
                                                                        &action, idx, cx,
                                                                    );
                                                                },
                                                            )
                                                        }),
                                                ),
                                        )
                                },
                            ))
                    })),
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

        self.capture_window = Some(window.window_handle());
        if self.activation_subscription.is_none() {
            self.activation_subscription =
                Some(cx.observe_window_activation(window, |this, _, _| {
                    this.held_keys.clear();
                }));
        }
        if self.capture.is_none() && self.editing.is_none() {
            focus_input(&self.search_input, window, cx);
        }

        let config = get_config();
        let conflicts = config.detect_conflicts();

        let query = self.search_input.read(cx).value().to_lowercase();
        let mut rows = build_rows(
            &config,
            &query,
            self.capture.as_ref(),
            self.assigned_only,
            self.all_platforms,
        );
        // Keep recording rows visible even when their eventual shortcut is filtered out.
        if let Some(editing) = &self.editing
            && !rows
                .iter()
                .any(|(_, (action, _))| action == &editing.action)
        {
            let descriptions = get_action_descriptions();
            let category = descriptions
                .get(editing.action.as_str())
                .map_or("Other", |d| d.category);
            rows.push((category, (editing.action.clone(), Vec::new())));
        }
        let empty_results = rows.is_empty();
        let mut categories: std::collections::HashMap<&str, Vec<ActionBindings>> =
            Default::default();
        for (category, row) in rows {
            categories.entry(category).or_default().push(row);
        }
        let category_order = [
            "Global",
            "Terminal",
            "Navigation",
            "Search",
            "Fullscreen",
            "Layout",
            "Project",
            "Services",
            "Git",
            "Other",
        ];

        let focus_handle = self.focus_handle.clone();
        let is_editing = self.editing.is_some();

        modal_backdrop("keybindings-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("KeybindingsHelp")
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                this.held_keys
                    .remove(&search::held_key(&event.keystroke, cx));
            }))
            .items_center()
            .on_action(cx.listener(|this, _: &ShowKeybindings, _window, cx| {
                this.close(cx);
            }))
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                if this.filter_open.take().is_some() {
                    cx.notify();
                } else if this.editing.is_some() {
                    this.cancel_recording(cx);
                } else if !this.search_input.read(cx).value().is_empty() {
                    this.search_input
                        .update(cx, |input, cx| input.set_value("", cx));
                } else {
                    this.close(cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    if this.filter_open.take().is_some() {
                        cx.notify();
                    } else if this.editing.is_some() {
                        this.cancel_recording(cx);
                    } else {
                        this.close(cx);
                    }
                }),
            )
            .child(
                modal_content("keybindings-modal", &t)
                    .w(px(650.0))
                    .max_h(px(700.0))
                    // Stop click propagation on modal content
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.filter_open.take().is_some() {
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .child(modal_header(
                        "Keyboard Shortcuts",
                        Some(if is_editing {
                            "Press keys to record, ESC to cancel"
                        } else {
                            "Click a binding to change it"
                        }),
                        &t,
                        cx,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(self.render_controls(cx))
                    .when(self.capture.is_none(), |d| {
                        d.child(search_input_row(&self.search_input, &t, cx))
                    })
                    .when(self.capture.is_some(), |d| {
                        d.child(
                            div().px(px(16.0)).py(px(10.0)).child(
                                self.capture.as_ref().map(|q| q.label()).unwrap_or_default(),
                            ),
                        )
                    })
                    .when(self.file_error.is_some(), |d| {
                        d.child(
                            div()
                                .px(px(16.0))
                                .text_color(rgb(t.error))
                                .child(self.file_error.clone().unwrap_or_default()),
                        )
                    })
                    // Conflict/info banners
                    .child(div().when(!conflicts.is_empty(), |d| {
                        d.px(px(16.0))
                            .py(px(8.0))
                            .bg(rgb(t.warning))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .child(div().text_size(ui_text_xl(cx)).child("⚠️"))
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
                    }))
                    .child(
                        // Scrollable content
                        div()
                            .id("keybindings-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .px(px(16.0))
                            .py(px(12.0))
                            .when(empty_results, |d| {
                                d.child(div().py(px(20.0)).child("No matching commands"))
                            })
                            .children(category_order.iter().filter_map(|category| {
                                categories
                                    .get(category)
                                    .map(|bindings| self.render_category(category, bindings, cx))
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
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _window, cx| {
                                                            this.handle_reset_to_defaults(cx);
                                                        }),
                                                    ),
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
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _window, cx| {
                                                            this.cancel_reset(cx);
                                                        }),
                                                    ),
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
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _window, cx| {
                                                        this.handle_reset_to_defaults(cx);
                                                    }),
                                                ),
                                        )
                                    }),
                            ),
                    ),
            )
            .children(self.render_filter_popup(cx))
    }
}

impl_focusable!(KeybindingsHelp);

#[cfg(test)]
mod capture_tests {
    use super::{KeybindingsHelp, ShowKeybindings, get_config, search};
    use gpui::prelude::*;
    use gpui::{
        Context, Entity, IntoElement, KeyBinding, KeyUpEvent, Keystroke, PlatformInput, Render,
        TestAppContext, Window, div, px, size,
    };

    struct Harness {
        dialog: Entity<KeybindingsHelp>,
        actions: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let focus = self.dialog.read(cx).focus_handle.clone();
            let dialog = self.dialog.clone();
            let actions = self.actions.clone();
            div()
                .track_focus(&focus)
                .key_context("KeybindingsHelp")
                .on_action(move |_: &ShowKeybindings, _, _| actions.set(actions.get() + 1))
                .on_key_up(move |event: &KeyUpEvent, _, cx| {
                    dialog.update(cx, |this, cx| {
                        this.held_keys
                            .remove(&search::held_key(&event.keystroke, cx));
                    });
                })
        }
    }

    #[cfg(target_os = "windows")]
    #[gpui::test]
    fn windows_recorder_rejects_win_without_saving(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(650.), px(700.)), |_, cx| Harness {
            dialog: cx.new(|cx| KeybindingsHelp::with_snapshot(cx, None, None)),
            actions: Default::default(),
        });
        window
            .update(cx, |this, window, cx| {
                this.dialog.update(cx, |dialog, cx| {
                    dialog.start_recording("Quit".into(), 0, cx);
                    dialog.handle_recorded_keystroke(
                        &Keystroke::parse("win-alt-left").unwrap(),
                        window,
                        cx,
                    );
                    assert!(
                        dialog
                            .file_error
                            .as_ref()
                            .unwrap()
                            .contains("Win key is reserved")
                    );
                    assert!(dialog.editing.as_ref().unwrap().first_chord.is_none());
                    assert!(dialog.disk_snapshot.is_none());
                    let editing = dialog.editing.as_mut().unwrap();
                    editing.first_chord = Some("ctrl-k".into());
                    editing.waiting_for_chord = true;
                    let generation = dialog.recording_generation;
                    dialog.handle_recorded_keystroke(
                        &Keystroke::parse("win-s").unwrap(),
                        window,
                        cx,
                    );
                    assert!(dialog.editing.as_ref().unwrap().first_chord.is_none());
                    assert!(!dialog.editing.as_ref().unwrap().waiting_for_chord);
                    assert!(dialog.recording_generation > generation);

                    dialog.cancel_recording(cx);
                });
            })
            .unwrap();
    }

    #[gpui::test]
    fn keybindings_capture_is_window_scoped_repeat_safe_and_never_dispatches(
        cx: &mut TestAppContext,
    ) {
        crate::keybindings::set_test_keybinding_config(
            crate::keybindings::KeybindingConfig::defaults(),
        );
        let before = serde_json::to_string(&*get_config()).unwrap();
        let window = cx.open_window(size(px(650.), px(700.)), |_, cx| Harness {
            dialog: cx.new(|cx| KeybindingsHelp::with_snapshot(cx, None, None)),
            actions: Default::default(),
        });
        let other = cx.open_window(size(px(100.), px(100.)), |_, cx| Harness {
            dialog: cx.new(|cx| KeybindingsHelp::with_snapshot(cx, None, None)),
            actions: Default::default(),
        });
        cx.update(|cx| cx.bind_keys([KeyBinding::new("ctrl-k ctrl-s", ShowKeybindings, None)]));
        let dialog = window.update(cx, |this, _, _| this.dialog.clone()).unwrap();
        let other_dialog = other.update(cx, |this, _, _| this.dialog.clone()).unwrap();
        let actions = window
            .update(cx, |this, _, _| this.actions.clone())
            .unwrap();
        let other_actions = other.update(cx, |this, _, _| this.actions.clone()).unwrap();
        let capture_window: gpui::AnyWindowHandle = window.into();
        let other_window: gpui::AnyWindowHandle = other.into();
        capture_window
            .update(cx, |_, window, cx| {
                dialog.update(cx, |dialog, cx| dialog.toggle_search(window, cx));
            })
            .unwrap();
        other_window
            .update(cx, |_, window, cx| {
                other_dialog.read(cx).focus_handle.clone().focus(window, cx);
                window.dispatch_keystroke(Keystroke::parse("ctrl-k").unwrap(), cx);
                window.dispatch_keystroke(Keystroke::parse("ctrl-s").unwrap(), cx);
            })
            .unwrap();
        assert_eq!(other_actions.get(), 1);
        capture_window
            .update(cx, |_, window, cx| {
                dialog.read(cx).focus_handle.clone().focus(window, cx);
                window.dispatch_keystroke(Keystroke::parse("ctrl-k").unwrap(), cx);
                window.dispatch_keystroke(Keystroke::parse("ctrl-k").unwrap(), cx);
                assert!(
                    dialog
                        .read(cx)
                        .capture
                        .as_ref()
                        .unwrap()
                        .label()
                        .contains("Waiting")
                );
                window.dispatch_event(
                    PlatformInput::KeyUp(KeyUpEvent {
                        keystroke: Keystroke::parse("ctrl-k").unwrap(),
                    }),
                    cx,
                );
                window.dispatch_keystroke(Keystroke::parse("ctrl-s").unwrap(), cx);
                assert!(
                    !dialog
                        .read(cx)
                        .capture
                        .as_ref()
                        .unwrap()
                        .label()
                        .contains("Waiting")
                );
                assert_eq!(actions.get(), 0);
                dialog.update(cx, |dialog, cx| dialog.close(cx));
            })
            .unwrap();
        assert_eq!(serde_json::to_string(&*get_config()).unwrap(), before);
        capture_window
            .update(cx, |_, window, cx| {
                window.dispatch_keystroke(Keystroke::parse("ctrl-k").unwrap(), cx);
                window.dispatch_keystroke(Keystroke::parse("ctrl-s").unwrap(), cx);
                assert!(dialog.read(cx)._interceptor.is_none());
            })
            .unwrap();
        assert_eq!(actions.get(), 1);
    }
}
