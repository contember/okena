//! GPUI-free settings & theme handlers for the headless daemon.
//!
//! This is the headless counterpart to the desktop app's
//! `okena-app/src/app/remote_config.rs`. Both now share the same logic via
//! [`okena_app_core::remote_config`]; this module only supplies the daemon's
//! [`ConfigBackend`] impl (a shared `Arc<parking_lot::Mutex<AppSettings>>`
//! backing store) and thin method wrappers over the shared functions.
//!
//! The data-vs-presentation split this migration follows: the daemon owns the
//! theme **preference** (the data — `theme_mode` + `custom_theme_id`, persisted
//! to settings.json) and the custom-theme files on disk. It does NOT own an
//! `AppTheme` entity, because applying colors to pixels is the client's job.
//! So `apply_active_theme` is a no-op here and `active_theme_colors` derives the
//! editable blob's colors straight from the persisted `theme_mode`.
//!
//! State is shared through a single `Arc<parking_lot::Mutex<AppSettings>>`,
//! loaded once at daemon startup via [`load_settings`]. [`DaemonConfig`] is the
//! write path; other daemon code reads the same `Arc`.
//!
//! [`load_settings`]: okena_workspace::persistence::load_settings

use std::sync::Arc;

use okena_app_core::remote_config::{self, ConfigBackend};
use okena_core::api::CommandResult;
use okena_theme::custom::load_custom_themes;
use okena_theme::{
    ThemeColors, ThemeMode, DARK_THEME, HIGH_CONTRAST_THEME, LIGHT_THEME, PASTEL_DARK_THEME,
};
use okena_workspace::persistence::AppSettings;
use okena_workspace::settings::save_settings;
use parking_lot::Mutex;
use serde_json::Value;

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

    /// Return the full current settings as JSON.
    pub fn get_settings(&mut self) -> CommandResult {
        remote_config::get_settings(self)
    }

    /// Deep-merge `patch` into the current settings, validate by
    /// re-deserializing, persist, then replace the held value.
    ///
    /// Unlike the GUI there is no settings observer here, so changes to the
    /// `remote_*` fields do NOT hot-restart the remote server — they apply on
    /// the next daemon launch. On a save failure the held value is left
    /// unchanged.
    pub fn set_settings(&mut self, patch: Value) -> CommandResult {
        remote_config::set_settings(self, patch)
    }

    /// List built-in + custom themes, flagging the active one.
    pub fn get_themes(&mut self) -> CommandResult {
        remote_config::get_themes(self)
    }

    /// Return a theme as an editable custom-theme blob (the active theme when
    /// `id` is None).
    pub fn get_theme(&mut self, id: Option<String>) -> CommandResult {
        remote_config::get_theme(self, id)
    }

    /// Activate a theme: a built-in mode or a custom theme id. Persists the
    /// preference to settings.json (there is no `AppTheme` to update).
    pub fn set_theme(&mut self, id: String) -> CommandResult {
        remote_config::set_theme(self, id)
    }

    /// Write a custom theme JSON file (a full `CustomThemeConfig`) and, when
    /// `activate`, switch the persisted preference to it.
    pub fn save_custom_theme(&mut self, id: String, config: Value, activate: bool) -> CommandResult {
        remote_config::save_custom_theme(self, id, config, activate)
    }
}

impl ConfigBackend for DaemonConfig {
    fn load_settings(&mut self) -> AppSettings {
        self.settings.lock().clone()
    }

    fn store_settings(&mut self, new: &AppSettings) -> Result<(), String> {
        // Persist to disk first; only replace the held value on success so a
        // save failure leaves the in-memory settings unchanged.
        save_settings(new).map_err(|e| e.to_string())?;
        *self.settings.lock() = new.clone();
        Ok(())
    }

    fn apply_active_theme(&mut self, _mode: ThemeMode, _custom_colors: Option<ThemeColors>) {
        // Headless: no live theme surface to update. The preference has already
        // been persisted via `store_settings`.
    }

    fn active_theme_colors(&mut self, mode: ThemeMode, custom_id: Option<&str>) -> ThemeColors {
        // No AppTheme entity here: derive the editable blob's colors straight
        // from the held `theme_mode`. The daemon has no windowing system to
        // detect light/dark, so Auto defaults to dark for this editable blob.
        match mode {
            ThemeMode::Dark | ThemeMode::Auto => DARK_THEME,
            ThemeMode::Light => LIGHT_THEME,
            ThemeMode::PastelDark => PASTEL_DARK_THEME,
            ThemeMode::HighContrast => HIGH_CONTRAST_THEME,
            ThemeMode::Custom => {
                let target = custom_id.map(|cid| format!("custom:{cid}"));
                match target
                    .and_then(|t| load_custom_themes().into_iter().find(|(i, _)| i.id == t))
                {
                    Some((_, colors)) => colors,
                    // Custom mode but no resolvable custom theme: fall back to
                    // dark so we still return an editable blob.
                    None => DARK_THEME,
                }
            }
        }
    }
}

/// Return a defaults instance of the settings — every key with its default
/// value, as a de-facto schema agents can read to discover available keys.
pub fn get_settings_schema() -> CommandResult {
    remote_config::get_settings_schema()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config_with(settings: AppSettings) -> DaemonConfig {
        DaemonConfig::new(Arc::new(Mutex::new(settings)))
    }

    fn default_settings() -> AppSettings {
        // Every field has a serde default, so an empty object yields all
        // defaults — the same instance the schema endpoint produces.
        serde_json::from_value::<AppSettings>(json!({})).expect("defaults")
    }

    #[test]
    fn get_settings_returns_held_value_round_trips() {
        let mut settings = default_settings();
        settings.font_size = 17.5;
        settings.font_family = "Fira Code".to_string();
        let mut cfg = config_with(settings);

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
}
