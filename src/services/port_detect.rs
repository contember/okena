use crate::process::{command, safe_output};
use std::collections::{HashSet, VecDeque};

/// Ports to exclude from detection results.
/// 9229 = Node.js inspector/debugger
const IGNORED_PORTS: &[u16] = &[9229];

/// Ports at or above this threshold are considered ephemeral/internal and excluded.
/// Linux default ephemeral range starts at 32768.
const EPHEMERAL_PORT_MIN: u16 = 32768;

/// Detect TCP ports that a service process (or any of its descendants) is listening on.
/// Filters out known debug ports and ephemeral ports.
pub fn detect_ports_for_pid(pid: u32) -> Vec<u16> {
    let pids = get_descendant_pids(pid);
    let mut ports = get_listening_ports(&pids);
    ports.retain(|p| *p < EPHEMERAL_PORT_MIN && !IGNORED_PORTS.contains(p));
    ports.sort();
    ports.dedup();
    ports
}

/// Walk the process tree to find all descendant PIDs (children, grandchildren, etc.)
/// including the root PID itself.
fn get_descendant_pids(root_pid: u32) -> HashSet<u32> {
    let mut result = HashSet::new();
    result.insert(root_pid);

    #[cfg(target_os = "linux")]
    {
        get_descendant_pids_linux(root_pid, &mut result);
    }

    #[cfg(target_os = "macos")]
    {
        get_descendant_pids_macos(root_pid, &mut result);
    }

    #[cfg(windows)]
    {
        get_descendant_pids_windows(root_pid, &mut result);
    }

    result
}

