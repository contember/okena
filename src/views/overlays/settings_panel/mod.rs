//! Settings panel for visual settings configuration
//!
//! Provides a Zed-style settings dialog with sidebar categories, project selector,
//! and hooks configuration.

mod categories;
mod components;
mod controls;
mod footer;
mod header;
mod render_font;
mod render_general;
mod render_hooks;
mod render_terminal;
mod sidebar;

use categories::SettingsCategory;
use components::opt_string;

use crate::keybindings::Cancel;
use crate::settings::settings_entity;
use crate::terminal::shell_config::{available_shells, AvailableShell};
use crate::theme::theme;
use crate::views::components::{modal_backdrop, modal_content};
use crate::views::components::simple_input::{InputChangedEvent, SimpleInputState};
use crate::workspace::state::Workspace;
use gpui::*;
use gpui::prelude::*;

// ============================================================================
// Settings Panel
// ============================================================================

/// Settings panel overlay for configuring app settings
pub struct SettingsPanel {
    pub(super) workspace: Entity<Workspace>,
    focus_handle: FocusHandle,
    pub(super) active_category: SettingsCategory,
    /// None = "User" (global settings), Some(id) = per-project
    pub(super) selected_project_id: Option<String>,
    pub(super) project_dropdown_open: bool,
    pub(super) font_dropdown_open: bool,
    pub(super) shell_dropdown_open: bool,
    pub(super) session_backend_dropdown_open: bool,
    pub(super) available_shells: Vec<AvailableShell>,
    // Global hook inputs
    pub(super) hook_project_open: Entity<SimpleInputState>,
    pub(super) hook_project_close: Entity<SimpleInputState>,
    pub(super) hook_worktree_create: Entity<SimpleInputState>,
    pub(super) hook_worktree_close: Entity<SimpleInputState>,
    // Per-project hook inputs
    pub(super) project_hook_project_open: Entity<SimpleInputState>,
    pub(super) project_hook_project_close: Entity<SimpleInputState>,
    pub(super) project_hook_worktree_create: Entity<SimpleInputState>,
    pub(super) project_hook_worktree_close: Entity<SimpleInputState>,
    // File opener input
    pub(super) file_opener_input: Entity<SimpleInputState>,
    // Remote listen address input
    pub(super) listen_address_input: Entity<SimpleInputState>,
}

