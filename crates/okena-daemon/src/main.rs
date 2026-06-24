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

use std::net::IpAddr;

use anyhow::Context;
use okena_daemon_core::{DaemonCore, DaemonParams};
use okena_workspace::persistence;
use okena_workspace::settings::load_settings;

fn main() -> anyhow::Result<()> {
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
    okena_core::profiles::init_profile(profile_paths);
    if let Err(e) =
        okena_core::profiles::migrate_legacy_layout_if_needed(okena_core::profiles::current())
    {
        eprintln!("Warning: profile migration failed: {e}");
    }

    // 1. Logging: env_logger driven by RUST_LOG, defaulting to "info".
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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
    let tls_enabled = settings.remote_tls_enabled;
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
