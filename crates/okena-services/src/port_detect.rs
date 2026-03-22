use okena_core::process::{command, safe_output};
use std::collections::{HashMap, HashSet, VecDeque};

/// Ports to exclude from detection results.
/// 9229 = Node.js inspector/debugger
const IGNORED_PORTS: &[u16] = &[9229];

/// Ports at or above this threshold are considered ephemeral/internal and excluded.
/// Linux default ephemeral range starts at 32768.
const EPHEMERAL_PORT_MIN: u16 = 32768;

// ---------------------------------------------------------------------------
// Process tree
// ---------------------------------------------------------------------------

/// Build the system-wide parent→children process tree.
/// On Linux reads `/proc`, on macOS runs `ps -eo pid,ppid`,
/// on Windows runs a single `wmic` call.
pub fn build_process_tree() -> HashMap<u32, Vec<u32>> {
    #[cfg(target_os = "linux")]
    {
        build_process_tree_linux()
    }

    #[cfg(target_os = "macos")]
    {
        build_process_tree_macos()
    }

    #[cfg(windows)]
    {
        build_process_tree_windows()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        HashMap::new()
    }
}

/// BFS to find all descendants of `root_pid` in a pre-built process tree.
pub fn descendants_from_tree(tree: &HashMap<u32, Vec<u32>>, root_pid: u32) -> HashSet<u32> {
    let mut result = HashSet::new();
    result.insert(root_pid);
    let mut queue = VecDeque::new();
    queue.push_back(root_pid);
    while let Some(pid) = queue.pop_front() {
        if let Some(children) = tree.get(&pid) {
            for &child in children {
                if result.insert(child) {
                    queue.push_back(child);
                }
            }
        }
    }
    result
}

/// Walk the process tree to find all descendant PIDs (children, grandchildren, etc.)
/// including the root PID itself. Convenience wrapper that builds the tree internally.
pub fn get_descendant_pids(root_pid: u32) -> HashSet<u32> {
    let tree = build_process_tree();
    descendants_from_tree(&tree, root_pid)
}

#[cfg(target_os = "linux")]
fn build_process_tree_linux() -> HashMap<u32, Vec<u32>> {
    use std::fs;

    let mut tree: HashMap<u32, Vec<u32>> = HashMap::new();

    let entries = match fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return tree,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let pid: u32 = match name_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        let stat_path = format!("/proc/{}/stat", pid);
        let stat = match fs::read_to_string(&stat_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Format: "pid (comm) state ppid ..."
        // The comm field can contain spaces and parentheses, so find the last ')'.
        if let Some(after_comm) = stat.rfind(')') {
            let fields: Vec<&str> = stat[after_comm + 2..].split_whitespace().collect();
            // fields[0] = state, fields[1] = ppid
            if let Some(ppid_str) = fields.get(1) {
                if let Ok(ppid) = ppid_str.parse::<u32>() {
                    tree.entry(ppid).or_default().push(pid);
                }
            }
        }
    }

    tree
}

#[cfg(target_os = "macos")]
fn build_process_tree_macos() -> HashMap<u32, Vec<u32>> {
    // Single `ps` call instead of recursive `pgrep -P` per PID.
    let mut cmd = command("ps");
    cmd.args(["-eo", "pid,ppid"]);
    let output = match safe_output(&mut cmd) {
        Ok(o) if o.status.success() => o,
        _ => return HashMap::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut tree: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in stdout.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 2 {
            if let (Ok(pid), Ok(ppid)) = (fields[0].parse::<u32>(), fields[1].parse::<u32>()) {
                tree.entry(ppid).or_default().push(pid);
            }
        }
    }
    tree
}

#[cfg(windows)]
fn build_process_tree_windows() -> HashMap<u32, Vec<u32>> {
    // Single `wmic` call instead of recursive per-PID queries.
    // CSV output: Node,ParentProcessId,ProcessId (alphabetical column order).
    let mut cmd = command("wmic");
    cmd.args(["process", "get", "ProcessId,ParentProcessId", "/FORMAT:CSV"]);
    let output = match safe_output(&mut cmd) {
        Ok(o) if o.status.success() => o,
        _ => return HashMap::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut tree: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let fields: Vec<&str> = trimmed.split(',').collect();
        // CSV: Node,ParentProcessId,ProcessId
        if fields.len() >= 3 {
            if let (Ok(ppid), Ok(pid)) = (
                fields[1].trim().parse::<u32>(),
                fields[2].trim().parse::<u32>(),
            ) {
                tree.entry(ppid).or_default().push(pid);
            }
        }
    }
    tree
}

// ---------------------------------------------------------------------------
// Port detection
// ---------------------------------------------------------------------------

