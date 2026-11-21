//! Daemon side of Claude remote outside the HTTP routes: the environment new
//! panes get, and the preload file that environment points Bun at.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::ai_proxy::AiProxyPoll;
use okena_terminal::backend::LaunchEnvironment;
use okena_workspace::persistence::AppSettings;
use parking_lot::Mutex;

const PRELOAD_JS: &str = include_str!("../../../integrations/claude-code/okena-remote/preload.js");
const PRELOAD_DIR: &str = "claude-remote";
const PRELOAD_FILE: &str = "preload.js";

/// How often the preload is re-ensured while the feature is on. Panes keep the
/// path for their lifetime, and temp cleaners may remove the file meanwhile.
const PRELOAD_REFRESH_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// A candidate directory for the preload: `base` must exist, `dirs` are
/// created (or reused) below it and must each be owned by us and private.
#[derive(Clone, Debug, PartialEq)]
struct PreloadLocation {
    base: PathBuf,
    dirs: Vec<String>,
}

impl PreloadLocation {
    fn file(&self) -> PathBuf {
        let mut path = self.base.clone();
        path.extend(&self.dirs);
        path.join(PRELOAD_FILE)
    }
}

/// What the pane environment needs from the daemon's process: resolved once at
/// startup, applied at every settings change.
pub struct PaneEnv {
    okena_bin: Option<PathBuf>,
    /// In order of preference.
    preload_locations: Vec<PreloadLocation>,
    /// Exported to panes so the hooks reach this daemon: a tmux server shared
    /// by several profiles hands panes the environment of the first one.
    profile_id: Option<String>,
    ai_proxy: AiProxyPoll,
}

impl PaneEnv {
    pub fn resolve() -> Self {
        let profile = okena_core::profiles::try_current();
        let preload_locations = match profile {
            Some(profile) => preload_locations(
                &profile.id,
                &profile.root,
                std::env::var_os("XDG_RUNTIME_DIR"),
                okena_core::profiles::cache_root(),
                std::env::var_os("TMPDIR"),
            ),
            None => Vec::new(),
        };
        let okena_bin = okena_remote_server::local::remember_current_executable()
            .ok()
            .and_then(|exe| okena_bin_for_exe(&exe));
        Self {
            okena_bin: match (okena_bin, profile) {
                (Some(okena_bin), Some(profile)) => {
                    Some(stable_okena_bin_or_original(okena_bin, &profile.root))
                }
                (okena_bin, _) => okena_bin,
            },
            preload_locations,
            profile_id: profile.map(|profile| profile.id.clone()),
            ai_proxy: AiProxyPoll::default(),
        }
    }

    /// The accounts poll that the shells routed through the AI proxy wake.
    pub(crate) fn ai_proxy(&self) -> &AiProxyPoll {
        &self.ai_proxy
    }

    /// The environment for PTYs spawned from now on: Claude account isolation,
    /// Claude remote (set when the setting is on and the preload could be
    /// written, removed otherwise) and the AI proxy URL of the terminal's
    /// project ([`crate::ai_proxy::launch_environment`]).
    pub fn launch_environment(&self, settings: &AppSettings) -> LaunchEnvironment {
        let mut env = okena_workspace::claude_env::claude_pty_env_for_settings(settings);
        let enabled = cfg!(unix) && settings.claude_remote;
        if enabled && self.okena_bin.is_none() {
            log::warn!(
                "[claude-remote] unavailable: Claude remote needs the `okena` binary next to the daemon"
            );
        }
        // Without the file, BUN_OPTIONS would make every Bun program in the pane fail.
        let preload = if enabled && self.okena_bin.is_some() {
            self.ensure_preload()
        } else {
            None
        };
        env.extend(okena_workspace::claude_remote_env::claude_remote_pty_env(
            preload.is_some(),
            settings.claude_remote_channel,
            preload.as_deref(),
            self.okena_bin.as_deref(),
            std::env::var("BUN_OPTIONS").ok().as_deref(),
        ));
        if let (Some(_), Some(profile_id)) = (&preload, &self.profile_id) {
            env.push(("OKENA_PROFILE".to_string(), Some(profile_id.clone())));
        }
        crate::ai_proxy::launch_environment(settings, env, &self.ai_proxy)
    }

