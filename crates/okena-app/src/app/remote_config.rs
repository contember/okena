//! Remote-bridge handlers for app-scoped actions: settings, theme, and the
//! command-palette action list. These touch globals (`GlobalSettings`,
//! `GlobalTheme`) and the filesystem, so they live here rather than in the
//! Workspace-scoped `execute_action`. The command-palette *invoke* needs a
//! window handle and is wired separately (see `remote_commands` + `mod.rs`).

use crate::keybindings::get_action_descriptions;
use crate::remote::bridge::CommandResult;
use crate::settings::{GlobalSettings, SettingsState};
use crate::theme::{
    AppTheme, CustomThemeColors, CustomThemeConfig, GlobalTheme, ThemeColors, ThemeMode,
    DARK_THEME, HIGH_CONTRAST_THEME, LIGHT_THEME, PASTEL_DARK_THEME, get_themes_dir,
    load_custom_themes,
};
use crate::workspace::persistence::AppSettings;
use gpui::*;
use serde_json::{json, Value};

// ── Settings ─────────────────────────────────────────────────────────────────

/// Return the full current settings as JSON.
pub(super) fn get_settings(cx: &App) -> CommandResult {
    match current_settings(cx) {
        Some(s) => match serde_json::to_value(&s) {
            Ok(v) => CommandResult::Ok(Some(v)),
            Err(e) => CommandResult::Err(format!("failed to serialize settings: {e}")),
        },
        None => CommandResult::Err("settings unavailable".into()),
    }
}

/// Return a defaults instance of the settings — every key with its default
/// value, as a de-facto schema agents can read to discover available keys.
pub(super) fn get_settings_schema() -> CommandResult {
    // Every field has a serde default, so an empty object yields all defaults.
    match serde_json::from_value::<AppSettings>(json!({})) {
        Ok(defaults) => match serde_json::to_value(&defaults) {
            Ok(v) => CommandResult::Ok(Some(v)),
            Err(e) => CommandResult::Err(format!("failed to serialize schema: {e}")),
        },
        Err(e) => CommandResult::Err(format!("failed to build schema: {e}")),
    }
}

/// Deep-merge `patch` into the current settings, validate by re-deserializing,
/// then replace and persist. The app's settings observer reacts to the change
/// (e.g. restarting the remote server when remote_* fields change).
pub(super) fn set_settings(cx: &mut App, patch: Value) -> CommandResult {
    let Some(entity) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) else {
        return CommandResult::Err("settings unavailable".into());
    };
    let current = entity.read(cx).settings.clone();
    let mut json = match serde_json::to_value(&current) {
        Ok(v) => v,
        Err(e) => return CommandResult::Err(format!("failed to read settings: {e}")),
    };
    merge_json(&mut json, patch);
    let new: AppSettings = match serde_json::from_value(json) {
        Ok(s) => s,
        Err(e) => return CommandResult::Err(format!("invalid settings: {e}")),
    };
    let out = serde_json::to_value(&new).unwrap_or(Value::Null);
    entity.update(cx, |st, cx| {
        st.settings = new;
        st.save_and_notify(cx);
    });
    CommandResult::Ok(Some(out))
}

fn current_settings(cx: &App) -> Option<AppSettings> {
    cx.try_global::<GlobalSettings>()
        .map(|g| g.0.read(cx).settings.clone())
}

/// Recursively merge `patch` into `base`: objects merge key-by-key, everything
/// else (scalars, arrays) is overwritten wholesale.
fn merge_json(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                merge_json(b.entry(k).or_insert(Value::Null), v);
            }
        }
        (b, p) => *b = p,
    }
}

// ── Theme ────────────────────────────────────────────────────────────────────

