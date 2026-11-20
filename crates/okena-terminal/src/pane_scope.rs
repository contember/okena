//! Memory limits for the processes of a pane.
//!
//! On Linux with a systemd user manager, every pane's program runs in its own
//! transient scope (`okena-pane-<id>-<nonce>.scope`) with `MemoryHigh` and
//! `MemoryMax`, so one runaway pane cannot exhaust the machine. Elsewhere, or
//! when the user manager is unreachable, panes start unconfined as before.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Prefix of every scope unit Okena starts for a pane.
pub const PANE_SCOPE_PREFIX: &str = "okena-pane-";

/// Per-profile memory limits of a pane's scope (`settings.json` →
/// `pane_memory`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneMemoryLimits {
    /// Run each pane in its own systemd user scope. Linux only.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// systemd `MemoryHigh`: past it the kernel throttles and reclaims the pane.
    #[serde(default = "default_memory_high")]
    pub memory_high: MemoryLimit,
    /// systemd `MemoryMax`: past it the kernel OOM-kills inside the pane.
    #[serde(default = "default_memory_max")]
    pub memory_max: MemoryLimit,
}

fn default_enabled() -> bool {
    true
}

fn default_memory_high() -> MemoryLimit {
    MemoryLimit::Bytes(12 * GIB)
}

fn default_memory_max() -> MemoryLimit {
    MemoryLimit::Bytes(16 * GIB)
}

impl Default for PaneMemoryLimits {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            memory_high: default_memory_high(),
            memory_max: default_memory_max(),
        }
    }
}

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;
const TIB: u64 = 1024 * GIB;

/// A cgroup memory limit: a positive byte count or `infinity`. Written in
/// settings as systemd does, `"12G"`, `"512M"`, `"1073741824"` or
/// `"infinity"` (suffixes are powers of 1024).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum MemoryLimit {
    Bytes(u64),
    Infinity,
}

impl MemoryLimit {
    /// The value of a systemd `-p Memory…=` property.
    fn systemd_value(self) -> String {
        match self {
            Self::Bytes(bytes) => bytes.to_string(),
            Self::Infinity => "infinity".to_string(),
        }
    }

    /// The D-Bus `t` value systemd takes for the property; `u64::MAX` is
    /// infinity.
    #[cfg(target_os = "linux")]
    fn dbus_value(self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes,
            Self::Infinity => u64::MAX,
        }
    }
}

impl FromStr for MemoryLimit {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("infinity") {
            return Ok(Self::Infinity);
        }
        let (digits, unit) = match value.char_indices().last() {
            Some((index, suffix)) if suffix.is_ascii_alphabetic() => {
                let unit = match suffix.to_ascii_uppercase() {
                    'K' => KIB,
                    'M' => MIB,
                    'G' => GIB,
                    'T' => TIB,
                    _ => return Err(format!("unknown memory unit in {value:?}")),
                };
                (&value[..index], unit)
            }
            _ => (value, 1),
        };
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!("invalid memory limit {value:?}"));
        }
        let bytes = digits
            .parse::<u64>()
            .ok()
            .and_then(|count| count.checked_mul(unit))
            .ok_or_else(|| format!("memory limit {value:?} is too large"))?;
        if bytes == 0 {
            return Err("a memory limit of 0 would kill every process in the pane".to_string());
        }
        Ok(Self::Bytes(bytes))
    }
}

impl TryFrom<String> for MemoryLimit {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<MemoryLimit> for String {
    fn from(limit: MemoryLimit) -> Self {
        limit.to_string()
    }
}

impl fmt::Display for MemoryLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = match *self {
            Self::Infinity => return f.write_str("infinity"),
            Self::Bytes(bytes) => bytes,
        };
        for (unit, suffix) in [(TIB, "T"), (GIB, "G"), (MIB, "M"), (KIB, "K")] {
            if bytes % unit == 0 {
                return write!(f, "{}{suffix}", bytes / unit);
            }
        }
        write!(f, "{bytes}")
    }
}

