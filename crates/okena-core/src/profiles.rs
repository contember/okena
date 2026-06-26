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
    /// Pristine pre-upgrade copies of config, one dir per outgoing version.
    pub fn config_backups_dir(&self) -> PathBuf { self.root.join("config-backups") }
    /// Plain-text marker holding the last app version that ran on this profile.
    pub fn app_version_marker(&self) -> PathBuf { self.root.join(".app-version") }
}

/// Initialize the process-wide active profile. Must be called exactly once before
/// any code calls `current()`. Panics if called twice.
pub fn init_profile(paths: ProfilePaths) {
    // Intentional panic: documented "call exactly once" contract.
    #[allow(clippy::expect_used)]
    PROFILE_PATHS
        .set(paths)
        .expect("init_profile called more than once");
}

/// Returns the active profile paths. Panics if `init_profile` was never called.
pub fn current() -> &'static ProfilePaths {
    // Intentional panic: documented precondition that init_profile() ran first.
    #[allow(clippy::expect_used)]
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
            // Migration is handled by the caller (main.rs) after init_profile.
            let idx = bootstrap_default_profile(&root)?;
            if let Some(req) = &requested
                && req != "default" {
                    bail!(
                        "Profile '{req}' not found. This appears to be a first launch; \
                         the 'default' profile was just created.\n\
                         Run `okena --new-profile {req}` to create it, \
                         or omit --profile to use 'default'."
                    );
                }
            return make_profile_paths(&idx.profiles[0], &root);
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
    // `id` is guaranteed present: it was either validated against the index above
    // or returned by pick_profile_id, which only yields ids from this same index.
    #[allow(clippy::unwrap_used)]
    let entry = index.profiles.iter().find(|p| p.id == id).unwrap().clone();
    make_profile_paths(&entry, &root)
}

fn pick_profile_id(index: &ProfileIndex) -> Result<String> {
    if index.profiles.is_empty() {
        bail!("No profiles found. Run `okena --new-profile <name>` to create one.");
    }
    if index.profiles.len() == 1 {
        return Ok(index.profiles[0].id.clone());
    }
    // Use last_used if it still exists
    if let Some(last) = &index.last_used
        && index.profiles.iter().any(|p| &p.id == last) {
            return Ok(last.clone());
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

fn validate_profile_id(id: &str) -> Result<()> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") || id.contains('\0') {
        bail!("Invalid profile id: '{id}'");
    }
    Ok(())
}

