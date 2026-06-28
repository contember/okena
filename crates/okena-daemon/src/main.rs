//! Standalone, GPUI-free headless daemon binary.
//!
//! This is the runnable counterpart of `okena --headless`: it boots the headless
//! server by constructing and running [`okena_daemon_core::DaemonCore`], with no
//! GPUI anywhere in scope. It mirrors the GUI's `run_headless` bootstrap
//! (`src/main.rs`), but loads settings via the gpui-free
//! [`okena_workspace::settings::load_settings`] instead of GPUI's
//! `settings::init_settings(cx)`.
//!
//! Wiring the desktop app's `spawn_daemon` to launch THIS binary (instead of
//! `okena --headless`) is a follow-up — the desktop-as-client phase.

use std::io::Write;
use std::net::IpAddr;

use anyhow::Context;
use okena_daemon_core::{DaemonCore, DaemonParams};
use okena_workspace::persistence;
use okena_workspace::settings::load_settings;

/// Writes to both stderr and a log file simultaneously (mirrors the GUI's
/// `TeeWriter`). The daemon is spawned by the GUI with inherited stdio, so its
/// stderr lands in the GUI's terminal; teeing to a dedicated `okena-daemon.log`
/// keeps a durable record (incl. panics) separate from the GUI's `okena.log`.
struct TeeWriter {
    stderr: std::io::Stderr,
    file: std::fs::File,
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = self.stderr.write_all(buf);
        self.file.write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let _ = self.stderr.flush();
        self.file.flush()
    }
}

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--version") {
        println!("okena-daemon {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // 0. Resolve the active profile (env-only: the spawning desktop propagates
    //    OKENA_PROFILE; a standalone daemon reads the default / last-used). This
    //    makes get_config_dir() resolve the SAME profile dir the desktop uses, so
    //    both read/write the same workspace. Must run before logging (which uses
    //    the profile's paths) and before load_settings()/load_workspace().
    let profile_paths = match okena_core::profiles::resolve_active_profile(None) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    // SAFETY: called before any threads are spawned; no concurrent env access.
    unsafe { std::env::set_var("OKENA_PROFILE", &profile_paths.id) };
    // Capture the daemon log path BEFORE moving `profile_paths` into the global.
    let daemon_log = profile_paths.root.join("okena-daemon.log");
    okena_core::profiles::init_profile(profile_paths);
    if let Err(e) =
        okena_core::profiles::migrate_legacy_layout_if_needed(okena_core::profiles::current())
    {
        eprintln!("Warning: profile migration failed: {e}");
    }

    // Snapshot the existing config BEFORE load_settings()/load_workspace() so an
    // upgrade can be reverted to an old-format config the previous binary reads.
    // Shares the marker + config-backups dir with the GUI (first-wins, idempotent).
    {
        use okena_core::profiles::SchemaVersion;
        use okena_workspace::persistence::{
            SETTINGS_VERSION, WINDOW_LAYOUT_VERSION, WORKSPACE_VERSION,
        };
        let schema_versions = [
            SchemaVersion {
                file: "workspace.json",
                current: WORKSPACE_VERSION,
            },
            SchemaVersion {
                file: "settings.json",
                current: SETTINGS_VERSION,
            },
            SchemaVersion {
                file: "window-layout.json",
                current: WINDOW_LAYOUT_VERSION,
            },
        ];
        if let Err(e) = okena_core::profiles::snapshot_configs_before_upgrade(
            okena_core::profiles::current(),
            env!("CARGO_PKG_VERSION"),
            &schema_versions,
        ) {
            eprintln!("Warning: config snapshot failed: {e}");
        }
        okena_core::profiles::record_app_version(
            okena_core::profiles::current(),
            env!("CARGO_PKG_VERSION"),
        );
    }

    // 1. Logging: env_logger driven by RUST_LOG (default "info"), teed to
    //    `okena-daemon.log` so the daemon's output + panics survive even though
    //    its stderr is the GUI's inherited terminal. Best-effort: if the file
    //    can't be opened we fall back to plain stderr.
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if let Ok(file) = std::fs::File::create(&daemon_log) {
        builder.target(env_logger::fmt::Target::Pipe(Box::new(TeeWriter {
            stderr: std::io::stderr(),
            file,
        })));
    }
    builder.init();

    // Log panics (with a backtrace) to okena-daemon.log. Without this a panic on
    // the awaited path (command loop / startup) aborts the process leaving only
    // the client's "connection refused" — no cause. The default hook still runs
    // after, preserving normal stderr output.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!("daemon PANIC: {info}\n{backtrace}");
        default_hook(info);
    }));

    // 1b. Self-restart handoff: a daemon restarting itself spawns this process
    //     with `--await-pid <old_pid>` (see okena_remote_server::routes::restart).
    //     Wait for the outgoing daemon to exit BEFORE constructing DaemonCore,
    //     which acquires the instance lock (fail-fast against a live PID) and
    //     binds a port (the old one may linger in TIME_WAIT). Bounded so a wedged
    //     predecessor doesn't hang the new daemon forever; on timeout we proceed
    //     anyway and let the lock acquisition surface the real error.
    if let Some(old_pid) = okena_remote_server::local::parse_await_pid(std::env::args()) {
        log::info!("restart: waiting for outgoing daemon (pid {old_pid}) to exit");
        let exited = okena_remote_server::local::wait_for_pid_exit(
            old_pid,
            std::time::Duration::from_secs(10),
        );
        if !exited {
            log::warn!(
                "restart: outgoing daemon (pid {old_pid}) still alive after 10s; \
                 proceeding (lock acquisition will fail if it truly holds the lock)"
            );
        }
    }

    // 2. Optional `--listen <ip>` override. Parsing is intentionally minimal and
    //    dependency-free; the error messages mirror the GUI's `src/main.rs`.
    let listen_override: Option<IpAddr> = parse_listen_override();

    // 3. Load settings (gpui-free), then the workspace — falling back to the
    //    default workspace on error, logging like the GUI's `run_headless`.
    let settings = load_settings();
    let session_backend = settings.session_backend; // `Copy`
    let workspace_data = persistence::load_workspace(session_backend).unwrap_or_else(|e| {
        log::error!(
            "Failed to load workspace: {}. A backup may have been saved to {:?}. Using default workspace.",
            e,
            persistence::get_workspace_path().with_extension("json.bak")
        );
        persistence::default_workspace()
    });

    // 4. Resolve the listen address: the `--listen` override wins; otherwise the
    //    settings value parsed as an `IpAddr`; otherwise loopback.
    let listen_addr: IpAddr = listen_override.unwrap_or_else(|| {
        let configured = settings.remote_listen_address.trim();
        match configured.parse::<IpAddr>() {
            Ok(addr) => addr,
            Err(_) => {
                if !configured.is_empty() {
                    log::warn!(
                        "Invalid remote_listen_address in settings ({configured:?}); falling back to 127.0.0.1"
                    );
                }
                IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            }
        }
    });

    // 5. Build params (read TLS out before moving `settings`) and run. `run`
    //    blocks until the bridge closes or ctrl-c arrives — that is expected,
    //    the daemon is UI-owned.
    //
    // TLS policy by deployment mode (architecture §1): a LOCAL loopback daemon
    // (spawned by the desktop on 127.0.0.1) serves plain http — loopback is
    // trusted, TLS there is pure overhead and adds handshake fragility (the
    // client connects plain and would otherwise TOFU-upgrade). Only a STANDALONE
    // server bound to a non-loopback address honors `remote_tls_enabled` for
    // off-host clients.
    let tls_enabled = !listen_addr.is_loopback() && settings.remote_tls_enabled;
    let params = DaemonParams {
        workspace_data,
        settings,
        session_backend,
        listen_addr,
        tls_enabled,
    };

    DaemonCore::new(params)
        .context("failed to start daemon")?
        .run()
}

/// Parse an optional `--listen <ip>` override from the process args.
///
/// Mirrors the GUI's `src/main.rs`: a missing or malformed value prints a helpful
/// message to stderr and exits non-zero.
fn parse_listen_override() -> Option<IpAddr> {
    let args: Vec<String> = std::env::args().collect();
    let pos = args.iter().position(|a| a == "--listen")?;
    match args.get(pos + 1) {
        Some(addr_str) => match addr_str.parse::<IpAddr>() {
            Ok(addr) => Some(addr),
            Err(_) => {
                eprintln!("Invalid address for --listen: {addr_str}");
                eprintln!("Expected an IP address, e.g. --listen 0.0.0.0");
                std::process::exit(1);
            }
        },
        None => {
            eprintln!("--listen requires an address argument, e.g. --listen 0.0.0.0");
            std::process::exit(1);
        }
    }
}
