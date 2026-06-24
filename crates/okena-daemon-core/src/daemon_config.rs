//! GPUI-free settings & theme handlers for the headless daemon.
//!
//! This is the headless counterpart to the desktop app's
//! `okena-app/src/app/remote_config.rs`. It serves the app-scoped remote
//! actions `GetSettings` / `SetSettings` / `GetThemes` / `GetTheme` /
//! `SetTheme` / `SaveCustomTheme` / `GetSettingsSchema` without touching GPUI.
//!
//! The data-vs-presentation split this migration follows: the daemon owns the
//! theme **preference** (the data — `theme_mode` + `custom_theme_id`, persisted
//! to settings.json) and the custom-theme files on disk. It does NOT own an
//! `AppTheme` entity, because applying colors to pixels is the client's job.
//! So every `GlobalTheme` / `AppTheme` interaction from the GPUI original is
//! dropped here; the theme preference is persisted to settings.json only.
//!
//! State is shared through a single `Arc<parking_lot::Mutex<AppSettings>>`,
//! loaded once at daemon startup via [`load_settings`]. [`DaemonConfig`] is the
//! write path; other daemon code reads the same `Arc`.
//!
//! [`load_settings`]: okena_workspace::persistence::load_settings

use std::sync::Arc;

use okena_core::api::CommandResult;
use okena_theme::custom::{get_themes_dir, load_custom_themes};
use okena_theme::{
    CustomThemeColors, CustomThemeConfig, ThemeColors, ThemeMode, DARK_THEME, HIGH_CONTRAST_THEME,
    LIGHT_THEME, PASTEL_DARK_THEME,
};
use okena_workspace::persistence::AppSettings;
use okena_workspace::settings::save_settings;
use parking_lot::Mutex;
use serde_json::{json, Value};

/// GPUI-free settings & theme handler backed by a shared
/// `Arc<parking_lot::Mutex<AppSettings>>`.
pub struct DaemonConfig {
    settings: Arc<Mutex<AppSettings>>,
}

impl DaemonConfig {
    /// Build the handler over the daemon's single shared settings cell.
    ///
    /// The daemon loads settings once at startup (via `load_settings()`) into
    /// this `Arc<Mutex<AppSettings>>`; this struct is the write path while
    /// other daemon code reads the same `Arc`.
    pub fn new(settings: Arc<Mutex<AppSettings>>) -> Self {
        Self { settings }
    }

    // ── Settings ───────────────────────────────────────────────────────────

    /// Return the full current settings as JSON.
    pub fn get_settings(&self) -> CommandResult {
        let current = self.settings.lock().clone();
        match serde_json::to_value(&current) {
            Ok(v) => CommandResult::Ok(Some(v)),
            Err(e) => CommandResult::Err(format!("failed to serialize settings: {e}")),
        }
    }

    /// Deep-merge `patch` into the current settings, validate by
    /// re-deserializing, persist, then replace the held value.
    ///
    /// Unlike the GUI there is no settings observer here, so changes to the
    /// `remote_*` fields do NOT hot-restart the remote server — they apply on
    /// the next daemon launch. On a save failure the held value is left
    /// unchanged.
    pub fn set_settings(&self, patch: Value) -> CommandResult {
        let current = self.settings.lock().clone();
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
        if let Err(e) = save_settings(&new) {
            return CommandResult::Err(format!("failed to save settings: {e}"));
        }
        *self.settings.lock() = new;
        CommandResult::Ok(Some(out))
    }

    // ── Theme ──────────────────────────────────────────────────────────────

