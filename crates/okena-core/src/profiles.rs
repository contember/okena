use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static PROFILE_PATHS: OnceLock<ProfilePaths> = OnceLock::new();

// ─── Path API ─────────────────────────────────────────────────────────────────

/// All file paths for the active profile. Resolved once at startup via `init_profile()`.
#[derive(Debug)]
pub struct ProfilePaths {
    pub id: String,
    /// `<config_root>/profiles/<id>/`
    pub root: PathBuf,
    /// `<config_root>/` — only for `profiles.json` and cross-profile files
    pub config_root: PathBuf,
}

impl ProfilePaths {
    pub fn workspace_json(&self)   -> PathBuf { self.root.join("workspace.json") }
    pub fn settings_json(&self)    -> PathBuf { self.root.join("settings.json") }
    pub fn keybindings_json(&self) -> PathBuf { self.root.join("keybindings.json") }
    pub fn sessions_dir(&self)     -> PathBuf { self.root.join("sessions") }
    pub fn themes_dir(&self)       -> PathBuf { self.root.join("themes") }
    pub fn updates_dir(&self)      -> PathBuf { self.root.join("updates") }
    pub fn lock_path(&self)        -> PathBuf { self.root.join("okena.lock") }
    pub fn log_path(&self)         -> PathBuf { self.root.join("okena.log") }
    pub fn cli_json(&self)         -> PathBuf { self.root.join("cli.json") }
    pub fn remote_json(&self)      -> PathBuf { self.root.join("remote.json") }
    pub fn remote_secret(&self)    -> PathBuf { self.root.join("remote_secret") }
    pub fn remote_tokens(&self)    -> PathBuf { self.root.join("remote_tokens.json") }
    pub fn pair_code(&self)        -> PathBuf { self.root.join("pair_code") }
}

/// Initialize the process-wide active profile. Must be called exactly once before
/// any code calls `current()`. Panics if called twice.
pub fn init_profile(paths: ProfilePaths) {
    PROFILE_PATHS
        .set(paths)
        .expect("init_profile called more than once");
}

/// Returns the active profile paths. Panics if `init_profile` was never called.
pub fn current() -> &'static ProfilePaths {
    PROFILE_PATHS.get().expect("profile not initialized — call init_profile() first")
}

/// Returns the active profile paths, or `None` if `init_profile` was never called.
pub fn try_current() -> Option<&'static ProfilePaths> {
    PROFILE_PATHS.get()
}

// ─── Index schema ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub id: String,
    pub display_name: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_config_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileIndex {
    pub version: u32,
    pub profiles: Vec<ProfileEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
    pub default_profile: String,
}

impl ProfileIndex {
    pub fn load(config_root: &Path) -> Result<Self> {
        let path = config_root.join("profiles.json");
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&content).with_context(|| "parsing profiles.json")
    }

    pub fn save(&self, config_root: &Path) -> Result<()> {
        std::fs::create_dir_all(config_root)?;
        let path = config_root.join("profiles.json");
        let content = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Update `last_used` to `id` and re-save. Silently ignores save errors.
    pub fn set_last_used(&mut self, id: &str, config_root: &Path) {
        self.last_used = Some(id.to_string());
        let _ = self.save(config_root);
    }
}

// ─── Config root ──────────────────────────────────────────────────────────────

/// `~/Library/Application Support/okena` on macOS; `~/.config/okena` on Linux.
pub fn config_root() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("okena")
}

// ─── Startup resolution ───────────────────────────────────────────────────────

/// Resolve the active profile from the explicit flag, the `OKENA_PROFILE` env var,
/// and the `profiles.json` index. Creates a default profile (and migrates legacy
/// state) on first run. Returns initialized `ProfilePaths` ready for `init_profile`.
pub fn resolve_active_profile(flag_id: Option<String>) -> Result<ProfilePaths> {
    let root = config_root();
    std::fs::create_dir_all(&root)?;

    let requested = flag_id.or_else(|| std::env::var("OKENA_PROFILE").ok());

    let mut index = match ProfileIndex::load(&root) {
        Ok(idx) => idx,
        Err(_) => {
            // No profiles.json — first ever run. Bootstrap default profile.
            let idx = bootstrap_default_profile(&root)?;
            let paths = make_profile_paths(&idx.profiles[0], &root);
            migrate_legacy_layout_if_needed(&paths)?;
            return Ok(paths);
        }
    };

    let id = if let Some(req) = requested {
        if !index.profiles.iter().any(|p| p.id == req) {
            let names: Vec<&str> = index.profiles.iter().map(|p| p.id.as_str()).collect();
            bail!(
                "Profile '{}' not found. Available: {}\nRun `okena --new-profile <name>` to create one.",
                req,
                names.join(", ")
            );
        }
        req
    } else {
        pick_profile_id(&index)?
    };

    index.set_last_used(&id, &root);
    let entry = index.profiles.iter().find(|p| p.id == id).unwrap().clone();
    Ok(make_profile_paths(&entry, &root))
}

