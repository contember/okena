//! Remote-bridge handlers for app-scoped actions: settings, theme, and the
//! command-palette action list. These touch globals (`GlobalSettings`,
//! `GlobalTheme`) and the filesystem, so they live here rather than in the
//! Workspace-scoped `execute_action`. The command-palette *invoke* needs a
//! window handle and is wired separately (see `remote_commands` + `mod.rs`).
//!
//! The settings/theme logic itself is shared with the headless daemon via
//! [`okena_app_core::remote_config`]; this module only supplies a GPUI-backed
//! [`ConfigBackend`] wrapper (over `GlobalSettings` / `GlobalTheme`) plus thin
//! public wrappers that preserve the GUI's "unavailable" error strings.

use crate::keybindings::get_action_descriptions;
use crate::remote::bridge::CommandResult;
use crate::settings::{GlobalSettings, SettingsState};
use crate::theme::{AppTheme, GlobalTheme, ThemeColors, ThemeMode, DARK_THEME};
use crate::workspace::persistence::AppSettings;
use okena_app_core::remote_config::{self, ConfigBackend};
use gpui::*;
use serde_json::{json, Value};

/// Short-lived GPUI-backed [`ConfigBackend`]: reads/writes the `GlobalSettings`
/// entity and applies the active theme to the live `GlobalTheme` entity.
struct GuiConfigBackend<'a> {
    cx: &'a mut App,
}

impl ConfigBackend for GuiConfigBackend<'_> {
    fn load_settings(&mut self) -> AppSettings {
        // The global always exists in the running GUI; if it were absent we
        // fall back to defaults (the public wrappers below still return the
        // "settings unavailable" error before ever reaching this path).
        self.cx
            .try_global::<GlobalSettings>()
            .map(|g| g.0.read(self.cx).settings.clone())
            // Unreachable in practice (the public wrappers guard on the
            // global's presence); fall back to the serde defaults instance.
            .unwrap_or_else(|| {
                serde_json::from_value::<AppSettings>(json!({})).expect("settings defaults")
            })
    }

    fn store_settings(&mut self, new: &AppSettings) -> Result<(), String> {
        let Some(entity) = self.cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) else {
            return Err("settings unavailable".into());
        };
        let new = new.clone();
        entity.update(self.cx, |st: &mut SettingsState, cx| {
            st.settings = new;
            st.save_and_notify(cx);
        });
        Ok(())
    }

    fn apply_active_theme(&mut self, mode: ThemeMode, custom_colors: Option<ThemeColors>) {
        let Some(theme) = self.cx.try_global::<GlobalTheme>().map(|g| g.0.clone()) else {
            return;
        };
        theme.update(self.cx, |t: &mut AppTheme, cx| {
            match custom_colors {
                Some(colors) => {
                    t.set_custom_colors(colors);
                    t.set_mode(ThemeMode::Custom);
                }
                None => t.set_mode(mode),
            }
            cx.notify();
        });
    }

    fn active_theme_colors(&mut self, _mode: ThemeMode, _custom_id: Option<&str>) -> ThemeColors {
        // The GUI has a live theme surface: read the colors it is actually
        // displaying rather than deriving from the persisted mode.
        self.cx
            .try_global::<GlobalTheme>()
            .map(|g| g.0.read(self.cx).display_colors())
            // Unreachable in practice (get_theme(None) guards on GlobalTheme).
            .unwrap_or(DARK_THEME)
    }
}

// ── Settings ─────────────────────────────────────────────────────────────────

/// Return the full current settings as JSON.
pub(super) fn get_settings(cx: &mut App) -> CommandResult {
    if cx.try_global::<GlobalSettings>().is_none() {
        return CommandResult::Err("settings unavailable".into());
    }
    remote_config::get_settings(&mut GuiConfigBackend { cx })
}

/// Return a defaults instance of the settings — every key with its default
/// value, as a de-facto schema agents can read to discover available keys.
pub(super) fn get_settings_schema() -> CommandResult {
    remote_config::get_settings_schema()
}

/// Deep-merge `patch` into the current settings, validate by re-deserializing,
/// then replace and persist. The app's settings observer reacts to the change
/// (e.g. restarting the remote server when remote_* fields change).
pub(super) fn set_settings(cx: &mut App, patch: Value) -> CommandResult {
    if cx.try_global::<GlobalSettings>().is_none() {
        return CommandResult::Err("settings unavailable".into());
    }
    remote_config::set_settings(&mut GuiConfigBackend { cx }, patch)
}

// ── Theme ────────────────────────────────────────────────────────────────────

/// List built-in + custom themes, flagging the active one.
pub(super) fn get_themes(cx: &mut App) -> CommandResult {
    if cx.try_global::<GlobalSettings>().is_none() {
        return CommandResult::Err("settings unavailable".into());
    }
    remote_config::get_themes(&mut GuiConfigBackend { cx })
}

/// Return a theme as an editable custom-theme blob (the active theme when
/// `id` is None).
pub(super) fn get_theme(cx: &mut App, id: Option<String>) -> CommandResult {
    // The active-theme (`None`) blob reads the live `GlobalTheme`; preserve the
    // original "theme unavailable" error when it is absent.
    if id.is_none() && cx.try_global::<GlobalTheme>().is_none() {
        return CommandResult::Err("theme unavailable".into());
    }
    remote_config::get_theme(&mut GuiConfigBackend { cx }, id)
}

/// Activate a theme: a built-in mode or a custom theme id.
pub(super) fn set_theme(cx: &mut App, id: String) -> CommandResult {
    if cx.try_global::<GlobalTheme>().is_none() {
        return CommandResult::Err("theme unavailable".into());
    }
    remote_config::set_theme(&mut GuiConfigBackend { cx }, id)
}

/// Write a custom theme JSON file (a full `CustomThemeConfig`) and, when
/// `activate`, switch to it.
pub(super) fn save_custom_theme(
    cx: &mut App,
    id: String,
    config: Value,
    activate: bool,
) -> CommandResult {
    if activate && cx.try_global::<GlobalTheme>().is_none() {
        return CommandResult::Err("theme unavailable".into());
    }
    remote_config::save_custom_theme(&mut GuiConfigBackend { cx }, id, config, activate)
}

// ── Command palette ──────────────────────────────────────────────────────────

/// List invokable GUI commands, sorted. `name` is the identifier `invoke_action`
/// expects (the registry key, e.g. "ToggleSidebar"); `label` is the human name.
pub(super) fn list_actions() -> CommandResult {
    let descs = get_action_descriptions();
    let mut actions: Vec<Value> = descs
        .iter()
        .map(|(key, d)| {
            json!({
                "name": *key,
                "label": d.name,
                "description": d.description,
                "category": d.category,
            })
        })
        .collect();
    actions.sort_by(|a, b| {
        let ka = (a["category"].as_str().unwrap_or(""), a["name"].as_str().unwrap_or(""));
        let kb = (b["category"].as_str().unwrap_or(""), b["name"].as_str().unwrap_or(""));
        ka.cmp(&kb)
    });
    CommandResult::Ok(Some(json!({ "actions": actions })))
}