/// The argv that runs a pane's program in a fresh scope with `limits`: the
/// program's own argv goes after it. `None` when scopes are disabled or this
/// machine cannot run them (see [`ScopeRunner`]).
pub fn scope_command_prefix(limits: &PaneMemoryLimits, terminal_id: &str) -> Option<Vec<String>> {
    if !limits.enabled {
        return None;
    }
    let runner = scope_runner()?;
    Some(scope_argv(
        runner,
        limits,
        &unit_name(terminal_id),
        terminal_id,
    ))
}

/// Run the one-time probe off the caller's thread, so the first pane does not
/// wait on the user manager.
pub fn warm_up(limits: &PaneMemoryLimits) {
    if !limits.enabled {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("pane-scope-probe".to_string())
        .spawn(|| {
            scope_runner();
        });
    if let Err(error) = spawned {
        log::warn!("could not start the pane scope probe: {error}");
    }
}

/// How this machine starts pane scopes, found once per process.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScopeRunner {
    /// `env NAME=value…` naming the user manager. A tmux or screen server
    /// started without them (`su -`, cron) would leave `systemd-run` unable to
    /// connect, and a pane that runs nothing.
    env_prefix: Vec<String>,
    systemd_run: String,
    /// Scopes take `OOMPolicy=` from systemd 253 on; older ones reject it.
    oom_policy: bool,
    /// `--expand-environment=no`, known from systemd 254 on. Later releases
    /// expand `$VAR` in a scope's argv by default, which rewrites `$$` and
    /// `${…}` in a pane's command.
    no_expand_environment: bool,
}

impl ScopeRunner {
    fn for_systemd_version(env_prefix: Vec<String>, systemd_run: String, version: u32) -> Self {
        Self {
            env_prefix,
            systemd_run,
            oom_policy: version >= 253,
            no_expand_environment: version >= 254,
        }
    }
}

/// `systemd-run --scope` keeps the program's pid, environment, cwd and
/// controlling terminal: it moves itself into the scope and execs the program.
fn scope_argv(
    runner: &ScopeRunner,
    limits: &PaneMemoryLimits,
    unit: &str,
    terminal_id: &str,
) -> Vec<String> {
    let mut argv = runner.env_prefix.clone();
    argv.extend([
        runner.systemd_run.clone(),
        "--user".to_string(),
        "--scope".to_string(),
        // Without it, systemd-run prints "Running scope as unit …" into the pane.
        "--quiet".to_string(),
        // A scope that ended in an OOM kill is removed instead of kept as failed.
        "--collect".to_string(),
        format!("--unit={unit}"),
        format!("--description={}", description(terminal_id)),
    ]);
    if runner.no_expand_environment {
        argv.push("--expand-environment=no".to_string());
    }
    for property in scope_properties(limits, runner.oom_policy) {
        argv.extend(["-p".to_string(), property]);
    }
    argv.push("--".to_string());
    argv
}

fn scope_properties(limits: &PaneMemoryLimits, oom_policy: bool) -> Vec<String> {
    let mut properties = vec![
        format!("MemoryHigh={}", limits.memory_high.systemd_value()),
        format!("MemoryMax={}", limits.memory_max.systemd_value()),
    ];
    // The kernel kills the largest process; the pane's shell keeps running.
    if oom_policy {
        properties.push("OOMPolicy=continue".to_string());
    }
    properties
}

fn description(terminal_id: &str) -> String {
    format!("Okena pane {terminal_id}")
}

/// A unit name unique per spawn: a session that outlives its pane, or a
/// process a pane left behind, keeps its scope alive under the old name.
fn unit_name(terminal_id: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id: String = terminal_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{PANE_SCOPE_PREFIX}{id}-{nanos:x}{sequence:x}.scope")
}

#[cfg(target_os = "linux")]
fn scope_runner() -> Option<&'static ScopeRunner> {
    static PROBE: std::sync::OnceLock<Option<ScopeRunner>> = std::sync::OnceLock::new();
    PROBE.get_or_init(probe_scope_runner).as_ref()
}