/// List built-in + custom themes, flagging the active one.
pub(super) fn get_themes(cx: &App) -> CommandResult {
    let settings = current_settings(cx);
    let mode = settings.as_ref().map(|s| s.theme_mode).unwrap_or_default();
    let active_custom = settings.and_then(|s| s.custom_theme_id);

    let mut themes = Vec::new();
    for (id, name, is_dark) in [
        ("auto", "Auto", Value::Null),
        ("dark", "Dark", json!(true)),
        ("light", "Light", json!(false)),
        ("pastel-dark", "Pastel Dark", json!(true)),
        ("high-contrast", "High Contrast", json!(true)),
    ] {
        let active = mode != ThemeMode::Custom && builtin_mode(id) == Some(mode);
        themes.push(json!({
            "id": id, "name": name, "kind": "builtin", "is_dark": is_dark, "active": active,
        }));
    }
    for (info, _colors) in load_custom_themes() {
        let cid = info.id.strip_prefix("custom:").unwrap_or(&info.id).to_string();
        let active = mode == ThemeMode::Custom && active_custom.as_deref() == Some(cid.as_str());
        themes.push(json!({
            "id": cid, "name": info.name, "kind": "custom",
            "is_dark": info.is_dark, "active": active,
        }));
    }
    CommandResult::Ok(Some(json!({ "themes": themes })))
}

/// Return a theme as an editable custom-theme blob (the active theme when
/// `id` is None).
pub(super) fn get_theme(cx: &App, id: Option<String>) -> CommandResult {
    let (name, is_dark, colors) = match id.as_deref() {
        None => {
            let Some(theme) = cx.try_global::<GlobalTheme>().map(|g| g.0.clone()) else {
                return CommandResult::Err("theme unavailable".into());
            };
            let mode = current_settings(cx).map(|s| s.theme_mode).unwrap_or_default();
            (
                format!("Active ({})", mode_label(mode)),
                mode != ThemeMode::Light,
                theme.read(cx).display_colors(),
            )
        }
        Some(raw) => match builtin_colors(raw) {
            Some((name, is_dark, colors)) => (name.to_string(), is_dark, colors),
            None => {
                let cid = raw.strip_prefix("custom:").unwrap_or(raw);
                let target = format!("custom:{cid}");
                match load_custom_themes().into_iter().find(|(i, _)| i.id == target) {
                    Some((info, colors)) => (info.name, info.is_dark, colors),
                    None => return CommandResult::Err(format!("theme not found: {raw}")),
                }
            }
        },
    };
    let blob = CustomThemeConfig {
        name,
        description: String::new(),
        is_dark,
        colors: CustomThemeColors::from_theme_colors(&colors),
    };
    match serde_json::to_value(&blob) {
        Ok(v) => CommandResult::Ok(Some(v)),
        Err(e) => CommandResult::Err(format!("failed to serialize theme: {e}")),
    }
}

/// Activate a theme: a built-in mode or a custom theme id.
pub(super) fn set_theme(cx: &mut App, id: String) -> CommandResult {
    if let Some(mode) = builtin_mode(&id) {
        apply_builtin(cx, mode)
    } else {
        let cid = id.strip_prefix("custom:").unwrap_or(&id).to_string();
        let target = format!("custom:{cid}");
        match load_custom_themes().into_iter().find(|(i, _)| i.id == target) {
            Some((_, colors)) => apply_custom(cx, cid, colors),
            None => CommandResult::Err(format!("theme not found: {id}")),
        }
    }
}