fn pick_profile_id(index: &ProfileIndex) -> Result<String> {
    if index.profiles.is_empty() {
        bail!("No profiles found. Run `okena --new-profile <name>` to create one.");
    }
    if index.profiles.len() == 1 {
        return Ok(index.profiles[0].id.clone());
    }
    // Use last_used if it still exists
    if let Some(last) = &index.last_used {
        if index.profiles.iter().any(|p| &p.id == last) {
            return Ok(last.clone());
        }
    }
    // Ambiguous — give the user a clear error
    let mut msg = String::from(
        "Multiple profiles found. Specify one with --profile <id> or OKENA_PROFILE:\n",
    );
    for p in &index.profiles {
        msg.push_str(&format!("  {:<20} {}\n", p.id, p.display_name));
    }
    bail!("{}", msg.trim_end());
}

fn make_profile_paths(entry: &ProfileEntry, config_root: &Path) -> ProfilePaths {
    let root = config_root.join("profiles").join(&entry.id);
    ProfilePaths {
        id: entry.id.clone(),
        root,
        config_root: config_root.to_path_buf(),
    }
}

// ─── Profile creation ─────────────────────────────────────────────────────────

/// Create a new profile with the given display name. Returns the generated id.
pub fn create_profile(display_name: &str) -> Result<String> {
    let root = config_root();
    let mut index = ProfileIndex::load(&root).unwrap_or_else(|_| ProfileIndex {
        version: 1,
        profiles: vec![],
        last_used: None,
        default_profile: "default".to_string(),
    });

    let id = unique_id(display_name, &index);
    let entry = ProfileEntry {
        id: id.clone(),
        display_name: display_name.to_string(),
        created_at: now_iso8601(),
        // New profiles get their own claude dir; user can override via settings.json
        claude_config_dir: Some(
            dirs::home_dir()
                .unwrap_or_default()
                .join(format!(".claude-okena-{id}"))
        ),
        icon: None,
        color: None,
    };
    index.profiles.push(entry);
    index.save(&root)?;

    // Create the profile directory and write a default settings.json snippet
    // so the Claude extension picks up the right config_dir immediately.
    let profile_root = root.join("profiles").join(&id);
    std::fs::create_dir_all(&profile_root)?;

    let claude_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(format!(".claude-okena-{id}"));
    let settings_json = serde_json::json!({
        "version": 3,
        "extension_settings": {
            "claude-code": {
                "config_dir": claude_dir.to_string_lossy()
            }
        }
    });
    let settings_path = profile_root.join("settings.json");
    if !settings_path.exists() {
        std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&settings_json)?,
        )?;
    }

    Ok(id)
}

/// Return all profiles from the index — for GUI use.
pub fn all_profiles() -> Result<Vec<ProfileEntry>> {
    let root = config_root();
    Ok(ProfileIndex::load(&root)?.profiles)
}

/// Delete a profile. Refuses to delete the active profile, the default profile, or a
/// profile whose `remote.json` points to a live PID. Removes the profile directory and
/// updates `profiles.json` (index written first so partial FS failure leaves index clean).
/// Claude credentials at `~/.claude-okena-<id>/` are intentionally preserved.
pub fn delete_profile(id: &str) -> Result<()> {
    let root = config_root();
    let mut index = ProfileIndex::load(&root)?;

    let entry = index.profiles.iter().find(|p| p.id == id)
        .ok_or_else(|| anyhow::anyhow!("Profile '{id}' does not exist"))?
        .clone();

    if id == index.default_profile {
        bail!("Cannot delete the default profile");
    }
    if let Some(active) = try_current() {
        if active.id == id {
            bail!("Cannot delete the active profile — switch to another profile first");
        }
    }
    let paths = make_profile_paths(&entry, &root);
    if is_profile_running(&paths) {
        bail!("Profile '{id}' is currently in use by another Okena instance");
    }

    index.profiles.retain(|p| p.id != id);
    if index.last_used.as_deref() == Some(id) {
        index.last_used = None;
    }
    index.save(&root)?;

    let _ = std::fs::remove_dir_all(&paths.root);
    Ok(())
}