    /// List built-in + custom themes, flagging the active one.
    pub fn get_themes(&self) -> CommandResult {
        let (mode, active_custom) = {
            let s = self.settings.lock();
            (s.theme_mode, s.custom_theme_id.clone())
        };
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
    pub fn get_theme(&self, id: Option<String>) -> CommandResult {
        let (name, is_dark, colors) = match id.as_deref() {
            None => {
                // No AppTheme entity here: derive the editable blob's colors
                // straight from the held `theme_mode`. The daemon has no
                // windowing system to detect light/dark, so Auto defaults to
                // dark for the purpose of this editable blob.
                let (mode, active_custom) = {
                    let s = self.settings.lock();
                    (s.theme_mode, s.custom_theme_id.clone())
                };
                let colors = match mode {
                    ThemeMode::Dark | ThemeMode::Auto => DARK_THEME,
                    ThemeMode::Light => LIGHT_THEME,
                    ThemeMode::PastelDark => PASTEL_DARK_THEME,
                    ThemeMode::HighContrast => HIGH_CONTRAST_THEME,
                    ThemeMode::Custom => {
                        let target = active_custom
                            .as_deref()
                            .map(|cid| format!("custom:{cid}"));
                        match target.and_then(|t| {
                            load_custom_themes().into_iter().find(|(i, _)| i.id == t)
                        }) {
                            Some((_, colors)) => colors,
                            // Custom mode but no resolvable custom theme: fall
                            // back to dark so we still return an editable blob.
                            None => DARK_THEME,
                        }
                    }
                };
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
    /// preference to settings.json (there is no `AppTheme` to update).
    pub fn set_theme(&self, id: String) -> CommandResult {
        if let Some(mode) = builtin_mode(&id) {
            self.apply_builtin(mode)
        } else {
            let cid = id.strip_prefix("custom:").unwrap_or(&id).to_string();
            let target = format!("custom:{cid}");
            match load_custom_themes().into_iter().find(|(i, _)| i.id == target) {
                Some(_) => self.apply_custom(cid),
                None => CommandResult::Err(format!("theme not found: {id}")),
            }
        }
    }

    /// Write a custom theme JSON file (a full `CustomThemeConfig`) and, when
    /// `activate`, switch the persisted preference to it.
    pub fn save_custom_theme(&self, id: String, config: Value, activate: bool) -> CommandResult {
        let cid = id.strip_prefix("custom:").unwrap_or(&id).to_string();
        if cid.is_empty()
            || !cid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return CommandResult::Err(format!(
                "invalid theme id '{cid}' (use letters, digits, '-' or '_')"
            ));
        }
        // Validate by deserializing into the typed config (serde fills any
        // missing colors with defaults).
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
            return self.apply_custom(cid);
        }
        CommandResult::Ok(Some(json!({ "id": cid, "path": path.display().to_string() })))
    }

    /// Persist a built-in theme mode preference.
    fn apply_builtin(&self, mode: ThemeMode) -> CommandResult {
        let new = {
            let mut s = self.settings.lock();
            s.theme_mode = mode;
            s.custom_theme_id = None;
            s.clone()
        };
        if let Err(e) = save_settings(&new) {
            return CommandResult::Err(format!("failed to save settings: {e}"));
        }
        CommandResult::Ok(Some(json!({ "active": mode_label(mode) })))
    }

    /// Persist a custom theme preference.
    fn apply_custom(&self, cid: String) -> CommandResult {
        let new = {
            let mut s = self.settings.lock();
            s.theme_mode = ThemeMode::Custom;
            s.custom_theme_id = Some(cid.clone());
            s.clone()
        };
        if let Err(e) = save_settings(&new) {
            return CommandResult::Err(format!("failed to save settings: {e}"));
        }
        CommandResult::Ok(Some(json!({ "active": format!("custom:{cid}") })))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(settings: AppSettings) -> DaemonConfig {
        DaemonConfig::new(Arc::new(Mutex::new(settings)))
    }

    fn default_settings() -> AppSettings {
        // Every field has a serde default, so an empty object yields all
        // defaults — the same instance the schema endpoint produces.
        serde_json::from_value::<AppSettings>(json!({})).expect("defaults")
    }

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
    fn get_settings_returns_held_value_round_trips() {
        let mut settings = default_settings();
        settings.font_size = 17.5;
        settings.font_family = "Fira Code".to_string();
        let cfg = config_with(settings);

        match cfg.get_settings() {
            CommandResult::Ok(Some(v)) => {
                assert_eq!(v["font_size"], json!(17.5));
                assert_eq!(v["font_family"], json!("Fira Code"));
                // Round-trips back into AppSettings.
                let back: AppSettings = serde_json::from_value(v).expect("round-trip");
                assert_eq!(back.font_size, 17.5);
                assert_eq!(back.font_family, "Fira Code");
            }
            other => panic!("expected Ok(Some), got {other:?}"),
        }
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