/// Write a custom theme JSON file (a full `CustomThemeConfig`) and, when
/// `activate`, switch to it.
pub(super) fn save_custom_theme(
    cx: &mut App,
    id: String,
    config: Value,
    activate: bool,
) -> CommandResult {
    let cid = id.strip_prefix("custom:").unwrap_or(&id).to_string();
    if cid.is_empty() || !cid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return CommandResult::Err(format!(
            "invalid theme id '{cid}' (use letters, digits, '-' or '_')"
        ));
    }
    // Validate by deserializing into the typed config (serde fills any missing
    // colors with defaults).
    let parsed: CustomThemeConfig = match serde_json::from_value(config) {
        Ok(c) => c,
        Err(e) => return CommandResult::Err(format!("invalid theme config: {e}")),
    };
    let dir = get_themes_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return CommandResult::Err(format!("failed to create themes dir: {e}"));
    }
    let path = dir.join(format!("{cid}.json"));
    let pretty = match serde_json::to_string_pretty(&parsed) {
        Ok(s) => s,
        Err(e) => return CommandResult::Err(format!("failed to serialize theme: {e}")),
    };
    if let Err(e) = std::fs::write(&path, pretty) {
        return CommandResult::Err(format!("failed to write {}: {e}", path.display()));
    }
    if activate {
        let colors = parsed.colors.to_theme_colors();
        return apply_custom(cx, cid, colors);
    }
    CommandResult::Ok(Some(json!({ "id": cid, "path": path.display().to_string() })))
}

fn apply_builtin(cx: &mut App, mode: ThemeMode) -> CommandResult {
    let Some(theme) = cx.try_global::<GlobalTheme>().map(|g| g.0.clone()) else {
        return CommandResult::Err("theme unavailable".into());
    };
    theme.update(cx, |t: &mut AppTheme, cx| {
        t.set_mode(mode);
        cx.notify();
    });
    if let Some(settings) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
        settings.update(cx, |s: &mut SettingsState, cx| {
            s.settings.theme_mode = mode;
            s.settings.custom_theme_id = None;
            s.save_and_notify(cx);
        });
    }
    CommandResult::Ok(Some(json!({ "active": mode_label(mode) })))
}

fn apply_custom(cx: &mut App, cid: String, colors: ThemeColors) -> CommandResult {
    let Some(theme) = cx.try_global::<GlobalTheme>().map(|g| g.0.clone()) else {
        return CommandResult::Err("theme unavailable".into());
    };
    theme.update(cx, |t: &mut AppTheme, cx| {
        t.set_custom_colors(colors);
        t.set_mode(ThemeMode::Custom);
        cx.notify();
    });
    if let Some(settings) = cx.try_global::<GlobalSettings>().map(|g| g.0.clone()) {
        let id = cid.clone();
        settings.update(cx, |s: &mut SettingsState, cx| {
            s.settings.theme_mode = ThemeMode::Custom;
            s.settings.custom_theme_id = Some(id);
            s.save_and_notify(cx);
        });
    }
    CommandResult::Ok(Some(json!({ "active": format!("custom:{cid}") })))
}

/// Normalize a theme id ("Pastel Dark" / "pastel-dark" → "pasteldark") and map
/// to a built-in [`ThemeMode`]. Returns None for custom ids.
fn builtin_mode(id: &str) -> Option<ThemeMode> {
    let n = id.to_ascii_lowercase().replace(['-', '_', ' '], "");
    match n.as_str() {
        "auto" => Some(ThemeMode::Auto),
        "dark" => Some(ThemeMode::Dark),
        "light" => Some(ThemeMode::Light),
        "pasteldark" => Some(ThemeMode::PastelDark),
        "highcontrast" => Some(ThemeMode::HighContrast),
        _ => None,
    }
}

/// Concrete colors for a built-in theme id (None for "auto" and custom ids).
fn builtin_colors(id: &str) -> Option<(&'static str, bool, ThemeColors)> {
    let n = id.to_ascii_lowercase().replace(['-', '_', ' '], "");
    match n.as_str() {
        "dark" => Some(("Dark", true, DARK_THEME)),
        "light" => Some(("Light", false, LIGHT_THEME)),
        "pasteldark" => Some(("Pastel Dark", true, PASTEL_DARK_THEME)),
        "highcontrast" => Some(("High Contrast", true, HIGH_CONTRAST_THEME)),
        _ => None,
    }
}

fn mode_label(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::Auto => "auto",
        ThemeMode::Dark => "dark",
        ThemeMode::Light => "light",
        ThemeMode::PastelDark => "pastel-dark",
        ThemeMode::HighContrast => "high-contrast",
        ThemeMode::Custom => "custom",
    }
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
