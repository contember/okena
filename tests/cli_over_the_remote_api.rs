//! `okena <subcommand>` against a real headless daemon.
//!
//! Every other CLI test is a parser or a resolver over a canned state; nothing
//! reached the HTTP API the subcommands actually speak. This one runs the
//! shipped binary in both roles — `--headless` is the daemon, the same binary
//! is the client — over an isolated config and runtime directory.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_okena");

/// A headless daemon with its own config, runtime and home directory. Runtime
/// isolation is not optional: the dtach socket pool lives under
/// `XDG_RUNTIME_DIR`, and a second instance sharing it reconciles — and kills —
/// the sessions of the one already running.
struct Daemon {
    child: Child,
    root: PathBuf,
}

impl Daemon {
    fn start() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("okena-cli-e2e-{}-{unique}", std::process::id()));
        for sub in ["cfg", "run", "home"] {
            std::fs::create_dir_all(root.join(sub)).expect("scratch directory");
        }

        let child = Self::command(&root)
            .arg("--headless")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the headless daemon");

        let daemon = Self { child, root };
        daemon.wait_for_remote_json();
        daemon
    }

    fn command(root: &Path) -> Command {
        let mut command = Command::new(BIN);
        command
            .env("XDG_CONFIG_HOME", root.join("cfg"))
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .env("HOME", root.join("home"))
            .env_remove("OKENA_PROFILE");
        command
    }

    /// The daemon publishes its port here, and the CLI discovers it from here.
    fn wait_for_remote_json(&self) {
        let published = self.root.join("cfg/okena/profiles/default/remote.json");
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if published.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the daemon never published {}", published.display());
    }

    fn cli(&self, args: &[&str]) -> Output {
        Self::command(&self.root)
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("running `okena {}`: {error}", args.join(" ")))
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.cli(args);
        assert!(
            output.status.success(),
            "`okena {}` failed: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// The bearer token the first CLI call registered for this daemon.
    fn cli_token(&self) -> String {
        let path = self.root.join("cfg/okena/profiles/default/cli.json");
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("cli.json"))
                .expect("cli.json is JSON");
        config["token"].as_str().expect("a token").to_string()
    }

    fn overview(&self) -> serde_json::Value {
        serde_json::from_str(&self.ok(&["ls", "--json"])).expect("`ls --json` emits JSON")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // dtach masters outlive the daemon by design; ours are addressed by a
        // path no other process on the machine can match.
        let _ = Command::new("pkill")
            .arg("-f")
            .arg(self.root.join("run").to_string_lossy().as_ref())
            .status();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn project_ids(overview: &serde_json::Value) -> Vec<String> {
    overview["projects"]
        .as_array()
        .expect("projects array")
        .iter()
        .map(|project| project["id"].as_str().expect("project id").to_string())
        .collect()
}

#[test]
fn the_cli_sees_what_it_adds_to_a_running_daemon() {
    let daemon = Daemon::start();
    let workdir = daemon.root.join("home/a-project");
    std::fs::create_dir_all(&workdir).unwrap();

    let before = project_ids(&daemon.overview());
    let project_id = daemon.ok(&["project", "add", workdir.to_str().unwrap()]);
    assert!(!project_id.is_empty(), "`project add` prints the new id");

    let overview = daemon.overview();
    let after = project_ids(&overview);
    assert!(
        !before.contains(&project_id) && after.contains(&project_id),
        "the added project must appear in `ls --json`: {after:?}"
    );

    let terminal_id = daemon.ok(&["term", "new", &project_id]);
    let terminals = overview_terminals(&daemon.overview(), &project_id);
    assert!(
        terminals.contains(&terminal_id),
        "`term new` printed {terminal_id}, which is not in {terminals:?}"
    );
}

fn overview_terminals(overview: &serde_json::Value, project_id: &str) -> Vec<String> {
    overview["projects"]
        .as_array()
        .expect("projects array")
        .iter()
        .find(|project| project["id"] == project_id)
        .expect("the project we just added")["terminals"]
        .as_array()
        .expect("terminals array")
        .iter()
        .map(|id| id.as_str().expect("terminal id").to_string())
        .collect()
}

#[test]
fn a_null_puts_a_setting_back_to_its_default() {
    let daemon = Daemon::start();
    let default_font_size = daemon.ok(&["settings", "show", "font_size"]);
    assert!(
        !default_font_size.is_empty(),
        "the daemon must answer with the current value"
    );

    daemon.ok(&["settings", "set", "font_size", "17"]);
    assert_eq!(daemon.ok(&["settings", "show", "font_size"]), "17.0");
    daemon.ok(&["settings", "set", "font_size", "null"]);
    assert_eq!(
        daemon.ok(&["settings", "show", "font_size"]),
        default_font_size,
        "null must restore the default, not store a literal null"
    );

    daemon.ok(&[
        "settings",
        "set",
        "hooks.worktree.pre_merge",
        "echo okena-e2e",
    ]);
    assert!(
        daemon
            .ok(&["settings", "show", "hooks"])
            .contains("okena-e2e"),
        "the hook must survive the round trip to the daemon"
    );
    daemon.ok(&["settings", "set", "hooks.worktree.pre_merge", "null"]);
    assert!(
        !daemon
            .ok(&["settings", "show", "hooks"])
            .contains("okena-e2e"),
        "clearing one hook must reach the daemon"
    );
}

/// The TUI binary, built alongside this test by `cargo test --workspace`.
/// `CARGO_BIN_EXE_` only covers this package's own binaries, so it is found
/// next to the test executable instead.
fn tui_binary() -> Option<PathBuf> {
    let mut dir = std::env::current_exe().ok()?;
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    let binary = dir.join("okena-tui");
    binary.exists().then_some(binary)
}

/// `TerminalGuard` owns every host terminal mode the TUI changes, and it is
/// entered only once a connection carries state. A TUI that cannot reach its
/// daemon therefore owes the shell it was started from an untouched terminal —
/// which only a real terminal can answer for.
#[test]
fn a_tui_that_cannot_connect_leaves_the_host_terminal_alone() {
    let Some(tui) = tui_binary() else {
        assert!(
            std::env::var_os("OKENA_REQUIRE_TUI").is_none(),
            "okena-tui is not built and OKENA_REQUIRE_TUI demands it"
        );
        eprintln!("skipping: okena-tui is not built (run `cargo test --workspace`)");
        return;
    };

    let pty = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open a pty");

    // Port 1 is privileged and unbound: the connection fails before the guard.
    let mut command = portable_pty::CommandBuilder::new(tui);
    command.args(["--host", "127.0.0.1", "--port", "1"]);
    command.env("TERM", "xterm-256color");
    let mut child = pty.slave.spawn_command(command).expect("spawn the tui");
    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().expect("pty reader");
    let pump = std::thread::spawn(move || {
        let mut host = Vec::new();
        let _ = std::io::Read::read_to_end(&mut reader, &mut host);
        host
    });

    let status = child.wait().expect("the tui exits");
    drop(pty.master);
    let host = String::from_utf8_lossy(&pump.join().expect("reader thread")).into_owned();

    assert!(!status.success(), "an unreachable daemon must not exit 0");
    for (sequence, what) in [
        ("\x1b[?1049h", "the alternate screen"),
        ("\x1b[?25l", "cursor hiding"),
        ("\x1b[?2004h", "bracketed paste"),
        ("\x1b[?7l", "no-wrap"),
    ] {
        assert!(
            !host.contains(sequence),
            "{what} was switched on before the connection existed: {}",
            host.escape_debug()
        );
    }
}

/// The other half of the guard's contract: what it turned on, it turns back off.
/// `Drop` runs only when the event loop returns, so this also pins that the quit
/// binding stays reachable — it was unreachable until the byte Ctrl+] actually
/// sends was matched.
#[test]
fn the_tui_hands_the_host_terminal_back_when_it_quits() {
    let Some(tui) = tui_binary() else {
        assert!(
            std::env::var_os("OKENA_REQUIRE_TUI").is_none(),
            "okena-tui is not built and OKENA_REQUIRE_TUI demands it"
        );
        eprintln!("skipping: okena-tui is not built (run `cargo test --workspace`)");
        return;
    };

    let daemon = Daemon::start();
    // Registers the token this run reuses; the TUI does no pairing of its own.
    daemon.ok(&["ls"]);

    let pty = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open a pty");

    let mut command = portable_pty::CommandBuilder::new(tui);
    command.env("XDG_CONFIG_HOME", daemon.root.join("cfg"));
    command.env("XDG_RUNTIME_DIR", daemon.root.join("run"));
    command.env("HOME", daemon.root.join("home"));
    command.env("OKENA_TOKEN", daemon.cli_token());
    command.env("TERM", "xterm-256color");
    let mut child = pty.slave.spawn_command(command).expect("spawn the tui");
    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().expect("pty reader");
    let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = collected.clone();
    let pump = std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = std::io::Read::read(&mut reader, &mut buffer) {
            if read == 0 {
                break;
            }
            sink.lock()
                .expect("sink")
                .extend_from_slice(&buffer[..read]);
        }
    });
    let host = || String::from_utf8_lossy(&collected.lock().expect("sink")).into_owned();

    let deadline = Instant::now() + Duration::from_secs(60);
    while !host().contains("\x1b[?1049h") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        host().contains("\x1b[?1049h"),
        "the tui never reached the alternate screen: {}",
        host().escape_debug()
    );

    let mut writer = pty.master.take_writer().expect("pty writer");
    // The byte Ctrl+] sends. Anything the TUI does not recognise as quit goes
    // to the remote terminal instead, and it would run until killed.
    std::io::Write::write_all(&mut writer, b"\x1d").expect("send ctrl-]");
    std::io::Write::flush(&mut writer).expect("flush ctrl-]");
    drop(writer);

    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait().expect("poll the tui") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("the tui did not quit on ctrl-]: {}", host().escape_debug());
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    drop(pty.master);
    let _ = pump.join();

    assert!(status.success(), "quitting must exit 0, got {status:?}");
    let host = host();
    for (sequence, what) in [
        ("\x1b[?1049l", "the alternate screen"),
        ("\x1b[?7h", "autowrap"),
        ("\x1b[?25h", "the cursor"),
        ("\x1b[?2004l", "bracketed paste"),
    ] {
        assert!(
            host.contains(sequence),
            "{what} was never restored: {}",
            host.escape_debug()
        );
    }
}
