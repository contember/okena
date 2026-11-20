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
/// machine has no reachable systemd user manager.
pub fn scope_command_prefix(limits: &PaneMemoryLimits, terminal_id: &str) -> Option<Vec<String>> {
    if !limits.enabled {
        return None;
    }
    let systemd_run = systemd_run()?;
    Some(scope_argv(
        systemd_run,
        limits,
        &unit_name(terminal_id),
        terminal_id,
    ))
}

/// `systemd-run --scope` keeps the program's pid, environment, cwd and
/// controlling terminal: it moves itself into the scope and execs the program.
fn scope_argv(
    systemd_run: &str,
    limits: &PaneMemoryLimits,
    unit: &str,
    terminal_id: &str,
) -> Vec<String> {
    vec![
        systemd_run.to_string(),
        "--user".to_string(),
        "--scope".to_string(),
        // Without it, systemd-run prints "Running scope as unit …" into the pane.
        "--quiet".to_string(),
        // A scope that ended in an OOM kill is removed instead of kept as failed.
        "--collect".to_string(),
        format!("--unit={unit}"),
        format!("--description={}", description(terminal_id)),
        "-p".to_string(),
        format!("MemoryHigh={}", limits.memory_high.systemd_value()),
        "-p".to_string(),
        format!("MemoryMax={}", limits.memory_max.systemd_value()),
        // The kernel kills the largest process; the pane's shell keeps running.
        "-p".to_string(),
        "OOMPolicy=continue".to_string(),
        "--".to_string(),
    ]
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

/// Absolute path of a `systemd-run` that can start user scopes here, probed
/// once per process.
#[cfg(target_os = "linux")]
fn systemd_run() -> Option<&'static str> {
    static PROBE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PROBE.get_or_init(probe_systemd_run).as_deref()
}

#[cfg(not(target_os = "linux"))]
fn systemd_run() -> Option<&'static str> {
    None
}

#[cfg(target_os = "linux")]
fn probe_systemd_run() -> Option<String> {
    let path = crate::session_backend::get_extended_path();
    let Some(systemd_run) = std::env::split_paths(&path)
        .map(|dir| dir.join("systemd-run"))
        .find(|candidate| candidate.is_file())
    else {
        log::info!("systemd-run not found; panes start without memory limits");
        return None;
    };
    let systemd_run = systemd_run.to_string_lossy().into_owned();
    let probe = okena_core::process::safe_output_with_timeout(
        crate::process::command(&systemd_run).args([
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            "--",
            "true",
        ]),
        std::time::Duration::from_secs(5),
    );
    match probe {
        Ok(output) if output.status.success() => {
            log::info!("panes run in systemd user scopes with memory limits");
            Some(systemd_run)
        }
        Ok(output) => {
            log::warn!(
                "systemd user scopes are unavailable ({}); panes start without memory limits",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            None
        }
        Err(error) => {
            log::warn!(
                "systemd user scopes are unavailable ({error}); panes start without memory limits"
            );
            None
        }
    }
}

/// Move the processes of a session that started outside any pane scope (a
/// dtach session from before scopes, reattached by a restarted daemon) into a
/// new scope with `limits`. Memory they already hold stays charged where it
/// was; what they allocate from now on counts against the limits.
#[cfg(target_os = "linux")]
pub fn adopt_session(roots: &[i32], limits: &PaneMemoryLimits, terminal_id: &str) {
    if !limits.enabled || systemd_run().is_none() {
        return;
    }
    if roots.iter().any(|&pid| runs_in_pane_scope(pid)) {
        return;
    }
    // A process can exit between the snapshot and the call; one fresh retry.
    for attempt in 0..2 {
        let pids = crate::session_backend::process_and_descendants(roots);
        if pids.is_empty() {
            return;
        }
        let unit = unit_name(terminal_id);
        match start_scope_with_pids(&unit, &pids, limits, terminal_id) {
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

/// Whether `pid` already runs in a scope Okena started for a pane.
#[cfg(target_os = "linux")]
fn runs_in_pane_scope(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .map(|cgroup| is_pane_scope_cgroup(&cgroup))
        .unwrap_or(false)
}

/// Parses `/proc/<pid>/cgroup` (cgroup v2: a single `0::<path>` line).
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
    terminal_id: &str,
) -> Result<(), String> {
    let output = okena_core::process::safe_output_with_timeout(
        crate::process::command("busctl")
            .env("PATH", crate::session_backend::get_extended_path())
            .args(start_transient_unit_args(unit, pids, limits, terminal_id)),
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
    args.extend([unit.to_string(), "fail".to_string(), "6".to_string()]);
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
        "OOMPolicy".to_string(),
        "s".to_string(),
        "continue".to_string(),
        "CollectMode".to_string(),
        "s".to_string(),
        "inactive-or-failed".to_string(),
        // No auxiliary units.
        "0".to_string(),
    ]);
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
        let argv = scope_argv("/usr/bin/systemd-run", &limits, "okena-pane-x.scope", "t-1");
        assert_eq!(
            argv[..5],
            [
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
        assert_eq!(argv.last().map(String::as_str), Some("--"));
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
        let args = start_transient_unit_args("okena-pane-x.scope", &[10, 20], &limits, "t-1");
        let joined = args.join(" ");
        assert!(
            joined
                .contains("okena-pane-x.scope fail 6 Description s Okena pane t-1 PIDs au 2 10 20")
        );
        assert!(joined.contains(&format!("MemoryHigh t {}", 12 * GIB)));
        assert!(joined.contains(&format!("MemoryMax t {}", 16 * GIB)));
        assert!(joined.ends_with("CollectMode s inactive-or-failed 0"));
    }
}