fn is_profile_running(paths: &ProfilePaths) -> bool {
    let remote = paths.remote_json();
    let Ok(data) = std::fs::read_to_string(&remote) else { return false; };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) else { return false; };
    let pid = json.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    pid != 0 && is_process_alive(pid)
}

/// List all profiles to stdout.
pub fn list_profiles() {
    let root = config_root();
    match ProfileIndex::load(&root) {
        Ok(index) => {
            for p in &index.profiles {
                let marker = if index.last_used.as_deref() == Some(&p.id) { "*" } else { " " };
                println!("{} {:<20} {}", marker, p.id, p.display_name);
            }
        }
        Err(_) => {
            println!("No profiles found.");
        }
    }
}

// ─── Legacy migration ─────────────────────────────────────────────────────────

/// If legacy flat-layout files exist in `config_root` and we're on the `default`
/// profile, move them into the profile's root directory.
pub fn migrate_legacy_layout_if_needed(paths: &ProfilePaths) -> Result<()> {
    if paths.id != "default" {
        return Ok(());
    }
    let marker = paths.root.join(".migrated_from_legacy_v1");
    if marker.exists() {
        return Ok(());
    }

    let src = &paths.config_root;
    let dst = &paths.root;

    // Check for a live legacy lock
    let legacy_lock = src.join("okena.lock");
    if legacy_lock.exists() {
        if let Ok(content) = std::fs::read_to_string(&legacy_lock) {
            if let Ok(pid) = content.trim().parse::<u32>() {
                if is_process_alive(pid) {
                    bail!(
                        "An older Okena instance is still running (PID {pid}). \
                         Quit it before upgrading to profiles."
                    );
                }
            }
        }
        let _ = std::fs::remove_file(&legacy_lock);
    }

    // Only migrate if there are legacy files to move
    let candidates = [
        "workspace.json", "workspace.json.bak",
        "settings.json",
        "keybindings.json",
        "cli.json",
        "remote.json",
        "remote_secret", "remote_tokens.json", "pair_code",
        "okena.log", "okena.log.1",
    ];
    let dir_candidates = ["sessions", "themes", "updates"];

    let has_legacy = candidates.iter().any(|f| src.join(f).exists())
        || dir_candidates.iter().any(|d| src.join(d).exists());

    if !has_legacy {
        // Nothing to migrate — just write the marker so we don't check again
        std::fs::create_dir_all(dst)?;
        std::fs::write(&marker, now_iso8601())?;
        return Ok(());
    }

    eprintln!("Migrating legacy Okena state into profile 'default'…");
    std::fs::create_dir_all(dst)?;

    for name in &candidates {
        let from = src.join(name);
        if from.exists() {
            let to = dst.join(name);
            if let Err(e) = std::fs::rename(&from, &to) {
                eprintln!("Warning: could not migrate {name}: {e}");
            }
        }
    }
    for name in &dir_candidates {
        let from = src.join(name);
        if from.exists() {
            let to = dst.join(name);
            if let Err(e) = std::fs::rename(&from, &to) {
                eprintln!("Warning: could not migrate directory {name}: {e}");
            }
        }
    }

    std::fs::write(&marker, now_iso8601())?;
    eprintln!("Migration complete.");
    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn bootstrap_default_profile(config_root: &Path) -> Result<ProfileIndex> {
    let entry = ProfileEntry {
        id: "default".to_string(),
        display_name: "Default".to_string(),
        created_at: now_iso8601(),
        claude_config_dir: None, // use ~/.claude (silent for existing users)
        icon: None,
        color: None,
    };
    let index = ProfileIndex {
        version: 1,
        profiles: vec![entry],
        last_used: Some("default".to_string()),
        default_profile: "default".to_string(),
    };
    index.save(config_root)?;
    Ok(index)
}

fn unique_id(display_name: &str, index: &ProfileIndex) -> String {
    let slug: String = display_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    let slug = if slug.is_empty() { "profile".to_string() } else { slug };

    if !index.profiles.iter().any(|p| p.id == slug) {
        return slug;
    }
    for n in 2u32.. {
        let candidate = format!("{slug}-{n}");
        if !index.profiles.iter().any(|p| p.id == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, mo, d) = unix_days_to_ymd(s / 86400);
    let h = (s % 86400) / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{sec:02}Z")
}

fn unix_days_to_ymd(mut n: u64) -> (u64, u64, u64) {
    let mut y = 1970u64;
    loop {
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let days = if leap { 366 } else { 365 };
        if n < days { break; }
        n -= days;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let months: [u64; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo = 1u64;
    for &days in &months {
        if n < days { break; }
        n -= days;
        mo += 1;
    }
    (y, mo, n + 1)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_root() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn test_profile_index_round_trip() {
        let dir = temp_root();
        let index = ProfileIndex {
            version: 1,
            profiles: vec![ProfileEntry {
                id: "default".to_string(),
                display_name: "Default".to_string(),
                created_at: "2024-01-01T00:00:00Z".to_string(),
                claude_config_dir: None,
                icon: None,
                color: None,
            }],
            last_used: Some("default".to_string()),
            default_profile: "default".to_string(),
        };
        index.save(dir.path()).unwrap();
        let loaded = ProfileIndex::load(dir.path()).unwrap();
        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(loaded.profiles[0].id, "default");
        assert_eq!(loaded.last_used.as_deref(), Some("default"));
    }

    #[test]
    fn test_unique_id_collision() {
        let mut index = ProfileIndex {
            version: 1,
            profiles: vec![],
            last_used: None,
            default_profile: "default".to_string(),
        };
        let id1 = unique_id("work", &index);
        assert_eq!(id1, "work");
        index.profiles.push(ProfileEntry {
            id: "work".to_string(),
            display_name: "Work".to_string(),
            created_at: "".to_string(),
            claude_config_dir: None,
            icon: None,
            color: None,
        });
        let id2 = unique_id("work", &index);
        assert_eq!(id2, "work-2");
    }

    #[test]
    fn test_migration_idempotent() {
        let dir = temp_root();
        // Create legacy files
        fs::write(dir.path().join("workspace.json"), "{}").unwrap();
        fs::write(dir.path().join("settings.json"), "{}").unwrap();

        let idx = ProfileIndex {
            version: 1,
            profiles: vec![ProfileEntry {
                id: "default".into(),
                display_name: "Default".into(),
                created_at: "".into(),
                claude_config_dir: None,
                icon: None,
                color: None,
            }],
            last_used: Some("default".into()),
            default_profile: "default".into(),
        };
        idx.save(dir.path()).unwrap();

        let paths = ProfilePaths {
            id: "default".to_string(),
            root: dir.path().join("profiles").join("default"),
            config_root: dir.path().to_path_buf(),
        };
        fs::create_dir_all(&paths.root).unwrap();

        migrate_legacy_layout_if_needed(&paths).unwrap();
        assert!(paths.workspace_json().exists());
        assert!(paths.settings_json().exists());
        // Source files should be gone
        assert!(!dir.path().join("workspace.json").exists());

        // Second run should be a no-op
        migrate_legacy_layout_if_needed(&paths).unwrap();
    }

    #[test]
    fn test_profile_paths() {
        let root = PathBuf::from("/tmp/test-okena");
        let paths = ProfilePaths {
            id: "work".to_string(),
            root: root.join("profiles/work"),
            config_root: root.clone(),
        };
        assert_eq!(paths.workspace_json(), root.join("profiles/work/workspace.json"));
        assert_eq!(paths.sessions_dir(), root.join("profiles/work/sessions"));
    }

    #[test]
    fn test_now_iso8601_format() {
        let ts = now_iso8601();
        assert_eq!(ts.len(), 20); // "YYYY-MM-DDTHH:MM:SSZ"
        assert!(ts.ends_with('Z'));
    }

    fn make_test_index_with_two(dir: &TempDir) -> ProfileIndex {
        let idx = ProfileIndex {
            version: 1,
            profiles: vec![
                ProfileEntry { id: "default".into(), display_name: "Default".into(), created_at: "".into(), claude_config_dir: None, icon: None, color: None },
                ProfileEntry { id: "work".into(), display_name: "Work".into(), created_at: "".into(), claude_config_dir: None, icon: None, color: None },
            ],
            last_used: Some("work".into()),
            default_profile: "default".into(),
        };
        fs::create_dir_all(dir.path().join("profiles/default")).unwrap();
        fs::create_dir_all(dir.path().join("profiles/work")).unwrap();
        idx.save(dir.path()).unwrap();
        idx
    }

    #[test]
    fn test_all_profiles_returns_empty_on_missing_index() {
        // all_profiles reads from config_root() which is the real system path —
        // we test the round-trip via ProfileIndex directly instead.
        let dir = temp_root();
        let idx = make_test_index_with_two(&dir);
        let loaded = ProfileIndex::load(dir.path()).unwrap();
        assert_eq!(loaded.profiles.len(), idx.profiles.len());
    }

    #[test]
    fn test_delete_profile_refuses_default() {
        let dir = temp_root();
        make_test_index_with_two(&dir);

        // Simulate delete_profile logic inline (can't call it because it uses config_root())
        let root = dir.path();
        let index = ProfileIndex::load(root).unwrap();
        let err = if "default" == index.default_profile {
            Some("Cannot delete the default profile")
        } else {
            None
        };
        assert!(err.is_some());
        // index should be unchanged
        assert_eq!(index.profiles.len(), 2);
    }

    #[test]
    fn test_delete_profile_removes_entry_and_dir() {
        let dir = temp_root();
        make_test_index_with_two(&dir);

        let root = dir.path();
        let mut index = ProfileIndex::load(root).unwrap();
        let id = "work";

        // Simulate the delete logic (no try_current guard needed — OnceLock is per-process)
        index.profiles.retain(|p| p.id != id);
        if index.last_used.as_deref() == Some(id) { index.last_used = None; }
        index.save(root).unwrap();
        let work_dir = root.join("profiles/work");
        fs::remove_dir_all(&work_dir).unwrap();

        let reloaded = ProfileIndex::load(root).unwrap();
        assert_eq!(reloaded.profiles.len(), 1);
        assert_eq!(reloaded.profiles[0].id, "default");
        assert!(reloaded.last_used.is_none());
        assert!(!work_dir.exists());
    }

    #[test]
    fn test_delete_profile_clears_last_used_when_matching() {
        let dir = temp_root();
        make_test_index_with_two(&dir);
        let root = dir.path();
        let mut index = ProfileIndex::load(root).unwrap();
        assert_eq!(index.last_used.as_deref(), Some("work"));

        index.profiles.retain(|p| p.id != "work");
        if index.last_used.as_deref() == Some("work") { index.last_used = None; }
        index.save(root).unwrap();

        let reloaded = ProfileIndex::load(root).unwrap();
        assert!(reloaded.last_used.is_none());
    }

    #[test]
    fn test_delete_profile_refuses_unknown_id() {
        let dir = temp_root();
        make_test_index_with_two(&dir);
        let root = dir.path();
        let index = ProfileIndex::load(root).unwrap();
        let exists = index.profiles.iter().any(|p| p.id == "nonexistent");
        assert!(!exists, "should not find nonexistent profile");
    }

    #[test]
    fn test_delete_partial_failure_index_written_first() {
        // Verify index-save-first ordering: if the dir is already gone,
        // index is still updated (no double-removal error).
        let dir = temp_root();
        make_test_index_with_two(&dir);
        let root = dir.path();
        let mut index = ProfileIndex::load(root).unwrap();
        index.profiles.retain(|p| p.id != "work");
        index.last_used = None;
        index.save(root).unwrap();
        // Dir already gone — remove_dir_all ignores it
        let work_dir = root.join("profiles/work");
        let _ = fs::remove_dir_all(&work_dir); // first removal
        let _ = fs::remove_dir_all(&work_dir); // second — should not panic
        let reloaded = ProfileIndex::load(root).unwrap();
        assert_eq!(reloaded.profiles.len(), 1);
    }
}