    /// Write the preload to the first location that works and return its path.
    fn ensure_preload(&self) -> Option<PathBuf> {
        for location in &self.preload_locations {
            match ensure_preload_at(location, PRELOAD_JS) {
                Ok(path) => return Some(path),
                Err(error) => log::debug!(
                    "[claude-remote] preload location {:?} not usable: {error}",
                    location.file()
                ),
            }
        }
        log::warn!(
            "[claude-remote] unavailable: no private directory without whitespace for the preload (tried {:?})",
            self.preload_locations
                .iter()
                .map(PreloadLocation::file)
                .collect::<Vec<_>>()
        );
        None
    }
}

/// Keep the preload in place while the feature is on, so Bun in panes created
/// earlier still finds it. Runs on the daemon's `LocalSet` until shutdown.
pub async fn run_preload_refresh(pane_env: Arc<PaneEnv>, settings: Arc<Mutex<AppSettings>>) {
    let mut interval = tokio::time::interval(PRELOAD_REFRESH_INTERVAL);
    // The first tick is immediate; startup has just written the file.
    interval.tick().await;
    loop {
        interval.tick().await;
        let enabled = cfg!(unix) && settings.lock().claude_remote;
        if enabled && pane_env.okena_bin.is_some() {
            pane_env.ensure_preload();
        }
    }
}

/// Where the preload may live, best first: the profile's config dir (durable,
/// user-owned), then the per-user runtime dir, then the per-user `TMPDIR`
/// (macOS). Never the shared `/tmp`. Bun splits `BUN_OPTIONS` on whitespace
/// without quoting, so paths with whitespace are dropped.
fn preload_locations(
    profile_id: &str,
    profile_root: &Path,
    xdg_runtime_dir: Option<OsString>,
    cache_root: Option<PathBuf>,
    tmpdir: Option<OsString>,
) -> Vec<PreloadLocation> {
    let non_empty = |value: Option<OsString>| value.filter(|value| !value.is_empty());
    let mut locations = vec![PreloadLocation {
        base: profile_root.to_path_buf(),
        dirs: vec![PRELOAD_DIR.to_string()],
    }];
    if let Some(runtime_dir) = non_empty(xdg_runtime_dir) {
        locations.push(PreloadLocation {
            base: PathBuf::from(runtime_dir),
            dirs: vec![
                "okena".to_string(),
                profile_id.to_string(),
                PRELOAD_DIR.to_string(),
            ],
        });
    }
    // Before $TMPDIR: macOS deletes its files after a few days unused, and a
    // pane whose preload is gone cannot start any Bun program.
    if let Some(cache_root) = cache_root {
        locations.push(PreloadLocation {
            base: cache_root,
            dirs: vec![profile_id.to_string(), PRELOAD_DIR.to_string()],
        });
    }
    if let Some(tmpdir) = non_empty(tmpdir) {
        locations.push(PreloadLocation {
            base: PathBuf::from(tmpdir),
            dirs: vec![format!("okena-{profile_id}"), PRELOAD_DIR.to_string()],
        });
    }
    locations.retain(|location| {
        location
            .file()
            .to_str()
            .is_some_and(|path| !path.chars().any(char::is_whitespace))
    });
    locations
}

/// The `okena` binary for a daemon running as `exe`: the daemon itself when it
/// is `okena`, else an `okena` next to it. A standalone `okena-daemon` usually
/// has none.
fn okena_bin_for_exe(exe: &Path) -> Option<PathBuf> {
    if exe.file_stem().is_some_and(|stem| stem == "okena") {
        return Some(exe.to_path_buf());
    }
    let sibling = exe.with_file_name(format!("okena{}", std::env::consts::EXE_SUFFIX));
    sibling.is_file().then_some(sibling)
}