/// Get all listening TCP (pid, port) pairs from the system in a single command.
pub fn get_listening_port_pairs() -> Vec<(u32, u16)> {
    #[cfg(target_os = "linux")]
    {
        get_listening_port_pairs_linux()
    }

    #[cfg(target_os = "macos")]
    {
        get_listening_port_pairs_macos()
    }

    #[cfg(windows)]
    {
        get_listening_port_pairs_windows()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Vec::new()
    }
}

/// Filter (pid, port) pairs: keep only ports owned by `pids`,
/// remove ephemeral/ignored ports, sort and deduplicate.
pub fn ports_for_pids(pairs: &[(u32, u16)], pids: &HashSet<u32>) -> Vec<u16> {
    let mut ports: Vec<u16> = pairs
        .iter()
        .filter(|(pid, port)| {
            pids.contains(pid) && *port < EPHEMERAL_PORT_MIN && !IGNORED_PORTS.contains(port)
        })
        .map(|(_, port)| *port)
        .collect();
    ports.sort();
    ports.dedup();
    ports
}

// ---------------------------------------------------------------------------
// Platform-specific port pair getters
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn get_listening_port_pairs_linux() -> Vec<(u32, u16)> {
    let mut cmd = command("ss");
    cmd.args(["-tlnp"]);
    let output = match safe_output(&mut cmd) {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ss_output(&stdout)
}

#[cfg(target_os = "macos")]
fn get_listening_port_pairs_macos() -> Vec<(u32, u16)> {
    let mut cmd = command("lsof");
    cmd.args(["-iTCP", "-sTCP:LISTEN", "-P", "-n"]);
    let output = match safe_output(&mut cmd) {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_lsof_output(&stdout)
}

#[cfg(windows)]
fn get_listening_port_pairs_windows() -> Vec<(u32, u16)> {
    let mut cmd = command("netstat");
    cmd.args(["-ano"]);
    let output = match safe_output(&mut cmd) {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_netstat_output(&stdout)
}

// ---------------------------------------------------------------------------
// Parsers — return all (pid, port) pairs found in the output
// ---------------------------------------------------------------------------

/// Parse `ss -tlnp` output into (pid, port) pairs.
pub(crate) fn parse_ss_output(stdout: &str) -> Vec<(u32, u16)> {
    let mut pairs = Vec::new();
    for line in stdout.lines() {
        let pids = extract_pids_from_ss_line(line);
        if let Some(port) = extract_port_from_ss_line(line) {
            for pid in pids {
                pairs.push((pid, port));
            }
        }
    }
    pairs
}

fn extract_pids_from_ss_line(line: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    let mut search = line;
    while let Some(pos) = search.find("pid=") {
        let after = &search[pos + 4..];
        let num_end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
        if let Ok(pid) = after[..num_end].parse::<u32>() {
            pids.push(pid);
        }
        search = &after[num_end..];
    }
    pids
}

fn extract_port_from_ss_line(line: &str) -> Option<u16> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 5 {
        return None;
    }
    let local_addr = fields[3];
    let port_str = local_addr.rsplit(':').next()?;
    port_str.parse().ok()
}

/// Parse `lsof -iTCP -sTCP:LISTEN -P -n` output into (pid, port) pairs.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_lsof_output(stdout: &str) -> Vec<(u32, u16)> {
    let mut pairs = Vec::new();
    for line in stdout.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        let pid: u32 = match fields[1].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        // NAME field like "*:5173" or "127.0.0.1:3000"
        let name = fields[8];
        if let Some(port_str) = name.rsplit(':').next() {
            if let Ok(port) = port_str.parse::<u16>() {
                pairs.push((pid, port));
            }
        }
    }
    pairs
}

