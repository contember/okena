//! Shared settings & theme handlers for the remote-control API, used by both
//! the GPUI desktop app and the headless daemon.
//!
//! The two front-ends serve the same app-scoped remote actions (`GetSettings`,
//! `SetSettings`, `GetThemes`, `GetTheme`, `SetTheme`, `SaveCustomTheme`,
//! `GetSettingsSchema`) but differ in exactly three places:
//!
//! 1. how they read/write the backing `AppSettings` store,
//! 2. whether they also apply the active theme to a live surface (the GUI has
//!    an `AppTheme` global; the daemon is headless and has none), and
//! 3. the source of the "active theme" colors for `get_theme(None)` (the GUI
//!    reads the live `AppTheme.display_colors()`; the daemon derives them from
//!    the persisted `theme_mode`).
//!
//! Those three divergent pieces are abstracted behind [`ConfigBackend`]; the
//! rest of the logic (deep-merge, validation, theme-list assembly, id
//! normalization, JSON shapes and error strings) lives here once and is shared
//! verbatim.
//!
//! This module is GPUI-free so it compiles under `--no-default-features`; the
//! GUI supplies a `&mut App`-holding backend, the daemon an
//! `Arc<Mutex<AppSettings>>`-holding one.

use okena_core::api::CommandResult;
use okena_theme::custom::{get_themes_dir, load_custom_themes};
use okena_theme::{
    CustomThemeColors, CustomThemeConfig, ThemeColors, ThemeMode, DARK_THEME, HIGH_CONTRAST_THEME,
    LIGHT_THEME, PASTEL_DARK_THEME,
};
use okena_workspace::persistence::AppSettings;
use serde_json::{json, Value};

/// Abstracts the three front-end-specific pieces of the settings/theme handlers
/// (backing store, live-theme application, active-theme color source) so the
/// rest of the logic can be shared between the GPUI app and the headless daemon.
pub trait ConfigBackend {
    /// Read the current settings snapshot.
    fn load_settings(&mut self) -> AppSettings;

    /// Persist the new settings (and, on the GUI, notify observers). Returns an
    /// error string on failure so the caller can surface it verbatim.
    fn store_settings(&mut self, new: &AppSettings) -> Result<(), String>;

    /// Apply the active theme to any live surface. The daemon is headless and
    /// implements this as a no-op; the GUI updates the `AppTheme` global.
    ///
    /// `custom_colors` is `Some` when a custom theme is being activated (in
    /// which case `mode` is [`ThemeMode::Custom`]) and `None` for built-ins.
    fn apply_active_theme(&mut self, mode: ThemeMode, custom_colors: Option<ThemeColors>);

    /// Colors for the "active theme" editable blob returned by
    /// [`get_theme`] with `id == None`. The GUI reads the live
    /// `AppTheme.display_colors()`; the daemon derives them from `mode`.
    fn active_theme_colors(&mut self, mode: ThemeMode, custom_id: Option<&str>) -> ThemeColors;
}

// ── Settings ─────────────────────────────────────────────────────────────────

/// Return the full current settings as JSON.
pub fn get_settings<B: ConfigBackend>(b: &mut B) -> CommandResult {
    let current = b.load_settings();
    match serde_json::to_value(&current) {
        Ok(v) => CommandResult::Ok(Some(v)),
        Err(e) => CommandResult::Err(format!("failed to serialize settings: {e}")),
    }
}