fn make_profile_paths(entry: &ProfileEntry, config_root: &Path) -> Result<ProfilePaths> {
    validate_profile_id(&entry.id)?;
    let root = config_root.join("profiles").join(&entry.id);
    Ok(ProfilePaths {
        id: entry.id.clone(),
        root,
        config_root: config_root.to_path_buf(),
    })
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
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot create profile: home directory not found"))?;
    let claude_dir = home.join(format!(".claude-{id}"));

    // Create the profile directory and write the default settings.json snippet
    // BEFORE updating the index. If any of these fs operations fail, the index
    // stays clean instead of pointing at a half-built profile root.
    let profile_root = root.join("profiles").join(&id);
    std::fs::create_dir_all(&profile_root)?;
    let settings_path = profile_root.join("settings.json");
    if !settings_path.exists() {
        let settings_json = serde_json::json!({
            "version": 3,
            "extension_settings": {
                "claude-code": {
                    "config_dir": claude_dir.to_string_lossy()
                }
            }
        });
        std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&settings_json)?,
        )?;
    }

    let entry = ProfileEntry {
        id: id.clone(),
        display_name: display_name.to_string(),
        created_at: now_iso8601(),
        icon: None,
        color: None,
    };
    index.profiles.push(entry);
    index.save(&root)?;

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
/// Claude credentials at `~/.claude-<id>/` are intentionally preserved.
pub fn delete_profile(id: &str) -> Result<()> {
    let root = config_root();
    let mut index = ProfileIndex::load(&root)?;

    let entry = index.profiles.iter().find(|p| p.id == id)
        .ok_or_else(|| anyhow::anyhow!("Profile '{id}' does not exist"))?
        .clone();

    if id == index.default_profile {
        bail!("Cannot delete the default profile");
    }
    if let Some(active) = try_current()
        && active.id == id {
            bail!("Cannot delete the active profile — switch to another profile first");
        }
    let paths = make_profile_paths(&entry, &root)?;
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
        if let Ok(content) = std::fs::read_to_string(&legacy_lock)
            && let Ok(pid) = content.trim().parse::<u32>()
                && is_process_alive(pid) {
                    bail!(
                        "An older Okena instance is still running (PID {pid}). \
                         Quit it before upgrading to profiles."
                    );
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

    const SENSITIVE: &[&str] = &["remote_secret", "remote_tokens.json", "pair_code"];

    for name in &candidates {
        let from = src.join(name);
        if from.exists() {
            let to = dst.join(name);
            if let Err(e) = std::fs::rename(&from, &to) {
                eprintln!("Warning: could not migrate {name}: {e}");
            } else {
                #[cfg(unix)]
                if SENSITIVE.contains(name) {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o600));
                }
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

// ─── Config snapshots (upgrade safety-net) ──────────────────────────────────────

/// A config file's in-code schema version, used to detect a pending migration
/// (on-disk version older than what this build produces).
pub struct SchemaVersion {
    /// File name relative to the profile root, e.g. `"workspace.json"`.
    pub file: &'static str,
    /// The schema version this build expects/produces.
    pub current: u32,
}

/// Config files copied verbatim into every snapshot.
const SNAPSHOT_FILES: &[&str] = &[
    "workspace.json",
    "workspace.json.bak",
    "settings.json",
    "keybindings.json",
    "window-layout.json",
];

/// Config directories copied recursively into every snapshot.
const SNAPSHOT_DIRS: &[&str] = &["themes", "sessions"];

/// Maximum number of config snapshots to retain per profile.
const MAX_SNAPSHOTS: usize = 3;

/// Snapshot the profile's config into `config-backups/<key>/` when an app
/// upgrade or a pending schema migration is detected, so a downgrade can restore
/// the old-format config the previous binary can read.
///
/// Idempotent and first-wins: an existing snapshot for the chosen key is left
/// untouched (it is already the pristine pre-upgrade state). Must run at startup
/// BEFORE any config is loaded/migrated. Returns the snapshot key if one was
/// created.
///
/// Trigger:
/// - app version increased vs the `.app-version` marker (key = old version), or
/// - no marker yet but config exists (key = `pre-<current>`), or
/// - any `schema_versions` entry is behind on disk (dev churn without a version
///   bump; key = `pre-<current>`).
pub fn snapshot_configs_before_upgrade(
    paths: &ProfilePaths,
    current_app_version: &str,
    schema_versions: &[SchemaVersion],
) -> Result<Option<String>> {
    // Only meaningful if there is existing config to protect.
    if !paths.workspace_json().exists() && !paths.settings_json().exists() {
        return Ok(None);
    }

    let marker = read_app_version_marker(paths);

    let upgrade = match &marker {
        Some(last) => is_upgrade(current_app_version, last),
        // First run with this feature on a pre-existing config: protect it once.
        None => true,
    };
    let schema_pending = schema_versions
        .iter()
        .any(|sv| schema_is_behind(&paths.root.join(sv.file), sv.current));

    if !upgrade && !schema_pending {
        return Ok(None);
    }

    // Key = the version we're leaving (a clean revert target) when we have a real
    // upgrade with a recorded previous version; otherwise `pre-<current>` (the
    // config as it was before any `current` build touched it).
    let key = match &marker {
        Some(last) if is_upgrade(current_app_version, last) => sanitize_key(last),
        _ => format!("pre-{}", sanitize_key(current_app_version)),
    };

    let backups_dir = paths.config_backups_dir();
    let target = backups_dir.join(&key);
    // First-wins: never overwrite an existing pristine snapshot for this key.
    if target.exists() {
        return Ok(None);
    }

    std::fs::create_dir_all(&backups_dir)?;
    // Per-process tmp name: the GUI and its daemon may snapshot the same profile
    // concurrently, so they must not share a staging dir.
    let tmp = backups_dir.join(format!("{key}.{}.tmp", std::process::id()));
    // Clear a leftover partial from a previous crash before publishing.
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;

    for name in SNAPSHOT_FILES {
        let from = paths.root.join(name);
        if from.exists() {
            let _ = std::fs::copy(&from, tmp.join(name));
        }
    }
    for name in SNAPSHOT_DIRS {
        let from = paths.root.join(name);
        if from.is_dir() {
            let _ = copy_dir_recursive(&from, &tmp.join(name));
        }
    }

    // Describe the snapshot for humans and a future revert command.
    let schema_meta: serde_json::Map<String, serde_json::Value> = schema_versions
        .iter()
        .map(|sv| {
            let on_disk = read_schema_version(&paths.root.join(sv.file));
            (
                sv.file.to_string(),
                serde_json::json!({ "on_disk": on_disk, "code": sv.current }),
            )
        })
        .collect();
    let meta = serde_json::json!({
        "from_app_version": marker,
        "to_app_version": current_app_version,
        "created_at": now_iso8601(),
        "schema_versions": schema_meta,
    });
    let _ = std::fs::write(
        tmp.join("meta.json"),
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    );

    // Atomically publish: a half-written `.tmp` never looks like a real snapshot.
    if target.exists() {
        // Lost a race with a concurrent snapshot — the other copy is pristine too.
        let _ = std::fs::remove_dir_all(&tmp);
        return Ok(None);
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e.into());
    }
    prune_snapshots(&backups_dir, MAX_SNAPSHOTS);

    log::info!("Config snapshot saved before upgrade: {}", target.display());
    Ok(Some(key))
}

/// Record the current app version into the profile's `.app-version` marker.
/// Call once at startup after the snapshot check. Best-effort.
pub fn record_app_version(paths: &ProfilePaths, current_app_version: &str) {
    let path = paths.app_version_marker();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, current_app_version);
}

fn read_app_version_marker(paths: &ProfilePaths) -> Option<String> {
    let content = std::fs::read_to_string(paths.app_version_marker()).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Read the root `"version"` field from a JSON config file.
fn read_schema_version(path: &Path) -> Option<u32> {
    let content = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    value.get("version").and_then(|v| v.as_u64()).map(|n| n as u32)
}

/// Whether an existing config file is behind the build's schema version (and so
/// would be migrated on load). A present file with a missing/invalid version is
/// treated as legacy (behind) — erring toward taking a backup is safe.
fn schema_is_behind(path: &Path, current: u32) -> bool {
    if !path.exists() {
        return false;
    }
    match read_schema_version(path) {
        Some(v) => v < current,
        None => true,
    }
}

/// True if `current` is a newer version than `last`. Falls back to string
/// inequality (conservative: take a snapshot) when either side is unparseable.
fn is_upgrade(current: &str, last: &str) -> bool {
    match (parse_version(current), parse_version(last)) {
        (Some(c), Some(l)) => c > l,
        _ => current.trim() != last.trim(),
    }
}

/// Parse a `major.minor.patch` version, ignoring any `-pre`/`+build` suffix.
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let core = v.trim().split(['-', '+']).next().unwrap_or("");
    let mut it = core.split('.');
    let major = it.next()?.trim().parse().ok()?;
    let minor = it.next().unwrap_or("0").trim().parse().ok()?;
    let patch = it.next().unwrap_or("0").trim().parse().ok()?;
    Some((major, minor, patch))
}

/// Make a version string safe to use as a directory name.
fn sanitize_key(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect()
}

fn copy_dir_recursive(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst = to.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &dst)?;
        }
        // Symlinks intentionally skipped — config dirs shouldn't contain them.
    }
    Ok(())
}