/// Parse `netstat -ano` output into (pid, port) pairs.
#[cfg(any(windows, test))]
pub(crate) fn parse_netstat_output(stdout: &str) -> Vec<(u32, u16)> {
    let mut pairs = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.contains("LISTENING") {
            continue;
        }
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        let pid: u32 = match fields[4].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let local_addr = fields[1];
        if let Some(port_str) = local_addr.rsplit(':').next() {
            if let Ok(port) = port_str.parse::<u16>() {
                pairs.push((pid, port));
            }
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ss_output_extracts_ports() {
        let output = "\
State  Recv-Q Send-Q Local Address:Port  Peer Address:Port Process
LISTEN 0      511    0.0.0.0:5173        0.0.0.0:*         users:((\"node\",pid=12345,fd=19))
LISTEN 0      128    127.0.0.1:3000      0.0.0.0:*         users:((\"cargo\",pid=99999,fd=5))
LISTEN 0      128    0.0.0.0:8080        0.0.0.0:*         users:((\"java\",pid=12345,fd=10))
LISTEN 0      128    [::]:22             [::]:*             users:((\"sshd\",pid=1,fd=3))
";
        let pairs = parse_ss_output(output);
        let pids: HashSet<u32> = [12345].into_iter().collect();
        let ports = ports_for_pids(&pairs, &pids);
        assert_eq!(ports, vec![5173, 8080]);
    }

    #[test]
    fn parse_ss_output_no_match() {
        let output = "\
State  Recv-Q Send-Q Local Address:Port  Peer Address:Port Process
LISTEN 0      128    0.0.0.0:22          0.0.0.0:*         users:((\"sshd\",pid=1,fd=3))
";
        let pairs = parse_ss_output(output);
        let pids: HashSet<u32> = [9999].into_iter().collect();
        let ports = ports_for_pids(&pairs, &pids);
        assert!(ports.is_empty());
    }

    #[test]
    fn parse_ss_output_multiple_pids_in_users() {
        let output = "\
State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process
LISTEN 0 511 0.0.0.0:8000 0.0.0.0:* users:((\"python3\",pid=500,fd=3),(\"python3\",pid=501,fd=3))
";
        let pairs = parse_ss_output(output);
        let pids: HashSet<u32> = [501].into_iter().collect();
        let ports = ports_for_pids(&pairs, &pids);
        assert_eq!(ports, vec![8000]);
    }

    #[test]
    fn parse_lsof_output_extracts_ports() {
        let output = "\
COMMAND   PID  USER   FD   TYPE DEVICE SIZE/OFF NODE NAME
node    12345 user   19u  IPv4  12345      0t0  TCP *:5173 (LISTEN)
cargo   99999 user    5u  IPv4  99998      0t0  TCP 127.0.0.1:3000 (LISTEN)
node    12345 user   20u  IPv6  12346      0t0  TCP *:5173 (LISTEN)
";
        let pairs = parse_lsof_output(output);
        let pids: HashSet<u32> = [12345].into_iter().collect();
        let ports = ports_for_pids(&pairs, &pids);
        assert_eq!(ports, vec![5173]); // deduped by ports_for_pids
    }

    #[test]
    fn parse_netstat_output_extracts_ports() {
        let output = "\
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:8000           0.0.0.0:0              LISTENING       12345
  TCP    0.0.0.0:3000           0.0.0.0:0              LISTENING       99999
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1
  TCP    127.0.0.1:8000         127.0.0.1:50000        ESTABLISHED     12345
";
        let pairs = parse_netstat_output(output);
        let pids: HashSet<u32> = [12345].into_iter().collect();
        let ports = ports_for_pids(&pairs, &pids);
        assert_eq!(ports, vec![8000]);
    }

    #[test]
    fn parse_netstat_output_no_match() {
        let output = "\
  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       1
";
        let pairs = parse_netstat_output(output);
        let pids: HashSet<u32> = [9999].into_iter().collect();
        let ports = ports_for_pids(&pairs, &pids);
        assert!(ports.is_empty());
    }

    #[test]
    fn filtering_removes_debug_and_ephemeral_ports() {
        let pairs = vec![
            (1, 5173),
            (1, 9229),
            (1, 36435),
            (1, 37903),
            (1, 3000),
        ];
        let pids: HashSet<u32> = [1].into_iter().collect();
        let ports = ports_for_pids(&pairs, &pids);
        assert_eq!(ports, vec![3000, 5173]);
    }

    #[test]
    fn parse_ss_output_returns_all_pairs() {
        let output = "\
State  Recv-Q Send-Q Local Address:Port  Peer Address:Port Process
LISTEN 0      511    0.0.0.0:5173        0.0.0.0:*         users:((\"node\",pid=100,fd=19))
LISTEN 0      128    127.0.0.1:3000      0.0.0.0:*         users:((\"cargo\",pid=200,fd=5))
";
        let pairs = parse_ss_output(output);
        assert_eq!(pairs, vec![(100, 5173), (200, 3000)]);
    }

    #[test]
    fn ports_for_pids_shared_port_list() {
        // Two services sharing the same port scan results
        let pairs = vec![
            (100, 5173),
            (200, 3000),
            (300, 8080),
        ];
        let pids_a: HashSet<u32> = [100].into_iter().collect();
        let pids_b: HashSet<u32> = [200, 300].into_iter().collect();
        assert_eq!(ports_for_pids(&pairs, &pids_a), vec![5173]);
        assert_eq!(ports_for_pids(&pairs, &pids_b), vec![3000, 8080]);
    }

    #[test]
    fn descendants_from_tree_basic() {
        let mut tree = HashMap::new();
        tree.insert(1, vec![2, 3]);
        tree.insert(2, vec![4]);
        tree.insert(5, vec![6]);

        let desc = descendants_from_tree(&tree, 1);
        assert_eq!(desc, [1, 2, 3, 4].into_iter().collect::<HashSet<_>>());

        // Unrelated subtree not included
        let desc5 = descendants_from_tree(&tree, 5);
        assert_eq!(desc5, [5, 6].into_iter().collect::<HashSet<_>>());
    }
}