/// `okena_bin` at a path that outlives the build it came from, else
/// `okena_bin` itself. A pane keeps `OKENA_BIN` for its whole life, which
/// spans daemon restarts, so a removed build directory would break every
/// `claude` started in it.
#[cfg(target_os = "linux")]
fn stable_okena_bin_or_original(okena_bin: PathBuf, profile_root: &Path) -> PathBuf {
    // SAFETY: `geteuid(2)` takes no arguments, dereferences no pointers, and is
    // documented as never failing.
    let euid = unsafe { libc::geteuid() };
    match install_stable_okena_bin(&okena_bin, &profile_root.join(STABLE_BIN_DIR), euid) {
        Ok(stable) => stable,
        Err(error) => {
            log::warn!(
                "[claude-remote] panes get {okena_bin:?}: no stable copy in the profile: {error}"
            );
            okena_bin
        }
    }
}

/// Only Linux: a macOS app bundle's binary is not known to run when copied out
/// of the bundle, and its path survives updates anyway.
#[cfg(not(target_os = "linux"))]
fn stable_okena_bin_or_original(okena_bin: PathBuf, _profile_root: &Path) -> PathBuf {
    okena_bin
}

#[cfg(target_os = "linux")]
const STABLE_BIN_DIR: &str = "bin";

/// Put `source` at `<dir>/okena`: a hard link (same file, no copy), or a copy
/// across file systems or when the link is refused. Replaced by a rename, so a
/// hook running the previous one keeps its file. The directory must be private:
/// every `claude` in a pane runs this file.
#[cfg(target_os = "linux")]
fn install_stable_okena_bin(source: &Path, dir: &Path, euid: u32) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt as _;

    create_private_dir(dir, euid)?;
    remove_interrupted_installs(dir);
    let target = dir.join("okena");
    // A hard link to a symlink would link the symlink, relative target and all.
    let source = std::fs::canonicalize(source)?;
    let source_meta = std::fs::metadata(&source)?;
    if std::fs::symlink_metadata(&target).is_ok_and(|current| {
        let same_file = current.dev() == source_meta.dev() && current.ino() == source_meta.ino();
        // A copy carries its source's modification time (`copy_stamped`).
        let same_copy = current.is_file()
            && current.len() == source_meta.len()
            && current.modified().ok() == source_meta.modified().ok();
        same_file || same_copy
    }) {
        return Ok(target);
    }
    let temp = dir.join(format!("okena.{}.tmp", std::process::id()));
    let installed = std::fs::hard_link(&source, &temp)
        .or_else(|_| copy_stamped(&source, &source_meta, &temp))
        .and_then(|()| std::fs::rename(&temp, &target));
    if let Err(error) = installed {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(target)
}

/// Copy `source` with its modification time, so the next start can tell this
/// copy is current, and with a mode only we can write.
#[cfg(target_os = "linux")]
fn copy_stamped(
    source: &Path,
    source_meta: &std::fs::Metadata,
    temp: &Path,
) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::copy(source, temp)?;
    // Before opening: a copy of a read-only install cannot be opened to write.
    std::fs::set_permissions(temp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::File::options()
        .write(true)
        .open(temp)?
        .set_modified(source_meta.modified()?)
}

/// Temporary files a daemon killed mid-copy left behind. The instance lock
/// makes this daemon the only writer of the profile.
#[cfg(target_os = "linux")]
fn remove_interrupted_installs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("okena.") && name.ends_with(".tmp") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(unix)]
fn ensure_preload_at(location: &PreloadLocation, contents: &str) -> std::io::Result<PathBuf> {
    // SAFETY: `geteuid(2)` takes no arguments, dereferences no pointers, and is
    // documented as never failing.
    let euid = unsafe { libc::geteuid() };
    ensure_preload_at_as(location, contents, euid)
}