#[cfg(target_os = "linux")]
fn get_descendant_pids_linux(root_pid: u32, result: &mut HashSet<u32>) {
    // Read /proc to build a parent→children map, then BFS from root_pid.
    use std::fs;

    let mut parent_to_children: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();

    let entries = match fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return,
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
                    parent_to_children.entry(ppid).or_default().push(pid);
                }
            }
        }
    }

    // BFS from root_pid
    let mut queue = VecDeque::new();
    queue.push_back(root_pid);
    while let Some(pid) = queue.pop_front() {
        if let Some(children) = parent_to_children.get(&pid) {
            for &child in children {
                if result.insert(child) {
                    queue.push_back(child);
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn get_descendant_pids_macos(root_pid: u32, result: &mut HashSet<u32>) {
    // Use pgrep -P recursively to find children.
    let mut queue = VecDeque::new();
    queue.push_back(root_pid);

    while let Some(pid) = queue.pop_front() {
        let mut cmd = command("pgrep");
        cmd.args(["-P", &pid.to_string()]);
        if let Ok(output) = safe_output(&mut cmd) {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if let Ok(child_pid) = line.trim().parse::<u32>() {
                        if result.insert(child_pid) {
                            queue.push_back(child_pid);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
fn get_descendant_pids_windows(root_pid: u32, result: &mut HashSet<u32>) {
    // Use wmic to find children recursively.
    let mut queue = VecDeque::new();
    queue.push_back(root_pid);

    while let Some(pid) = queue.pop_front() {
        let mut cmd = command("wmic");
        cmd.args([
            "process",
            "where",
            &format!("(ParentProcessId={})", pid),
            "get",
            "ProcessId",
        ]);
        if let Ok(output) = safe_output(&mut cmd) {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines().skip(1) {
                    // Skip header
                    if let Ok(child_pid) = line.trim().parse::<u32>() {
                        if result.insert(child_pid) {
                            queue.push_back(child_pid);
                        }
                    }
                }
            }
        }
    }
}

/// Get TCP ports in LISTEN state owned by any of the given PIDs.
fn get_listening_ports(pids: &HashSet<u32>) -> Vec<u16> {
    #[cfg(target_os = "linux")]
    {
        get_listening_ports_linux(pids)
    }

    #[cfg(target_os = "macos")]
    {
        get_listening_ports_macos(pids)
    }

    #[cfg(windows)]
    {
        get_listening_ports_windows(pids)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = pids;
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn get_listening_ports_linux(pids: &HashSet<u32>) -> Vec<u16> {
    // Parse `ss -tlnp` output.
    // Lines like: LISTEN 0 511 0.0.0.0:5173 0.0.0.0:* users:(("node",pid=12345,fd=19))
    let mut cmd = command("ss");
    cmd.args(["-tlnp"]);
    let output = match safe_output(&mut cmd) {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ss_output(&stdout, pids)
}

pub(crate) fn parse_ss_output(stdout: &str, pids: &HashSet<u32>) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in stdout.lines() {
        // Extract PIDs from users:((...,pid=NNNN,...)) section
        let line_pids = extract_pids_from_ss_line(line);
        if line_pids.iter().any(|p| pids.contains(p)) {
            if let Some(port) = extract_port_from_ss_line(line) {
                ports.push(port);
            }
        }
    }
    ports
}

fn extract_pids_from_ss_line(line: &str) -> Vec<u32> {
    // Look for pid=NNNN patterns in the line
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
    // Fields: State Recv-Q Send-Q Local_Address:Port Peer_Address:Port Process
    // Local address field is typically the 4th whitespace-separated token.
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 5 {
        return None;
    }
    // Local address is fields[3] (0-indexed) for lines starting with LISTEN
    let local_addr = fields[3];
    // Port is after the last ':'
    let port_str = local_addr.rsplit(':').next()?;
    port_str.parse().ok()
}

#[cfg(target_os = "macos")]
fn get_listening_ports_macos(pids: &HashSet<u32>) -> Vec<u16> {
    // Parse `lsof -iTCP -sTCP:LISTEN -P -n` output.
    // Columns: COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
    let mut cmd = command("lsof");
    cmd.args(["-iTCP", "-sTCP:LISTEN", "-P", "-n"]);
    let output = match safe_output(&mut cmd) {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_lsof_output(&stdout, pids)
}

pub(crate) fn parse_lsof_output(stdout: &str, pids: &HashSet<u32>) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in stdout.lines().skip(1) {
        // Skip header
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        let pid: u32 = match fields[1].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !pids.contains(&pid) {
            continue;
        }
        // NAME field like "*:5173" or "127.0.0.1:3000"
        let name = fields[8];
        if let Some(port_str) = name.rsplit(':').next() {
            if let Ok(port) = port_str.parse::<u16>() {
                ports.push(port);
            }
        }
    }
    ports
}

#[cfg(windows)]
fn get_listening_ports_windows(pids: &HashSet<u32>) -> Vec<u16> {
    // Parse `netstat -ano` output, filtering for LISTENING state.
    // Columns: Proto LocalAddr ForeignAddr State PID
    let mut cmd = command("netstat");
    cmd.args(["-ano"]);
    let output = match safe_output(&mut cmd) {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_netstat_output(&stdout, pids)
}

pub(crate) fn parse_netstat_output(stdout: &str, pids: &HashSet<u32>) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.contains("LISTENING") {
            continue;
        }
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        // PID is the last field
        let pid: u32 = match fields[4].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !pids.contains(&pid) {
            continue;
        }
        // Local address is fields[1], port after last ':'
        let local_addr = fields[1];
        if let Some(port_str) = local_addr.rsplit(':').next() {
            if let Ok(port) = port_str.parse::<u16>() {
                ports.push(port);
            }
        }
    }
    ports
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
        let pids: HashSet<u32> = [12345].into_iter().collect();
        let ports = parse_ss_output(output, &pids);
        assert_eq!(ports, vec![5173, 8080]);
    }

    #[test]
    fn parse_ss_output_no_match() {
        let output = "\
State  Recv-Q Send-Q Local Address:Port  Peer Address:Port Process
LISTEN 0      128    0.0.0.0:22          0.0.0.0:*         users:((\"sshd\",pid=1,fd=3))
";
        let pids: HashSet<u32> = [9999].into_iter().collect();
        let ports = parse_ss_output(output, &pids);
        assert!(ports.is_empty());
    }

    #[test]
    fn parse_ss_output_multiple_pids_in_users() {
        // Some lines can have multiple pid= entries
        let output = "\
State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process
LISTEN 0 511 0.0.0.0:8000 0.0.0.0:* users:((\"python3\",pid=500,fd=3),(\"python3\",pid=501,fd=3))
";
        let pids: HashSet<u32> = [501].into_iter().collect();
        let ports = parse_ss_output(output, &pids);
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
        let pids: HashSet<u32> = [12345].into_iter().collect();
        let ports = parse_lsof_output(output, &pids);
        assert_eq!(ports, vec![5173, 5173]); // caller deduplicates
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
        let pids: HashSet<u32> = [12345].into_iter().collect();
        let ports = parse_netstat_output(output, &pids);
        assert_eq!(ports, vec![8000]);
    }

    #[test]
    fn parse_netstat_output_no_match() {
        let output = "\
  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       1
";
        let pids: HashSet<u32> = [9999].into_iter().collect();
        let ports = parse_netstat_output(output, &pids);
        assert!(ports.is_empty());
    }

    #[test]
    fn filtering_removes_debug_and_ephemeral_ports() {
        // Simulate what detect_ports_for_pid does after get_listening_ports
        let mut ports = vec![5173, 9229, 36435, 37903, 3000];
        ports.retain(|p| *p < EPHEMERAL_PORT_MIN && !IGNORED_PORTS.contains(p));
        ports.sort();
        ports.dedup();
        assert_eq!(ports, vec![3000, 5173]);
    }
}
