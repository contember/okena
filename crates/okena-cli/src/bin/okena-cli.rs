//! GPUI-free `okena` CLI for headless hosts (a server, a container) that run
//! `okena-daemon` without the desktop app. Same subcommands as the desktop
//! binary, but it never launches a GUI. Install it as `okena` next to
//! `okena-daemon`: the daemon runs that sibling for Claude Remote's hooks and
//! channel.
//!
//! The profile is resolved like `src/main.rs` does for a subcommand, minus the
//! desktop's start-up bookkeeping (legacy migration, config snapshots), which
//! the daemon does itself.
fn main() {
    if let Some(code) = okena_cli::reject_unparsable_args() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().collect();
    let profile_flag = args
        .iter()
        .position(|a| a == "--profile" || a.starts_with("--profile="))
        .and_then(|i| match args[i].strip_prefix("--profile=") {
            Some(value) => Some(value.to_string()),
            None => args.get(i + 1).cloned(),
        });
    // `claude-remote` runs as a Claude Code hook, often several at once:
    // resolve the profile read-only and never fail Claude (as `src/main.rs`).
    let hook = args
        .get(1)
        .is_some_and(|a| a == okena_cli::CLAUDE_REMOTE_SUBCOMMAND);
    let paths = if hook {
        okena_core::profiles::resolve_existing_profile(profile_flag)
    } else {
        okena_core::profiles::resolve_active_profile(profile_flag)
    };
    let paths = match paths {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(if hook { 0 } else { 1 });
        }
    };
    if !hook {
        // SAFETY: no threads have been spawned yet.
        unsafe { std::env::set_var("OKENA_PROFILE", &paths.id) };
    }
    okena_core::profiles::init_profile(paths);
    match okena_cli::try_handle_cli() {
        Some(code) => std::process::exit(code),
        // `claude-remote` must exit 0 whatever happens.
        None if hook => std::process::exit(0),
        None => {
            eprintln!(
                "okena: this build has no GUI; run `okena-daemon` and use a subcommand (`okena --help`)"
            );
            std::process::exit(2);
        }
    }
}
