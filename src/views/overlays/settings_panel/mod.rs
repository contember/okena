//! Settings panel for visual settings configuration
//!
//! Provides a Zed-style settings dialog with sidebar categories, project selector,
//! and hooks configuration.

mod categories;
mod components;
mod controls;
mod footer;
mod header;
mod render_extensions;
mod render_font;
mod render_general;
mod render_hooks;
mod render_paired_devices;
mod render_terminal;
mod render_worktree;
mod sidebar;

use categories::SettingsCategory;
use components::opt_string;

use crate::keybindings::Cancel;
use crate::remote::auth::{AuthStore, TokenInfo};
use crate::remote::GlobalRemoteInfo;
use crate::settings::settings_entity;
use crate::terminal::shell_config::{available_shells, AvailableShell};
use crate::theme::theme;
use crate::views::components::{dropdown_anchored_below, modal_backdrop, modal_content};
use crate::views::components::simple_input::{InputChangedEvent, SimpleInputState};
use crate::workspace::state::Workspace;
use gpui::*;
use gpui::prelude::*;
use okena_extensions::ExtensionRegistry;
use std::collections::HashMap;
use std::sync::Arc;

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
    pub(super) project_button_bounds: Option<Bounds<Pixels>>,
    pub(super) font_button_bounds: Option<Bounds<Pixels>>,
    pub(super) shell_button_bounds: Option<Bounds<Pixels>>,
    pub(super) session_backend_button_bounds: Option<Bounds<Pixels>>,
    pub(super) available_shells: Vec<AvailableShell>,
    // Global hook inputs
    pub(super) hook_project_open: Entity<SimpleInputState>,
    pub(super) hook_project_close: Entity<SimpleInputState>,
    pub(super) hook_worktree_create: Entity<SimpleInputState>,
    pub(super) hook_worktree_close: Entity<SimpleInputState>,
    // New global hook inputs
    pub(super) hook_pre_merge: Entity<SimpleInputState>,
    pub(super) hook_post_merge: Entity<SimpleInputState>,
    pub(super) hook_before_worktree_remove: Entity<SimpleInputState>,
    pub(super) hook_worktree_removed: Entity<SimpleInputState>,
    pub(super) hook_on_rebase_conflict: Entity<SimpleInputState>,
    pub(super) hook_on_dirty_worktree_close: Entity<SimpleInputState>,
    // Global terminal hook inputs
    pub(super) hook_terminal_on_create: Entity<SimpleInputState>,
    pub(super) hook_terminal_on_close: Entity<SimpleInputState>,
    pub(super) hook_terminal_shell_wrapper: Entity<SimpleInputState>,
    // Per-project hook inputs
    pub(super) project_hook_project_open: Entity<SimpleInputState>,
    pub(super) project_hook_project_close: Entity<SimpleInputState>,
    pub(super) project_hook_worktree_create: Entity<SimpleInputState>,
    pub(super) project_hook_worktree_close: Entity<SimpleInputState>,
    pub(super) project_hook_pre_merge: Entity<SimpleInputState>,
    pub(super) project_hook_post_merge: Entity<SimpleInputState>,
    pub(super) project_hook_before_worktree_remove: Entity<SimpleInputState>,
    pub(super) project_hook_worktree_removed: Entity<SimpleInputState>,
    pub(super) project_hook_on_rebase_conflict: Entity<SimpleInputState>,
    pub(super) project_hook_on_dirty_worktree_close: Entity<SimpleInputState>,
    // Per-project terminal hook inputs
    pub(super) project_hook_terminal_on_create: Entity<SimpleInputState>,
    pub(super) project_hook_terminal_on_close: Entity<SimpleInputState>,
    pub(super) project_hook_terminal_shell_wrapper: Entity<SimpleInputState>,
    // Worktree dir suffix input
    pub(super) worktree_dir_suffix_input: Entity<SimpleInputState>,
    // File opener input
    pub(super) file_opener_input: Entity<SimpleInputState>,
    // Remote listen address input
    pub(super) listen_address_input: Entity<SimpleInputState>,
    // Paired devices
    pub(super) paired_devices: Vec<TokenInfo>,
    pub(super) auth_store: Option<Arc<AuthStore>>,
    /// Cached extension settings views (lazily created on first access).
    extension_views: HashMap<String, AnyView>,
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
                .multiline()
                .placeholder("e.g. echo \"opened $OKENA_PROJECT_NAME\"");
            match s.hooks.project.on_open { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_project_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. echo \"closed $OKENA_PROJECT_NAME\"");
            match s.hooks.project.on_close { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_worktree_create = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. npm install");
            match s.hooks.worktree.on_create { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_worktree_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. cleanup script");
            match s.hooks.worktree.on_close { Some(ref v) => state.default_value(v.clone()), None => state }
        });

        // Create new global hook inputs
        let hook_pre_merge = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. run linter before merge");
            match s.hooks.worktree.pre_merge { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_post_merge = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. notify team after merge");
            match s.hooks.worktree.post_merge { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_before_worktree_remove = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. backup work before removal");
            match s.hooks.worktree.before_remove { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_worktree_removed = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. cleanup after removal");
            match s.hooks.worktree.after_remove { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_on_rebase_conflict = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. notify on rebase conflict");
            match s.hooks.worktree.on_rebase_conflict { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_on_dirty_worktree_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. backup uncommitted changes");
            match s.hooks.worktree.on_dirty_close { Some(ref v) => state.default_value(v.clone()), None => state }
        });

        // Create global terminal hook inputs
        let hook_terminal_on_create = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. echo \"terminal created\"");
            match s.hooks.terminal.on_create { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_terminal_on_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. echo \"terminal closed\"");
            match s.hooks.terminal.on_close { Some(ref v) => state.default_value(v.clone()), None => state }
        });
        let hook_terminal_shell_wrapper = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder("e.g. devcontainer exec -- {shell}");
            match s.hooks.terminal.shell_wrapper { Some(ref v) => state.default_value(v.clone()), None => state }
        });

        // Subscribe to global hook input changes
        cx.subscribe(&hook_project_open, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_project_on_open(val, cx));
        }).detach();
        cx.subscribe(&hook_project_close, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_project_on_close(val, cx));
        }).detach();
        cx.subscribe(&hook_worktree_create, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_worktree_on_create(val, cx));
        }).detach();
        cx.subscribe(&hook_worktree_close, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_worktree_on_close(val, cx));
        }).detach();
        cx.subscribe(&hook_pre_merge, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_worktree_pre_merge(val, cx));
        }).detach();
        cx.subscribe(&hook_post_merge, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_worktree_post_merge(val, cx));
        }).detach();
        cx.subscribe(&hook_before_worktree_remove, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_worktree_before_remove(val, cx));
        }).detach();
        cx.subscribe(&hook_worktree_removed, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_worktree_after_remove(val, cx));
        }).detach();
        cx.subscribe(&hook_on_rebase_conflict, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_worktree_on_rebase_conflict(val, cx));
        }).detach();
        cx.subscribe(&hook_on_dirty_worktree_close, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_worktree_on_dirty_close(val, cx));
        }).detach();
        cx.subscribe(&hook_terminal_on_create, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_terminal_on_create(val, cx));
        }).detach();
        cx.subscribe(&hook_terminal_on_close, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_terminal_on_close(val, cx));
        }).detach();
        cx.subscribe(&hook_terminal_shell_wrapper, |_this, entity, _: &InputChangedEvent, cx| {
            let val = opt_string(entity.read(cx).value());
            settings_entity(cx).update(cx, |state, cx| state.set_hook_terminal_shell_wrapper(val, cx));
        }).detach();

        // Create per-project hook inputs (initialized for selected project)
        let project_hooks = project_id.as_ref().and_then(|pid| {
            workspace.read(cx).project(pid).map(|p| p.hooks.clone())
        });
        let global_hooks = &s.hooks;

        let project_hook_project_open = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.project.on_open.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.project.on_open.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_project_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.project.on_close.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.project.on_close.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_worktree_create = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.worktree.on_create.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.worktree.on_create.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_worktree_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.worktree.on_close.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.worktree.on_close.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_pre_merge = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.worktree.pre_merge.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.worktree.pre_merge.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_post_merge = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.worktree.post_merge.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.worktree.post_merge.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_before_worktree_remove = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.worktree.before_remove.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.worktree.before_remove.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_worktree_removed = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.worktree.after_remove.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.worktree.after_remove.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_on_rebase_conflict = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.worktree.on_rebase_conflict.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.worktree.on_rebase_conflict.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_on_dirty_worktree_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.worktree.on_dirty_close.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.worktree.on_dirty_close.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_terminal_on_create = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.terminal.on_create.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.terminal.on_create.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_terminal_on_close = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.terminal.on_close.as_deref().unwrap_or("No global hook set"));
            match project_hooks.as_ref().and_then(|h| h.terminal.on_close.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
        });
        let project_hook_terminal_shell_wrapper = cx.new(|cx| {
            let state = SimpleInputState::new(cx)
                .multiline()
                .placeholder(global_hooks.terminal.shell_wrapper.as_deref().unwrap_or("e.g. devcontainer exec -- {shell}"));
            match project_hooks.as_ref().and_then(|h| h.terminal.shell_wrapper.as_ref()) { Some(v) => state.default_value(v.clone()), None => state }
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
                        ws.with_project(&pid, cx, |p| { p.hooks.project.on_open = val; true });
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
                        ws.with_project(&pid, cx, |p| { p.hooks.project.on_close = val; true });
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
                        ws.with_project(&pid, cx, |p| { p.hooks.worktree.on_create = val; true });
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
                        ws.with_project(&pid, cx, |p| { p.hooks.worktree.on_close = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_pre_merge, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.worktree.pre_merge = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_post_merge, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.worktree.post_merge = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_before_worktree_remove, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.worktree.before_remove = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_worktree_removed, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.worktree.after_remove = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_on_rebase_conflict, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.worktree.on_rebase_conflict = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_on_dirty_worktree_close, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.worktree.on_dirty_close = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_terminal_on_create, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.terminal.on_create = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_terminal_on_close, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.terminal.on_close = val; true });
                    });
                }
            }
        }).detach();
        cx.subscribe(&project_hook_terminal_shell_wrapper, {
            let ws = ws.clone();
            move |this, entity, _: &InputChangedEvent, cx| {
                if let Some(ref pid) = this.selected_project_id {
                    let val = opt_string(entity.read(cx).value());
                    let pid = pid.clone();
                    ws.update(cx, |ws, cx| {
                        ws.with_project(&pid, cx, |p| { p.hooks.terminal.shell_wrapper = val; true });
                    });
                }
            }
        }).detach();

        // Worktree path template input
        let worktree_dir_suffix_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("../{repo}-wt/{branch}")
                .highlight_vars()
                .default_value(s.worktree.path_template.clone())
        });
        cx.subscribe(&worktree_dir_suffix_input, |_this, entity, _: &InputChangedEvent, cx| {
            let val = entity.read(cx).value().to_string();
            settings_entity(cx).update(cx, |state, cx| state.set_worktree_path_template(val, cx));
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

        let (auth_store, paired_devices) = cx
            .try_global::<GlobalRemoteInfo>()
            .and_then(|info| info.0.auth_store())
            .map(|store| {
                let tokens = store.list_tokens();
                (Some(store), tokens)
            })
            .unwrap_or((None, Vec::new()));

        Self {
            workspace,
            focus_handle: cx.focus_handle(),
            active_category: category.unwrap_or(SettingsCategory::General),
            selected_project_id: project_id,
            project_dropdown_open: false,
            font_dropdown_open: false,
            shell_dropdown_open: false,
            session_backend_dropdown_open: false,
            project_button_bounds: None,
            font_button_bounds: None,
            shell_button_bounds: None,
            session_backend_button_bounds: None,
            available_shells: available_shells(),
            hook_project_open,
            hook_project_close,
            hook_worktree_create,
            hook_worktree_close,
            hook_pre_merge,
            hook_post_merge,
            hook_before_worktree_remove,
            hook_worktree_removed,
            hook_on_rebase_conflict,
            hook_on_dirty_worktree_close,
            hook_terminal_on_create,
            hook_terminal_on_close,
            hook_terminal_shell_wrapper,
            project_hook_project_open,
            project_hook_project_close,
            project_hook_worktree_create,
            project_hook_worktree_close,
            project_hook_pre_merge,
            project_hook_post_merge,
            project_hook_before_worktree_remove,
            project_hook_worktree_removed,
            project_hook_on_rebase_conflict,
            project_hook_on_dirty_worktree_close,
            project_hook_terminal_on_create,
            project_hook_terminal_on_close,
            project_hook_terminal_shell_wrapper,
            worktree_dir_suffix_input,
            file_opener_input,
            listen_address_input,
            paired_devices,
            auth_store,
            extension_views: HashMap::new(),
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(SettingsPanelEvent::Close);
    }

    /// Create a bounds tracking callback for dropdown buttons.
    pub(super) fn bounds_setter(
        cx: &mut Context<Self>,
        setter: fn(&mut Self, Option<Bounds<Pixels>>),
    ) -> impl Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static {
        let entity = cx.entity().downgrade();
        move |bounds, _, cx: &mut App| {
            if let Some(entity) = entity.upgrade() {
                entity.update(cx, |this, _| setter(this, Some(bounds)));
            }
        }
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
            state.set_placeholder(global_hooks.project.on_open.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.project.on_open.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_project_close.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.project.on_close.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.project.on_close.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_worktree_create.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.worktree.on_create.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.worktree.on_create.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_worktree_close.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.worktree.on_close.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.worktree.on_close.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_pre_merge.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.worktree.pre_merge.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.worktree.pre_merge.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_post_merge.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.worktree.post_merge.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.worktree.post_merge.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_before_worktree_remove.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.worktree.before_remove.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.worktree.before_remove.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_worktree_removed.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.worktree.after_remove.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.worktree.after_remove.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_on_rebase_conflict.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.worktree.on_rebase_conflict.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.worktree.on_rebase_conflict.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_on_dirty_worktree_close.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.worktree.on_dirty_close.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.worktree.on_dirty_close.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_terminal_on_create.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.terminal.on_create.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.terminal.on_create.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_terminal_on_close.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.terminal.on_close.as_deref().unwrap_or("No global hook set"));
            let val = project_hooks.as_ref().and_then(|h| h.terminal.on_close.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
        self.project_hook_terminal_shell_wrapper.update(cx, |state, cx| {
            state.set_placeholder(global_hooks.terminal.shell_wrapper.as_deref().unwrap_or("e.g. devcontainer exec -- {shell}"));
            let val = project_hooks.as_ref().and_then(|h| h.terminal.shell_wrapper.clone()).unwrap_or_default();
            state.set_value(val, cx);
        });
    }

    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.active_category {
            SettingsCategory::General => self.render_general(cx).into_any_element(),
            SettingsCategory::Font => self.render_font(cx).into_any_element(),
            SettingsCategory::Terminal => self.render_terminal(cx).into_any_element(),
            SettingsCategory::Worktree => self.render_worktree(cx).into_any_element(),
            SettingsCategory::Hooks => self.render_hooks(cx).into_any_element(),
            SettingsCategory::Extensions => self.render_extensions(cx).into_any_element(),
            SettingsCategory::PairedDevices => self.render_paired_devices(cx).into_any_element(),
            SettingsCategory::Extension(ext_id) => {
                self.render_extension_settings(ext_id.clone(), cx)
            }
        };

        div()
            .id("settings-content")
            .flex_1()
            .overflow_y_scroll()
            .min_w_0()
            .child(content)
    }

    fn render_extension_settings(&mut self, ext_id: String, cx: &mut Context<Self>) -> AnyElement {
        // Lazily create and cache the extension's settings view
        if !self.extension_views.contains_key(&ext_id) {
            // Clone the factory out to avoid holding a borrow on cx
            let factory = cx
                .try_global::<ExtensionRegistry>()
                .and_then(|registry| {
                    registry
                        .extensions()
                        .iter()
                        .find(|ext| ext.manifest.id == ext_id)
                        .and_then(|ext| ext.settings_view.clone())
                });
            if let Some(factory) = factory {
                let view = factory(cx);
                self.extension_views.insert(ext_id.clone(), view);
            }
        }

        if let Some(view) = self.extension_views.get(&ext_id) {
            view.clone().into_any_element()
        } else {
            div().into_any_element()
        }
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
                    .w(px(720.0))
                    .h(px(560.0))
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
                    // Click-outside backdrop (covers the modal, under the dropdown)
                    .when(self.has_open_dropdown(), |modal| {
                        modal.child(
                            div()
                                .id("dropdown-backdrop")
                                .absolute()
                                .inset_0()
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    this.close_all_dropdowns();
                                    cx.notify();
                                }))
                        )
                    })
                    // Dropdown overlays positioned below trigger button
                    .when(self.project_dropdown_open && self.project_button_bounds.is_some(), |modal| {
                        modal.child(dropdown_anchored_below(self.project_button_bounds.unwrap(), self.render_project_dropdown_overlay(cx)))
                    })
                    .when(self.font_dropdown_open && self.font_button_bounds.is_some(), |modal| {
                        let current = settings_entity(cx).read(cx).settings.font_family.clone();
                        modal.child(dropdown_anchored_below(self.font_button_bounds.unwrap(), self.render_font_dropdown_overlay(&current, cx)))
                    })
                    .when(self.shell_dropdown_open && self.shell_button_bounds.is_some(), |modal| {
                        let current = settings_entity(cx).read(cx).settings.default_shell.clone();
                        modal.child(dropdown_anchored_below(self.shell_button_bounds.unwrap(), self.render_shell_dropdown_overlay(&current, cx)))
                    })
                    .when(self.session_backend_dropdown_open && self.session_backend_button_bounds.is_some(), |modal| {
                        let current = settings_entity(cx).read(cx).settings.session_backend;
                        modal.child(dropdown_anchored_below(self.session_backend_button_bounds.unwrap(), self.render_session_backend_dropdown_overlay(&current, cx)))
                    }),
            )
    }
}

impl_focusable!(SettingsPanel);