#[cfg(not(unix))]
fn ensure_preload_at(_location: &PreloadLocation, _contents: &str) -> std::io::Result<PathBuf> {
    Err(std::io::Error::other("Claude remote is Unix only"))
}

/// Create the location's directories and write `contents` there unless it is
/// already there. Panes load the file on every Bun start, so it is replaced
/// atomically.
#[cfg(unix)]
fn ensure_preload_at_as(
    location: &PreloadLocation,
    contents: &str,
    euid: u32,
) -> std::io::Result<PathBuf> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut dir = location.base.clone();
    for component in &location.dirs {
        dir.push(component);
        create_private_dir(&dir, euid)?;
    }

    let path = dir.join(PRELOAD_FILE);
    if std::fs::read(&path).is_ok_and(|current| current == contents.as_bytes()) {
        return Ok(path);
    }
    let temp = dir.join(format!("{PRELOAD_FILE}.{}.tmp", std::process::id()));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(&temp)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .and_then(|()| std::fs::rename(&temp, &path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(path)
}

/// Code in these directories runs inside every `claude`: refuse one that is
/// a symlink, belongs to someone else, or others can write to.
#[cfg(unix)]
fn create_private_dir(dir: &Path, euid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};

    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(dir)?;
    if metadata.is_dir() && dir_is_ours_and_private(metadata.uid(), metadata.mode(), euid) {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{dir:?} is not a directory owned by us and writable only by us"
        )))
    }
}