impl SettingsPanel {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self::new_with_options(workspace, None, None, cx)
    }

    pub fn new_for_project(
        workspace: Entity<Workspace>,
        project_id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_options(workspace, Some(project_id), Some(SettingsCategory::Hooks), cx)
    }

    fn new_with_options(
        workspace: Entity<Workspace>,
        project_id: Option<String>,
        category: Option<SettingsCategory>,
        cx: &mut Context<Self>,
    ) -> Self {
        let s = settings_entity(cx).read(cx).settings.clone();

        // Create global hook inputs
        let hook_project_open = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. echo \"opened $TERM_MANAGER_PROJECT_NAME\"");
            match s.hooks.on_project_open { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_project_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. echo \"closed $TERM_MANAGER_PROJECT_NAME\"");
            match s.hooks.on_project_close { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_worktree_create = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. npm install");
            match s.hooks.on_worktree_create { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_worktree_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. cleanup script");
            match s.hooks.on_worktree_close { Some(ref v) => state.default_value(v.clone()), None => state }
        });

        // Subscribe to global hook input changes
        cx.subscribe(&hook_project_open, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_on_project_open(val, cx));
        }).detach();
        cx.subscribe(&hook_project_close, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_on_project_close(val, cx));
        }).detach();
        cx.subscribe(&hook_worktree_create, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_on_worktree_create(val, cx));
        }).detach();
        cx.subscribe(&hook_worktree_close, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_on_worktree_close(val, cx));
        }).detach();

        // Create per-project hook inputs (initialized for selected project)
        let project_hooks = project_id.as_ref().and_then(|pid| {
            workspace.read(cx).project(pid).map(|p| p.hooks.clone())
        });
        let global_hooks = &s.hooks;

        let project_hook_project_open = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder(global_hooks.on_project_open.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.on_project_open.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_project_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder(global_hooks.on_project_close.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.on_project_close.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_worktree_create = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder(global_hooks.on_worktree_create.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.on_worktree_create.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_worktree_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder(global_hooks.on_worktree_close.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.on_worktree_close.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });

        // Subscribe to per-project hook input changes
        let ws = workspace.clone();
        cx.subscribe(&project_hook_project_open, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.on_project_open = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_project_close, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.on_project_close = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_worktree_create, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.on_worktree_create = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_worktree_close, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.on_worktree_close = val; true });
                    });
                }
            }
        }).detach();

        // File opener input
        let file_opener_input = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. code, cursor, zed, vim");
            if !s.file_opener.is_empty() { state.default_value(s.file_opener.clone()) } else { state }
        });
        cx.subscribe(&file_opener_input, |_this, entity, _: &InputChangedEvent, cx| {
            let val = entity.read(cx).value().to_string();
            settings_entity(cx).update(cx, |state, cx| state.set_file_opener(val, cx));
        }).detach();

        // Remote listen address input
        let listen_address_input = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .placeholder("e.g. 127.0.0.1, 0.0.0.0");
            if !s.remote_listen_address.is_empty() { state.default_value(s.remote_listen_address.clone()) } else { state }
        });
        cx.subscribe(&listen_address_input, |_this, entity, _: &InputChangedEvent, cx| {
            let val = entity.read(cx).value().to_string();
            settings_entity(cx).update(cx, |state, cx| state.set_remote_listen_address(val, cx));
        }).detach();

        Self {
            workspace,
            focus_handle: cx.focus_handle(),
            active_category: category.unwrap_or(SettingsCategory::General),
            selected_project_id: project_id,
            project_dropdown_open: false,
            font_dropdown_open: false,
            shell_dropdown_open: false,
            session_backend_dropdown_open: false,
            available_shells: available_shells(),
            hook_project_open,
            hook_project_close,
            hook_worktree_create,
            hook_worktree_close,
            project_hook_project_open,
            project_hook_project_close,
            project_hook_worktree_create,
            project_hook_worktree_close,
            file_opener_input,
            listen_address_input,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(SettingsPanelEvent::Close);
    }

    pub(super) fn close_all_dropdowns(&mut self) {
        self.font_dropdown_open = false;
        self.shell_dropdown_open = false;
        self.session_backend_dropdown_open = false;
        self.project_dropdown_open = false;
    }

    fn has_open_dropdown(&self) -> bool {
        self.font_dropdown_open || self.shell_dropdown_open || self.session_backend_dropdown_open || self.project_dropdown_open
    }

    /// Switch to a different project (or "User" if None)
    pub(super) fn select_project(&mut self, project_id: Option<String>, cx: &mut Context<Self>) {
        self.selected_project_id = project_id.clone();
        self.project_dropdown_open = false;

        // When switching to project mode, ensure Hooks is selected
        if project_id.is_some() {
            let available = SettingsCategory::project_categories();
            if !available.contains(&self.active_category) {
                self.active_category = SettingsCategory::Hooks;
            }
        }

        // Reload project hook inputs for the new project
        self.reload_project_hook_inputs(cx);
        cx.notify();
    }

    /// Reload per-project hook inputs with values from the selected project
    fn reload_project_hook_inputs(&mut self, cx: &mut Context<Self>) {
        let global_hooks = settings_entity(cx).read(cx).settings.hooks.clone();
        let project_hooks = self.selected_project_id.as_ref().and_then(|pid| {
            self.workspace.read(cx).project(pid).map(|p| p.hooks.clone())
        });

        // Update placeholders and values
        self.project_hook_project_open.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.on_project_open.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.on_project_open.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_project_close.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.on_project_close.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.on_project_close.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_worktree_create.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.on_worktree_create.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.on_worktree_create.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_worktree_close.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.on_worktree_close.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.on_worktree_close.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
    }

    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings-content")
            .flex_1()
            .overflow_y_scroll()
            .min_w_0()
            .child(match self.active_category {
                SettingsCategory::General => self.render_general(cx).into_any_element(),
                SettingsCategory::Font => self.render_font(cx).into_any_element(),
                SettingsCategory::Terminal => self.render_terminal(cx).into_any_element(),
                SettingsCategory::Hooks => self.render_hooks(cx).into_any_element(),
            })
    }
}

pub enum SettingsPanelEvent {
    Close,
}

impl EventEmitter<SettingsPanelEvent> for SettingsPanel {}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        if !focus_handle.contains_focused(window, cx) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("settings-panel-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("SettingsPanel")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                if this.has_open_dropdown() {
                    this.close_all_dropdowns();
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.has_open_dropdown() {
                    this.close_all_dropdowns();
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }))
            .child(
                modal_content("settings-panel-modal", &t)
                    .relative()
                    .w(px(620.0))
                    .max_h(px(560.0))
                    // Header with project selector and edit button
                    .child(self.render_header(cx))
                    // Main body: sidebar + content
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.render_sidebar(cx))
                            .child(self.render_content(cx)),
                    )
                    // Footer
                    .child(self.render_footer(cx))
                    // Dropdown overlays (rendered last to be on top)
                    .when(self.project_dropdown_open, |modal| {
                        modal.child(self.render_project_dropdown_overlay(cx))
                    })
                    .when(self.font_dropdown_open, |modal| {
                        let settings = settings_entity(cx);
                        let current = settings.read(cx).settings.font_family.clone();
                        modal.child(self.render_font_dropdown_overlay(&current, cx))
                    })
                    .when(self.shell_dropdown_open, |modal| {
                        let settings = settings_entity(cx);
                        let current = settings.read(cx).settings.default_shell.clone();
                        modal.child(self.render_shell_dropdown_overlay(&current, cx))
                    })
                    .when(self.session_backend_dropdown_open, |modal| {
                        let settings = settings_entity(cx);
                        let current = settings.read(cx).settings.session_backend;
                        modal.child(self.render_session_backend_dropdown_overlay(&current, cx))
                    }),
            )
    }
}

impl_focusable!(SettingsPanel);