#[cfg(not(target_os = "linux"))]
fn scope_runner() -> Option<&'static ScopeRunner> {
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        log::info!("pane memory limits need systemd (Linux); panes start without them");
    });
    None
}

#[cfg(target_os = "linux")]
fn probe_scope_runner() -> Option<ScopeRunner> {
    let unavailable = |reason: &str| {
        log::warn!(
            "systemd user scopes are unavailable ({reason}); panes start without memory limits"
        );
        None
    };
    let path = crate::session_backend::get_extended_path();
    let find = |name: &str| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
            .map(|found| found.to_string_lossy().into_owned())
    };
    let Some(systemd_run) = find("systemd-run") else {
        return unavailable("systemd-run not found");
    };
    if !scopes_survive_logout() {
        return unavailable(
            "the user manager stops at logout and would take every pane with it; \
             run Okena from the graphical session or enable lingering (loginctl enable-linger)",
        );
    }
    let env_prefix = match find("env") {
        Some(env) => {
            let vars: Vec<String> = ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"]
                .into_iter()
                .filter_map(|name| Some(format!("{name}={}", std::env::var(name).ok()?)))
                .collect();
            if vars.is_empty() {
                Vec::new()
            } else {
                std::iter::once(env).chain(vars).collect()
            }
        }
        None => Vec::new(),
    };
    let Some(version) = systemd_version(&systemd_run) else {
        return unavailable("could not read the systemd version");
    };
    let runner = ScopeRunner::for_systemd_version(env_prefix, systemd_run, version);
    // The launcher's own argv, so a property the manager rejects shows here
    // and not as a pane that never starts.
    let limits = PaneMemoryLimits {
        enabled: true,
        memory_high: MemoryLimit::Infinity,
        memory_max: MemoryLimit::Infinity,
    };
    let mut argv = scope_argv(&runner, &limits, &unit_name("probe"), "probe");
    argv.push("true".to_string());
    let output = okena_core::process::safe_output_with_timeout(
        crate::process::command(&argv[0]).args(&argv[1..]),
        std::time::Duration::from_secs(5),
    );
    match output {
        Ok(output) if output.status.success() => {
            log::info!("panes run in systemd {version} user scopes with memory limits");
            Some(runner)
        }
        Ok(output) => unavailable(String::from_utf8_lossy(&output.stderr).trim()),
        Err(error) => unavailable(&error.to_string()),
    }
}

#[cfg(target_os = "linux")]
fn systemd_version(systemd_run: &str) -> Option<u32> {
    let output = okena_core::process::safe_output_with_timeout(
        crate::process::command(systemd_run).arg("--version"),
        std::time::Duration::from_secs(5),
    )
    .ok()?;
    parse_systemd_version(&String::from_utf8_lossy(&output.stdout))
}

/// Parses `systemd-run --version`, whose first line is `systemd 257 (…)`.
#[cfg(any(target_os = "linux", test))]
fn parse_systemd_version(output: &str) -> Option<u32> {
    let mut words = output.lines().next()?.split_whitespace();
    if words.next()? != "systemd" {
        return None;
    }
    words.next()?.parse().ok()
}

/// Scopes are units of the user manager, which logind stops when the user's
/// last session ends unless lingering is on. A daemon that runs under that
/// manager stops with it anyway; any other daemon would outlive its panes.
#[cfg(target_os = "linux")]
fn scopes_survive_logout() -> bool {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let own_cgroup = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    if runs_under_user_manager(&own_cgroup, uid) {
        return true;
    }
    okena_core::process::safe_output_with_timeout(
        crate::process::command("loginctl")
            .env("PATH", crate::session_backend::get_extended_path())
            .args([
                "show-user",
                &uid.to_string(),
                "--property=Linger",
                "--value",
            ]),
        std::time::Duration::from_secs(5),
    )
    .is_ok_and(|output| output.status.success() && output.stdout.trim_ascii() == b"yes")
}