/// Same rule as the daemon socket directory in `okena-remote-server`'s
/// `local.rs`, which keeps its copy private.
#[cfg(unix)]
fn dir_is_ours_and_private(owner_uid: u32, mode: u32, euid: u32) -> bool {
    owner_uid == euid && mode & 0o022 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "okena-claude-remote-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn location(base: &Path, dirs: &[&str]) -> PreloadLocation {
        PreloadLocation {
            base: base.to_path_buf(),
            dirs: dirs.iter().map(|dir| dir.to_string()).collect(),
        }
    }

    #[cfg(unix)]
    fn euid() -> u32 {
        // SAFETY: `geteuid(2)` takes no arguments and never fails.
        unsafe { libc::geteuid() }
    }

    #[test]
    fn locations_are_profile_dir_then_runtime_dir_then_cache_then_tmpdir() {
        let locations = preload_locations(
            "work",
            Path::new("/home/u/.config/okena/profiles/work"),
            Some("/run/user/1000".into()),
            Some(PathBuf::from("/home/u/.cache/okena")),
            Some("/var/folders/xy/T/".into()),
        );

        assert_eq!(
            locations,
            vec![
                location(
                    Path::new("/home/u/.config/okena/profiles/work"),
                    &["claude-remote"]
                ),
                location(
                    Path::new("/run/user/1000"),
                    &["okena", "work", "claude-remote"]
                ),
                location(
                    Path::new("/home/u/.cache/okena"),
                    &["work", "claude-remote"]
                ),
                location(
                    Path::new("/var/folders/xy/T/"),
                    &["okena-work", "claude-remote"]
                ),
            ]
        );
        assert_eq!(
            locations[1].file(),
            Path::new("/run/user/1000/okena/work/claude-remote/preload.js")
        );
    }

    #[test]
    fn locations_with_whitespace_or_unset_bases_are_dropped() {
        let locations = preload_locations(
            "default",
            Path::new("/Users/u/Library/Application Support/okena/profiles/default"),
            Some("".into()),
            Some(PathBuf::from("/Users/u/Library/Caches/okena")),
            Some("/var/folders/xy/T".into()),
        );

        assert_eq!(
            locations,
            vec![
                location(
                    Path::new("/Users/u/Library/Caches/okena"),
                    &["default", "claude-remote"]
                ),
                location(
                    Path::new("/var/folders/xy/T"),
                    &["okena-default", "claude-remote"]
                ),
            ]
        );
        assert!(preload_locations("default", Path::new("/a b"), None, None, None).is_empty());
    }

    #[test]
    fn the_okena_binary_is_used_directly() {
        let dir = temp_dir("self");
        let exe = dir.join("okena");

        assert_eq!(okena_bin_for_exe(&exe), Some(exe));
        std::fs::remove_dir_all(dir).expect("remove temp dir");
    }

    #[test]
    fn another_binary_uses_an_okena_sibling() {
        let dir = temp_dir("sibling");
        let sibling = dir.join(format!("okena{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&sibling, "").expect("write sibling");

        assert_eq!(okena_bin_for_exe(&dir.join("okena-daemon")), Some(sibling));
        std::fs::remove_dir_all(dir).expect("remove temp dir");
    }

    #[test]
    fn another_binary_without_an_okena_sibling_has_none() {
        let dir = temp_dir("none");

        assert_eq!(okena_bin_for_exe(&dir.join("okena-daemon")), None);
        std::fs::remove_dir_all(dir).expect("remove temp dir");
    }

    /// A fake `okena` build: an executable file in its own build directory.
    #[cfg(target_os = "linux")]
    fn fake_build(root: &Path, name: &str, contents: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let build = root.join(name);
        std::fs::create_dir_all(&build).expect("create build dir");
        let bin = build.join("okena");
        std::fs::write(&bin, contents).expect("write fake okena");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .expect("make fake okena executable");
        bin
    }

    /// Read without running it: executing a file this process just wrote races
    /// other tests' forks for its write descriptor (`ETXTBSY`).
    #[cfg(target_os = "linux")]
    fn executable_contents(path: &Path) -> String {
        use std::os::unix::fs::PermissionsExt as _;

        let mode = std::fs::metadata(path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "{path:?} is executable");
        std::fs::read_to_string(path).expect("read")
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_okena_binary_panes_get_outlives_the_build_it_came_from() {
        let root = temp_dir("stable-bin");
        let profile = root.join("profile");
        std::fs::create_dir_all(&profile).expect("create profile dir");
        let build = fake_build(&root, "build-1", "build 1");

        let stable = stable_okena_bin_or_original(build.clone(), &profile);
        assert_eq!(stable, profile.join(STABLE_BIN_DIR).join("okena"));
        std::fs::remove_dir_all(build.parent().unwrap()).expect("remove build 1");
        assert_eq!(executable_contents(&stable), "build 1");

        // The next daemon start, from another build, replaces it at the same path.
        let next = fake_build(&root, "build-2", "build 2");
        assert_eq!(stable_okena_bin_or_original(next, &profile), stable);
        assert_eq!(executable_contents(&stable), "build 2");
        std::fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_current_copy_is_kept_and_a_stale_one_replaced() {
        use std::os::unix::fs::MetadataExt as _;

        let root = temp_dir("stable-bin-copy");
        let dir = root.join(STABLE_BIN_DIR);
        create_private_dir(&dir, euid()).expect("create bin dir");
        let build = fake_build(&root, "build", "the build");
        let target = dir.join("okena");
        // What another file system leaves: a copy instead of a link.
        copy_stamped(&build, &std::fs::metadata(&build).unwrap(), &target).expect("copy");
        let copy_inode = std::fs::metadata(&target).unwrap().ino();
        // A daemon killed mid-copy.
        let interrupted = dir.join("okena.4242.tmp");
        std::fs::write(&interrupted, "partial").expect("write partial copy");

        install_stable_okena_bin(&build, &dir, euid()).expect("install");
        assert_eq!(std::fs::metadata(&target).unwrap().ino(), copy_inode);
        assert!(!interrupted.exists());

        // Same length, another build: a different modification time.
        std::fs::write(&build, "new build").expect("rebuild");
        let rebuilt = std::fs::File::options().write(true).open(&build).unwrap();
        rebuilt
            .set_modified(std::time::SystemTime::UNIX_EPOCH)
            .expect("set mtime");
        std::fs::write(&target, "old build").expect("old copy");
        install_stable_okena_bin(&build, &dir, euid()).expect("install");
        assert_eq!(executable_contents(&target), "new build");
        std::fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_symlinked_okena_is_installed_as_the_file_it_points_to() {
        let root = temp_dir("stable-bin-link");
        let build = fake_build(&root, "build", "the build");
        let link = root.join("okena-link");
        std::os::unix::fs::symlink(&build, &link).expect("symlink okena");

        let stable = install_stable_okena_bin(&link, &root.join(STABLE_BIN_DIR), euid())
            .expect("install stable okena");
        std::fs::remove_dir_all(build.parent().unwrap()).expect("remove build");
        assert!(!std::fs::symlink_metadata(&stable).unwrap().is_symlink());
        assert_eq!(executable_contents(&stable), "the build");
        std::fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn panes_get_the_build_path_when_the_profile_cannot_hold_a_copy() {
        let root = temp_dir("stable-bin-refused");
        let build = fake_build(&root, "build", "the build");
        // A file where the private bin directory would go.
        let profile = root.join("profile");
        std::fs::create_dir_all(&profile).expect("create profile dir");
        std::fs::write(profile.join(STABLE_BIN_DIR), "").expect("block bin dir");

        assert_eq!(stable_okena_bin_or_original(build.clone(), &profile), build);
        std::fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[test]
    fn the_embedded_preload_is_the_integration_script() {
        assert!(PRELOAD_JS.contains("function okenaClaudeArgs("));
    }

    #[cfg(unix)]
    #[test]
    fn preload_is_written_private_and_replaced_only_when_it_differs() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = temp_dir("write");
        let location = location(&root, &["okena", "p", PRELOAD_DIR]);
        let dir = root.join("okena").join("p").join(PRELOAD_DIR);

        let path = ensure_preload_at_as(&location, "one", euid()).expect("first write");
        assert_eq!(path, location.file());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "one");
        for created in [
            root.join("okena"),
            root.join("okena").join("p"),
            dir.clone(),
        ] {
            let mode = std::fs::metadata(&created)
                .expect("dir metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "{created:?}");
        }

        let modified = std::fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("mtime");
        std::thread::sleep(std::time::Duration::from_millis(20));
        ensure_preload_at_as(&location, "one", euid()).expect("same content");
        let unchanged = std::fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("mtime");
        assert_eq!(modified, unchanged);

        ensure_preload_at_as(&location, "two", euid()).expect("new content");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "two");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(leftovers, vec![OsString::from(PRELOAD_FILE)]);

        std::fs::remove_file(&path).expect("simulate a temp cleaner");
        ensure_preload_at_as(&location, "two", euid()).expect("rewrite");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "two");

        std::fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn a_reused_directory_others_can_write_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = temp_dir("shared");
        let shared = root.join("okena-p");
        std::fs::create_dir(&shared).expect("create shared dir");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).expect("chmod");

        let location = location(&root, &["okena-p", PRELOAD_DIR]);
        assert!(ensure_preload_at_as(&location, "x", euid()).is_err());
        assert!(!shared.join(PRELOAD_DIR).exists());
        std::fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_owned_by_someone_else_is_refused() {
        let root = temp_dir("foreign");
        let location = location(&root, &[PRELOAD_DIR]);

        assert!(ensure_preload_at_as(&location, "x", euid().wrapping_add(1)).is_err());
        assert!(!location.file().exists());
        std::fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_is_refused() {
        let root = temp_dir("symlink");
        let target = temp_dir("symlink-target");
        std::os::unix::fs::symlink(&target, root.join(PRELOAD_DIR)).expect("symlink");

        let location = location(&root, &[PRELOAD_DIR]);
        assert!(ensure_preload_at_as(&location, "x", euid()).is_err());
        assert!(!target.join(PRELOAD_FILE).exists());
        std::fs::remove_dir_all(root).expect("remove temp dir");
        std::fs::remove_dir_all(target).expect("remove temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn a_later_location_is_used_when_an_earlier_one_fails() {
        let root = temp_dir("fallback");
        let missing_base = root.join("missing");
        let pane_env = PaneEnv {
            okena_bin: Some(PathBuf::from("/opt/okena/okena")),
            preload_locations: vec![
                location(&missing_base, &[PRELOAD_DIR]),
                location(&root, &["okena-p", PRELOAD_DIR]),
            ],
            profile_id: Some("work".to_string()),
            ai_proxy: AiProxyPoll::default(),
        };

        assert_eq!(
            pane_env.ensure_preload(),
            Some(root.join("okena-p").join(PRELOAD_DIR).join(PRELOAD_FILE))
        );
        assert!(!missing_base.exists());
        std::fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[test]
    fn claude_remote_off_removes_the_variables() {
        let root = temp_dir("off");
        let pane_env = PaneEnv {
            okena_bin: Some(PathBuf::from("/opt/okena/okena")),
            preload_locations: vec![location(&root, &[PRELOAD_DIR])],
            profile_id: Some("work".to_string()),
            ai_proxy: AiProxyPoll::default(),
        };
        let settings = AppSettings::default();

        let env = pane_env.launch_environment(&settings)(None);

        let mut expected = okena_workspace::claude_env::claude_pty_env_for_settings(&settings);
        expected.extend(okena_workspace::claude_remote_env::claude_remote_pty_env(
            false,
            true,
            None,
            None,
            std::env::var("BUN_OPTIONS").ok().as_deref(),
        ));
        assert_eq!(env, expected);
        assert!(env.contains(&("OKENA_BIN".to_string(), None)));
        assert!(!root.join(PRELOAD_DIR).exists());
        std::fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn claude_remote_on_writes_the_preload_and_points_bun_at_it() {
        let root = temp_dir("on");
        let pane_env = PaneEnv {
            okena_bin: Some(PathBuf::from("/opt/okena/okena")),
            preload_locations: vec![location(&root, &[PRELOAD_DIR])],
            profile_id: Some("work".to_string()),
            ai_proxy: AiProxyPoll::default(),
        };
        let mut settings = AppSettings::default();
        settings.claude_remote = true;

        let env = pane_env.launch_environment(&settings)(None);

        let preload = root.join(PRELOAD_DIR).join(PRELOAD_FILE);
        assert_eq!(
            std::fs::read_to_string(&preload).expect("preload written"),
            PRELOAD_JS
        );
        let bun_options = env
            .iter()
            .find(|(key, _)| key == "BUN_OPTIONS")
            .and_then(|(_, value)| value.clone())
            .expect("BUN_OPTIONS set");
        assert!(bun_options.starts_with(&format!("--preload={}", preload.display())));
        assert!(env.contains(&(
            "OKENA_CLAUDE_REMOTE_CHANNEL".to_string(),
            Some("1".to_string())
        )));
        assert!(env.contains(&("OKENA_PROFILE".to_string(), Some("work".to_string()))));
        std::fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn claude_remote_on_without_a_usable_location_removes_the_variables() {
        let pane_env = PaneEnv {
            okena_bin: Some(PathBuf::from("/opt/okena/okena")),
            preload_locations: Vec::new(),
            profile_id: Some("work".to_string()),
            ai_proxy: AiProxyPoll::default(),
        };
        let mut settings = AppSettings::default();
        settings.claude_remote = true;

        let env = pane_env.launch_environment(&settings)(None);

        assert!(env.contains(&("OKENA_BIN".to_string(), None)));
        assert!(env.contains(&("OKENA_CLAUDE_REMOTE_CHANNEL".to_string(), None)));
    }
}