/// Return a defaults instance of the settings — every key with its default
/// value, as a de-facto schema agents can read to discover available keys.
pub fn get_settings_schema() -> CommandResult {
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
/// then replace and persist via the backend.
pub fn set_settings<B: ConfigBackend>(b: &mut B, patch: Value) -> CommandResult {
    let current = b.load_settings();
    let mut value = match serde_json::to_value(&current) {
        Ok(v) => v,
        Err(e) => return CommandResult::Err(format!("failed to read settings: {e}")),
    };
    merge_json(&mut value, patch);
    let new: AppSettings = match serde_json::from_value(value) {
        Ok(s) => s,
        Err(e) => return CommandResult::Err(format!("invalid settings: {e}")),
    };
    let out = match serde_json::to_value(&new) {
        Ok(v) => v,
        Err(e) => return CommandResult::Err(format!("failed to serialize settings: {e}")),
    };
    if let Err(e) = b.store_settings(&new) {
        return CommandResult::Err(format!("failed to save settings: {e}"));
    }
    CommandResult::Ok(Some(out))
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
pub fn get_themes<B: ConfigBackend>(b: &mut B) -> CommandResult {
    let settings = b.load_settings();
    let mode = settings.theme_mode;
    let active_custom = settings.custom_theme_id.clone();

    let custom = load_custom_themes();
    let custom_refs: Vec<(String, String, bool)> = custom
        .iter()
        .map(|(info, _colors)| {
            let cid = info.id.strip_prefix("custom:").unwrap_or(&info.id).to_string();
            (cid, info.name.clone(), info.is_dark)
        })
        .collect();
    let themes = build_themes_list(mode, active_custom.as_deref(), &custom_refs);
    CommandResult::Ok(Some(json!({ "themes": themes })))
}

/// Return a theme as an editable custom-theme blob (the active theme when
/// `id` is None).
pub fn get_theme<B: ConfigBackend>(b: &mut B, id: Option<String>) -> CommandResult {
    let (name, is_dark, colors) = match id.as_deref() {
        None => {
            let settings = b.load_settings();
            let mode = settings.theme_mode;
            let custom_id = settings.custom_theme_id.clone();
            let colors = b.active_theme_colors(mode, custom_id.as_deref());
            (
                format!("Active ({})", mode_label(mode)),
                mode != ThemeMode::Light,
                colors,
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

/// Activate a theme: a built-in mode or a custom theme id. Persists the
/// preference and applies it to any live surface.
pub fn set_theme<B: ConfigBackend>(b: &mut B, id: String) -> CommandResult {
    if let Some(mode) = builtin_mode(&id) {
        let mut new = b.load_settings();
        new.theme_mode = mode;
        new.custom_theme_id = None;
        if let Err(e) = b.store_settings(&new) {
            return CommandResult::Err(format!("failed to save settings: {e}"));
        }
        b.apply_active_theme(mode, None);
        CommandResult::Ok(Some(json!({ "active": mode_label(mode) })))
    } else {
        let cid = id.strip_prefix("custom:").unwrap_or(&id).to_string();
        let target = format!("custom:{cid}");
        match load_custom_themes().into_iter().find(|(i, _)| i.id == target) {
            Some((_, colors)) => apply_custom(b, cid, colors),
            None => CommandResult::Err(format!("theme not found: {id}")),
        }
    }
}

/// Write a custom theme JSON file (a full `CustomThemeConfig`) and, when
/// `activate`, switch to it.
pub fn save_custom_theme<B: ConfigBackend>(
    b: &mut B,
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
        return apply_custom(b, cid, colors);
    }
    CommandResult::Ok(Some(json!({ "id": cid, "path": path.display().to_string() })))
}

/// Persist a custom theme preference and apply it to any live surface.
fn apply_custom<B: ConfigBackend>(b: &mut B, cid: String, colors: ThemeColors) -> CommandResult {
    let mut new = b.load_settings();
    new.theme_mode = ThemeMode::Custom;
    new.custom_theme_id = Some(cid.clone());
    if let Err(e) = b.store_settings(&new) {
        return CommandResult::Err(format!("failed to save settings: {e}"));
    }
    b.apply_active_theme(ThemeMode::Custom, Some(colors));
    CommandResult::Ok(Some(json!({ "active": format!("custom:{cid}") })))
}

/// Build the themes list (built-ins + custom) flagging the active one. Pure:
/// the FS read (`load_custom_themes`) happens in the caller and is passed in as
/// `custom` triples of `(id, name, is_dark)` (the `custom:` prefix already
/// stripped from the id).
fn build_themes_list(
    mode: ThemeMode,
    active_custom: Option<&str>,
    custom: &[(String, String, bool)],
) -> Vec<Value> {
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
    for (cid, name, is_dark) in custom {
        let active = mode == ThemeMode::Custom && active_custom == Some(cid.as_str());
        themes.push(json!({
            "id": cid, "name": name, "kind": "custom",
            "is_dark": is_dark, "active": active,
        }));
    }
    themes
}

/// Normalize a theme id ("Pastel Dark" / "pastel-dark" → "pasteldark") and map
/// to a built-in [`ThemeMode`]. Returns None for custom ids.
pub fn builtin_mode(id: &str) -> Option<ThemeMode> {
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
pub fn builtin_colors(id: &str) -> Option<(&'static str, bool, ThemeColors)> {
    let n = id.to_ascii_lowercase().replace(['-', '_', ' '], "");
    match n.as_str() {
        "dark" => Some(("Dark", true, DARK_THEME)),
        "light" => Some(("Light", false, LIGHT_THEME)),
        "pasteldark" => Some(("Pastel Dark", true, PASTEL_DARK_THEME)),
        "highcontrast" => Some(("High Contrast", true, HIGH_CONTRAST_THEME)),
        _ => None,
    }
}

pub fn mode_label(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::Auto => "auto",
        ThemeMode::Dark => "dark",
        ThemeMode::Light => "light",
        ThemeMode::PastelDark => "pastel-dark",
        ThemeMode::HighContrast => "high-contrast",
        ThemeMode::Custom => "custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_json_deep_merges_objects() {
        let mut base = json!({
            "a": 1,
            "nested": { "x": 1, "y": 2 },
        });
        merge_json(
            &mut base,
            json!({
                "b": 2,
                "nested": { "y": 20, "z": 30 },
            }),
        );
        assert_eq!(
            base,
            json!({
                "a": 1,
                "b": 2,
                "nested": { "x": 1, "y": 20, "z": 30 },
            })
        );
    }

    #[test]
    fn merge_json_overwrites_scalars_and_arrays_wholesale() {
        let mut base = json!({ "n": 1, "list": [1, 2, 3] });
        merge_json(&mut base, json!({ "n": 99, "list": [4] }));
        assert_eq!(base, json!({ "n": 99, "list": [4] }));

        // A scalar replaced by an object is overwritten wholesale, not merged.
        let mut base = json!({ "k": 5 });
        merge_json(&mut base, json!({ "k": { "deep": true } }));
        assert_eq!(base, json!({ "k": { "deep": true } }));
    }

    #[test]
    fn builtin_mode_normalizes_variants() {
        assert_eq!(builtin_mode("auto"), Some(ThemeMode::Auto));
        assert_eq!(builtin_mode("dark"), Some(ThemeMode::Dark));
        assert_eq!(builtin_mode("light"), Some(ThemeMode::Light));
        assert_eq!(builtin_mode("high-contrast"), Some(ThemeMode::HighContrast));
        // All three spellings of pastel dark normalize to the same mode.
        assert_eq!(builtin_mode("Pastel Dark"), Some(ThemeMode::PastelDark));
        assert_eq!(builtin_mode("pastel-dark"), Some(ThemeMode::PastelDark));
        assert_eq!(builtin_mode("pasteldark"), Some(ThemeMode::PastelDark));
        // Unknown / custom ids are not built-ins.
        assert_eq!(builtin_mode("custom:mine"), None);
        assert_eq!(builtin_mode("nonsense"), None);
    }

    #[test]
    fn builtin_colors_resolves_only_concrete_builtins() {
        assert!(builtin_colors("dark").is_some());
        assert!(builtin_colors("light").is_some());
        assert!(builtin_colors("Pastel Dark").is_some());
        assert!(builtin_colors("high-contrast").is_some());
        // "auto" has no concrete colors (it follows the system), and custom
        // ids are resolved elsewhere.
        assert!(builtin_colors("auto").is_none());
        assert!(builtin_colors("custom:mine").is_none());
    }

    #[test]
    fn get_settings_schema_contains_expected_keys() {
        match get_settings_schema() {
            CommandResult::Ok(Some(v)) => {
                let obj = v.as_object().expect("schema is an object");
                assert!(obj.contains_key("font_size"));
                assert!(obj.contains_key("theme_mode"));
                assert!(obj.contains_key("font_family"));
                // The schema deserializes back into AppSettings (it IS the
                // defaults instance).
                serde_json::from_value::<AppSettings>(v).expect("schema round-trips");
            }
            other => panic!("expected Ok(Some), got {other:?}"),
        }
    }

    #[test]
    fn build_themes_list_flags_active_builtin() {
        let custom = vec![("mine".to_string(), "Mine".to_string(), true)];
        let themes = build_themes_list(ThemeMode::PastelDark, None, &custom);

        // Exactly the pastel-dark built-in is active; no custom theme is active
        // because mode != Custom.
        for t in &themes {
            let id = t["id"].as_str().unwrap();
            let active = t["active"].as_bool().unwrap();
            if id == "pastel-dark" {
                assert!(active, "pastel-dark should be active");
            } else {
                assert!(!active, "{id} should be inactive");
            }
        }
    }

    #[test]
    fn build_themes_list_flags_active_custom() {
        let custom = vec![
            ("mine".to_string(), "Mine".to_string(), true),
            ("other".to_string(), "Other".to_string(), false),
        ];
        let themes = build_themes_list(ThemeMode::Custom, Some("mine"), &custom);

        // No built-in is active (mode == Custom), and only the matching custom
        // id is flagged.
        for t in &themes {
            let id = t["id"].as_str().unwrap();
            let kind = t["kind"].as_str().unwrap();
            let active = t["active"].as_bool().unwrap();
            match (kind, id) {
                ("custom", "mine") => assert!(active, "active custom should be flagged"),
                _ => assert!(!active, "{kind}/{id} should be inactive"),
            }
        }
    }

    #[test]
    fn build_themes_list_no_custom_active_when_id_mismatches() {
        let custom = vec![("mine".to_string(), "Mine".to_string(), true)];
        let themes = build_themes_list(ThemeMode::Custom, Some("missing"), &custom);
        // Custom mode but the active id doesn't match any present custom theme:
        // nothing is active.
        assert!(themes.iter().all(|t| !t["active"].as_bool().unwrap()));
    }
}