/// Parses `/proc/self/cgroup`.
#[cfg(any(target_os = "linux", test))]
fn runs_under_user_manager(cgroup: &str, uid: u32) -> bool {
    let manager = format!("/user@{uid}.service/");
    cgroup
        .lines()
        .filter_map(|line| line.strip_prefix("0::"))
        .any(|path| path.contains(&manager))
}

/// Move the processes of a session that started outside any pane scope (a
/// dtach session from before scopes, reattached by a restarted daemon) into a
/// new scope with `limits`. Memory they already hold stays charged where it
/// was; what they allocate from now on counts against the limits.
#[cfg(target_os = "linux")]
pub fn adopt_session(roots: &[i32], limits: &PaneMemoryLimits, terminal_id: &str) {
    if !limits.enabled {
        return;
    }
    let Some(runner) = scope_runner() else {
        return;
    };
    let cgroup_of = |pid: i32| std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok();
    let root_cgroups: Vec<String> = roots.iter().filter_map(|&pid| cgroup_of(pid)).collect();
    if root_cgroups
        .iter()
        .any(|cgroup| is_pane_scope_cgroup(cgroup))
    {
        return;
    }
    // A process can exit between the snapshot and the call; one fresh retry.
    for attempt in 0..2 {
        // A descendant in a scope of its own (a nested `systemd-run`, a snap)
        // keeps it and its limits.
        let pids: Vec<i32> = crate::session_backend::process_and_descendants(roots)
            .into_iter()
            .filter(|&pid| cgroup_of(pid).is_some_and(|cgroup| root_cgroups.contains(&cgroup)))
            .collect();
        if pids.is_empty() {
            return;
        }
        let unit = unit_name(terminal_id);
        match start_scope_with_pids(&unit, &pids, limits, runner.oom_policy, terminal_id) {
            Ok(()) => {
                log::info!(
                    "moved {} process(es) of terminal {terminal_id} into {unit}",
                    pids.len()
                );
                return;
            }
            Err(error) if attempt == 0 => {
                log::debug!(
                    "adopting terminal {terminal_id} into a scope failed, retrying: {error}"
                );
            }
            Err(error) => {
                log::warn!("could not move terminal {terminal_id} into a scope: {error}");
            }
        }
    }
}

/// Whether a `/proc/<pid>/cgroup` (cgroup v2: a single `0::<path>` line)
/// names a scope Okena started for a pane.
#[cfg(any(target_os = "linux", test))]
fn is_pane_scope_cgroup(cgroup: &str) -> bool {
    cgroup
        .lines()
        .filter_map(|line| line.strip_prefix("0::"))
        .filter_map(|path| path.rsplit('/').next())
        .any(|unit| unit.starts_with(PANE_SCOPE_PREFIX) && unit.ends_with(".scope"))
}

/// `systemd-run` cannot take existing processes, so this calls the manager's
/// `StartTransientUnit` with a `PIDs` property through `busctl`.
#[cfg(target_os = "linux")]
fn start_scope_with_pids(
    unit: &str,
    pids: &[i32],
    limits: &PaneMemoryLimits,
    oom_policy: bool,
    terminal_id: &str,
) -> Result<(), String> {
    let output = okena_core::process::safe_output_with_timeout(
        crate::process::command("busctl")
            .env("PATH", crate::session_backend::get_extended_path())
            .args(start_transient_unit_args(
                unit,
                pids,
                limits,
                oom_policy,
                terminal_id,
            )),
        std::time::Duration::from_secs(5),
    )
    .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(target_os = "linux")]
