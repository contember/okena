//! Where OpenSpec keeps machine-level state.

use std::path::{Path, PathBuf};

const DIR_NAME: &str = "openspec";

/// OpenSpec's machine-level directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSpecDirs {
    /// Holds `stores/registry.yaml` and saved worksets.
    pub data_dir: PathBuf,
    /// Holds `config.json`.
    pub config_dir: PathBuf,
}

impl OpenSpecDirs {
    /// Resolve the directories for this process, with optional overrides from
    /// okena's settings. A blank override counts as unset.
    pub fn detect(data_override: Option<&str>, config_override: Option<&str>) -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let mut resolved = Self::from_env(|k| std::env::var(k).ok(), &home, cfg!(windows));
        if let Some(p) = data_override.map(str::trim).filter(|p| !p.is_empty()) {
            resolved.data_dir = expand_home(p);
        }
        if let Some(p) = config_override.map(str::trim).filter(|p| !p.is_empty()) {
            resolved.config_dir = expand_home(p);
        }
        resolved
    }

    /// Mirror of OpenSpec's `getGlobalDataDir` / `getGlobalConfigDir`:
    /// `XDG_*_HOME` wins on every platform, then the platform default.
    pub fn from_env(env: impl Fn(&str) -> Option<String>, home: &Path, windows: bool) -> Self {
        let var = |k: &str| env(k).filter(|v| !v.is_empty());
        let data_dir = match var("XDG_DATA_HOME") {
            Some(x) => PathBuf::from(x).join(DIR_NAME),
            None if windows => match var("LOCALAPPDATA") {
                Some(l) => PathBuf::from(l).join(DIR_NAME),
                None => home.join("AppData").join("Local").join(DIR_NAME),
            },
            None => home.join(".local").join("share").join(DIR_NAME),
        };
        let config_dir = match var("XDG_CONFIG_HOME") {
            Some(x) => PathBuf::from(x).join(DIR_NAME),
            None if windows => match var("APPDATA") {
                Some(a) => PathBuf::from(a).join(DIR_NAME),
                None => home.join("AppData").join("Roaming").join(DIR_NAME),
            },
            None => home.join(".config").join(DIR_NAME),
        };
        Self {
            data_dir,
            config_dir,
        }
    }

    pub fn registry_path(&self) -> PathBuf {
        self.data_dir.join("stores").join("registry.yaml")
    }

    pub fn registry_lock_path(&self) -> PathBuf {
        self.data_dir.join("stores").join("registry.yaml.lock")
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }
}

// OpenSpec compares and stores canonical paths (`realpath`), so okena must too
// or the same checkout reached through a symlink reads as two stores.
pub use okena_core::fs::{canonical, expand_home};

/// Display/storage form of a path.
pub fn display(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn unix_defaults_match_the_cli_including_on_macos() {
        // OpenSpec uses ~/.local/share and ~/.config on macOS too, not
        // ~/Library — a platform-native guess would miss every store.
        let d = OpenSpecDirs::from_env(env(&[]), Path::new("/home/u"), false);
        assert_eq!(
            d.registry_path(),
            Path::new("/home/u/.local/share/openspec/stores/registry.yaml")
        );
        assert_eq!(
            d.config_path(),
            Path::new("/home/u/.config/openspec/config.json")
        );
    }

    #[test]
    fn xdg_wins_on_every_platform() {
        let pairs = [("XDG_DATA_HOME", "/x/data"), ("XDG_CONFIG_HOME", "/x/cfg")];
        for windows in [false, true] {
            let d = OpenSpecDirs::from_env(env(&pairs), Path::new("/home/u"), windows);
            assert_eq!(d.data_dir, Path::new("/x/data/openspec"));
            assert_eq!(d.config_dir, Path::new("/x/cfg/openspec"));
        }
    }

    #[test]
    fn an_empty_xdg_variable_counts_as_unset() {
        let d = OpenSpecDirs::from_env(env(&[("XDG_DATA_HOME", "")]), Path::new("/h"), false);
        assert_eq!(d.data_dir, Path::new("/h/.local/share/openspec"));
    }

    #[test]
    fn windows_uses_localappdata_and_appdata() {
        let pairs = [("LOCALAPPDATA", "C:/L"), ("APPDATA", "C:/R")];
        let d = OpenSpecDirs::from_env(env(&pairs), Path::new("C:/Users/u"), true);
        assert_eq!(d.data_dir, Path::new("C:/L/openspec"));
        assert_eq!(d.config_dir, Path::new("C:/R/openspec"));
        let bare = OpenSpecDirs::from_env(env(&[]), Path::new("C:/Users/u"), true);
        assert_eq!(
            bare.data_dir,
            Path::new("C:/Users/u/AppData/Local/openspec")
        );
        assert_eq!(
            bare.config_dir,
            Path::new("C:/Users/u/AppData/Roaming/openspec")
        );
    }

    #[test]
    fn overrides_replace_the_resolved_directories() {
        let d = OpenSpecDirs::detect(Some("/custom/data"), Some("  "));
        assert_eq!(d.data_dir, Path::new("/custom/data"));
        // A blank override must not turn into a relative empty path.
        assert!(d.config_dir.ends_with("openspec"));
    }
}