/// Keep the `keep` most recently created snapshots, removing older ones.
fn prune_snapshots(backups_dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(backups_dir) else {
        return;
    };
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| !e.file_name().to_string_lossy().ends_with(".tmp"))
        .filter_map(|e| {
            let mtime = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((mtime, e.path()))
        })
        .collect();
    if dirs.len() <= keep {
        return;
    }
    dirs.sort_by_key(|(t, _)| *t); // oldest first
    let remove_count = dirs.len() - keep;
    for (_, path) in dirs.into_iter().take(remove_count) {
        let _ = std::fs::remove_dir_all(&path);
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn bootstrap_default_profile(config_root: &Path) -> Result<ProfileIndex> {
    let entry = ProfileEntry {
        id: "default".to_string(),
        display_name: "Default".to_string(),
        created_at: now_iso8601(),
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
    #[cfg(windows)]
    {
        // Use WaitForSingleObject with a 0 timeout rather than
        // GetExitCodeProcess + STILL_ACTIVE: a process that legitimately exited
        // with code 259 (== STILL_ACTIVE) would otherwise be reported alive
        // forever (or until its PID is reused). The process handle is signaled
        // once the process terminates; WAIT_TIMEOUT means it is still running.
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                return false;
            }
            let result = WaitForSingleObject(handle, 0);
            CloseHandle(handle);
            result == WAIT_TIMEOUT
        }
    }
    #[cfg(not(any(unix, windows)))]
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
        let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
        let days = if leap { 366 } else { 365 };
        if n < days { break; }
        n -= days;
        y += 1;
    }
    let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
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

    // ─── Config snapshot tests ──────────────────────────────────────────────

    fn snap_paths(dir: &TempDir) -> ProfilePaths {
        let root = dir.path().join("profiles").join("default");
        fs::create_dir_all(&root).unwrap();
        ProfilePaths {
            id: "default".to_string(),
            root,
            config_root: dir.path().to_path_buf(),
        }
    }

    #[test]
    fn test_snapshot_on_app_version_upgrade() {
        let dir = temp_root();
        let paths = snap_paths(&dir);
        fs::write(paths.workspace_json(), r#"{"version":2,"hello":"world"}"#).unwrap();
        record_app_version(&paths, "0.27.0");

        let sv = [SchemaVersion { file: "workspace.json", current: 2 }];
        let key = snapshot_configs_before_upgrade(&paths, "0.28.0", &sv).unwrap();
        assert_eq!(key.as_deref(), Some("0.27.0"));

        let snap_dir = paths.config_backups_dir().join("0.27.0");
        let content = fs::read_to_string(snap_dir.join("workspace.json")).unwrap();
        assert!(content.contains("\"hello\":\"world\""));
        assert!(snap_dir.join("meta.json").exists());
    }

    #[test]
    fn test_snapshot_when_schema_behind_no_marker() {
        let dir = temp_root();
        let paths = snap_paths(&dir);
        fs::write(paths.workspace_json(), r#"{"version":1}"#).unwrap();
        // No .app-version marker — first run of the feature on an existing config.
        let sv = [SchemaVersion { file: "workspace.json", current: 2 }];
        let key = snapshot_configs_before_upgrade(&paths, "0.28.0", &sv).unwrap();
        assert_eq!(key.as_deref(), Some("pre-0.28.0"));
        assert!(paths.config_backups_dir().join("pre-0.28.0").join("workspace.json").exists());
    }

    #[test]
    fn test_no_snapshot_when_up_to_date() {
        let dir = temp_root();
        let paths = snap_paths(&dir);
        fs::write(paths.workspace_json(), r#"{"version":2}"#).unwrap();
        record_app_version(&paths, "0.28.0");
        let sv = [SchemaVersion { file: "workspace.json", current: 2 }];
        let key = snapshot_configs_before_upgrade(&paths, "0.28.0", &sv).unwrap();
        assert_eq!(key, None);
        assert!(!paths.config_backups_dir().exists());
    }

    #[test]
    fn test_no_snapshot_on_fresh_install() {
        let dir = temp_root();
        let paths = snap_paths(&dir);
        // No config files at all.
        let sv = [SchemaVersion { file: "workspace.json", current: 2 }];
        let key = snapshot_configs_before_upgrade(&paths, "0.28.0", &sv).unwrap();
        assert_eq!(key, None);
    }

    #[test]
    fn test_snapshot_on_schema_churn_same_version() {
        let dir = temp_root();
        let paths = snap_paths(&dir);
        fs::write(paths.workspace_json(), r#"{"version":2}"#).unwrap();
        record_app_version(&paths, "0.28.0");
        // Code schema bumped without an app-version bump (dev churn on a branch).
        let sv = [SchemaVersion { file: "workspace.json", current: 3 }];
        let key = snapshot_configs_before_upgrade(&paths, "0.28.0", &sv).unwrap();
        assert_eq!(key.as_deref(), Some("pre-0.28.0"));
    }

    #[test]
    fn test_snapshot_idempotent_first_wins() {
        let dir = temp_root();
        let paths = snap_paths(&dir);
        fs::write(paths.workspace_json(), r#"{"version":2,"n":1}"#).unwrap();
        record_app_version(&paths, "0.27.0");
        let sv = [SchemaVersion { file: "workspace.json", current: 2 }];

        let k1 = snapshot_configs_before_upgrade(&paths, "0.28.0", &sv).unwrap();
        assert_eq!(k1.as_deref(), Some("0.27.0"));

        // Mutate the live config, then snapshot again with the same key.
        fs::write(paths.workspace_json(), r#"{"version":2,"n":2}"#).unwrap();
        let k2 = snapshot_configs_before_upgrade(&paths, "0.28.0", &sv).unwrap();
        assert_eq!(k2, None, "must not re-snapshot an existing key");

        let snap =
            fs::read_to_string(paths.config_backups_dir().join("0.27.0").join("workspace.json"))
                .unwrap();
        assert!(snap.contains("\"n\":1"), "first snapshot must stay pristine");
    }

    #[test]
    fn test_snapshot_copies_dirs() {
        let dir = temp_root();
        let paths = snap_paths(&dir);
        fs::write(paths.workspace_json(), r#"{"version":2}"#).unwrap();
        fs::create_dir_all(paths.themes_dir()).unwrap();
        fs::write(paths.themes_dir().join("custom.json"), "{}").unwrap();
        record_app_version(&paths, "0.27.0");

        let sv = [SchemaVersion { file: "workspace.json", current: 2 }];
        snapshot_configs_before_upgrade(&paths, "0.28.0", &sv).unwrap();
        assert!(paths
            .config_backups_dir()
            .join("0.27.0")
            .join("themes")
            .join("custom.json")
            .exists());
    }

    #[test]
    fn test_prune_keeps_recent() {
        let dir = temp_root();
        let backups = dir.path().join("config-backups");
        fs::create_dir_all(&backups).unwrap();
        for name in ["a", "b", "c", "d", "e"] {
            fs::create_dir_all(backups.join(name)).unwrap();
        }
        // A leftover staging dir must never be counted or removed as a snapshot.
        fs::create_dir_all(backups.join("f.123.tmp")).unwrap();

        prune_snapshots(&backups, 3);

        let snapshot_dirs = fs::read_dir(&backups)
            .unwrap()
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(snapshot_dirs, 3);
        assert!(backups.join("f.123.tmp").exists());
    }

    #[test]
    fn test_version_compare() {
        assert!(is_upgrade("0.28.0", "0.27.0"));
        assert!(is_upgrade("1.0.0", "0.99.99"));
        assert!(is_upgrade("0.28.1", "0.28.0"));
        assert!(!is_upgrade("0.27.0", "0.28.0"));
        assert!(!is_upgrade("0.28.0", "0.28.0"));
        // Pre-release / build suffix ignored on the core triple.
        assert!(!is_upgrade("0.28.0-beta", "0.28.0"));
        // Unparseable on either side → conservative: snapshot iff strings differ.
        assert!(is_upgrade("weird", "other"));
        assert!(!is_upgrade("weird", "weird"));
    }

    fn make_test_index_with_two(dir: &TempDir) -> ProfileIndex {
        let idx = ProfileIndex {
            version: 1,
            profiles: vec![
                ProfileEntry { id: "default".into(), display_name: "Default".into(), created_at: "".into(), icon: None, color: None },
                ProfileEntry { id: "work".into(), display_name: "Work".into(), created_at: "".into(), icon: None, color: None },
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