fn start_transient_unit_args(
    unit: &str,
    pids: &[i32],
    limits: &PaneMemoryLimits,
    oom_policy: bool,
    terminal_id: &str,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "--user",
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "StartTransientUnit",
        "ssa(sv)a(sa(sv))",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let property_count = if oom_policy { 6 } else { 5 };
    args.extend([
        unit.to_string(),
        "fail".to_string(),
        property_count.to_string(),
    ]);
    args.extend([
        "Description".to_string(),
        "s".to_string(),
        description(terminal_id),
    ]);
    args.extend(["PIDs".to_string(), "au".to_string(), pids.len().to_string()]);
    args.extend(pids.iter().map(|pid| pid.to_string()));
    args.extend([
        "MemoryHigh".to_string(),
        "t".to_string(),
        limits.memory_high.dbus_value().to_string(),
        "MemoryMax".to_string(),
        "t".to_string(),
        limits.memory_max.dbus_value().to_string(),
        "CollectMode".to_string(),
        "s".to_string(),
        "inactive-or-failed".to_string(),
    ]);
    if oom_policy {
        args.extend([
            "OOMPolicy".to_string(),
            "s".to_string(),
            "continue".to_string(),
        ]);
    }
    // No auxiliary units.
    args.push("0".to_string());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_limits_parse_systemd_sizes() {
        assert_eq!("12G".parse(), Ok(MemoryLimit::Bytes(12 * GIB)));
        assert_eq!("512m".parse(), Ok(MemoryLimit::Bytes(512 * MIB)));
        assert_eq!("1048576".parse(), Ok(MemoryLimit::Bytes(MIB)));
        assert_eq!(" infinity ".parse(), Ok(MemoryLimit::Infinity));
        assert_eq!("2T".parse(), Ok(MemoryLimit::Bytes(2 * TIB)));
    }

    #[test]
    fn memory_limits_reject_what_systemd_would_misread() {
        for value in [
            "",
            "G",
            "0",
            "0G",
            "12GB",
            "1.5G",
            "-1G",
            "12X",
            "99999999999T",
        ] {
            assert!(
                value.parse::<MemoryLimit>().is_err(),
                "{value:?} must be rejected"
            );
        }
    }

    #[test]
    fn memory_limits_print_in_the_largest_exact_unit() {
        assert_eq!(MemoryLimit::Bytes(16 * GIB).to_string(), "16G");
        assert_eq!(MemoryLimit::Bytes(1536 * MIB).to_string(), "1536M");
        assert_eq!(MemoryLimit::Bytes(1000).to_string(), "1000");
        assert_eq!(MemoryLimit::Infinity.to_string(), "infinity");
    }

    #[test]
    fn settings_default_and_round_trip() {
        let limits: PaneMemoryLimits = serde_json::from_str("{}").expect("defaults");
        assert_eq!(limits, PaneMemoryLimits::default());
        assert_eq!(limits.memory_high, MemoryLimit::Bytes(12 * GIB));
        assert_eq!(limits.memory_max, MemoryLimit::Bytes(16 * GIB));
        let json = serde_json::to_string(&limits).expect("serialize");
        assert_eq!(
            json,
            r#"{"enabled":true,"memory_high":"12G","memory_max":"16G"}"#
        );
        assert!(serde_json::from_str::<PaneMemoryLimits>(r#"{"memory_max":"lots"}"#).is_err());
    }

    #[test]
    fn scope_argv_sets_limits_and_ends_before_the_program() {
        let limits = PaneMemoryLimits {
            enabled: true,
            memory_high: MemoryLimit::Bytes(GIB),
            memory_max: MemoryLimit::Infinity,
        };
        let runner = ScopeRunner::for_systemd_version(
            vec![
                "/usr/bin/env".to_string(),
                "XDG_RUNTIME_DIR=/run/user/7".to_string(),
            ],
            "/usr/bin/systemd-run".to_string(),
            257,
        );
        let argv = scope_argv(&runner, &limits, "okena-pane-x.scope", "t-1");
        assert_eq!(
            argv[..7],
            [
                "/usr/bin/env",
                "XDG_RUNTIME_DIR=/run/user/7",
                "/usr/bin/systemd-run",
                "--user",
                "--scope",
                "--quiet",
                "--collect"
            ]
        );
        assert!(argv.contains(&"--unit=okena-pane-x.scope".to_string()));
        assert!(argv.contains(&"--description=Okena pane t-1".to_string()));
        assert!(argv.contains(&format!("MemoryHigh={GIB}")));
        assert!(argv.contains(&"MemoryMax=infinity".to_string()));
        assert!(argv.contains(&"OOMPolicy=continue".to_string()));
        assert!(argv.contains(&"--expand-environment=no".to_string()));
        assert_eq!(argv.last().map(String::as_str), Some("--"));
    }

    #[test]
    fn scope_options_follow_the_systemd_version() {
        let argv = |version| {
            let runner =
                ScopeRunner::for_systemd_version(Vec::new(), "systemd-run".to_string(), version);
            scope_argv(&runner, &PaneMemoryLimits::default(), "u.scope", "t")
        };
        let has = |argv: &[String], arg: &str| argv.iter().any(|a| a == arg);
        let v252 = argv(252);
        assert_eq!(v252[0], "systemd-run");
        assert!(!has(&v252, "OOMPolicy=continue"));
        assert!(!has(&v252, "--expand-environment=no"));
        assert!(has(&v252, &format!("MemoryMax={}", 16 * GIB)));
        let v253 = argv(253);
        assert!(has(&v253, "OOMPolicy=continue"));
        assert!(!has(&v253, "--expand-environment=no"));
        assert!(has(&argv(254), "--expand-environment=no"));
    }

    #[test]
    fn systemd_versions_parse_from_the_first_line() {
        assert_eq!(
            parse_systemd_version("systemd 257 (257.9-0ubuntu2.5)\n+PAM +AUDIT\n"),
            Some(257)
        );
        assert_eq!(parse_systemd_version("systemd 249\n"), Some(249));
        assert_eq!(parse_systemd_version("something 257\n"), None);
        assert_eq!(parse_systemd_version(""), None);
    }

    #[test]
    fn user_manager_cgroups_are_recognized() {
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/x.scope\n";
        assert!(runs_under_user_manager(cgroup, 1000));
        assert!(!runs_under_user_manager(cgroup, 1001));
        assert!(!runs_under_user_manager(
            "0::/user.slice/user-1000.slice/session-3.scope\n",
            1000
        ));
    }

    #[test]
    fn disabled_limits_give_no_scope() {
        let limits = PaneMemoryLimits {
            enabled: false,
            ..PaneMemoryLimits::default()
        };
        assert_eq!(scope_command_prefix(&limits, "t"), None);
    }

    #[test]
    fn unit_names_are_valid_and_unique_per_spawn() {
        let first = unit_name("a1b2/c3 d4");
        let second = unit_name("a1b2/c3 d4");
        assert_ne!(first, second);
        assert!(first.starts_with("okena-pane-a1b2_c3_d4-"));
        assert!(first.ends_with(".scope"));
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        );
    }

    #[test]
    fn pane_scope_cgroups_are_recognized() {
        assert!(is_pane_scope_cgroup(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/okena-pane-abc-1f.scope\n"
        ));
        assert!(!is_pane_scope_cgroup(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/vte-spawn-1.scope\n"
        ));
        assert!(!is_pane_scope_cgroup("0::/okena-pane-abc.slice/x\n"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn start_transient_unit_passes_pids_and_limits() {
        let limits = PaneMemoryLimits::default();
        let args = start_transient_unit_args("okena-pane-x.scope", &[10, 20], &limits, true, "t-1");
        let joined = args.join(" ");
        assert!(
            joined
                .contains("okena-pane-x.scope fail 6 Description s Okena pane t-1 PIDs au 2 10 20")
        );
        assert!(joined.contains(&format!("MemoryHigh t {}", 12 * GIB)));
        assert!(joined.contains(&format!("MemoryMax t {}", 16 * GIB)));
        assert!(joined.ends_with("CollectMode s inactive-or-failed OOMPolicy s continue 0"));

        let old = start_transient_unit_args("u.scope", &[10], &limits, false, "t-1").join(" ");
        assert!(old.contains("u.scope fail 5 Description"));
        assert!(old.ends_with("CollectMode s inactive-or-failed 0"));
    }
}
