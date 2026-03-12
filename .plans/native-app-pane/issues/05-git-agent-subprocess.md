# Issue 05: KruhPane git helpers and agent subprocess management

**Priority:** high
**Files:** `src/views/layout/kruh_pane/git.rs` (new), `src/views/layout/kruh_pane/agent.rs` (new)

## Description

Create the git helper functions and agent subprocess management module. These handle the external process interactions for the kruh loop.

## New file: `src/views/layout/kruh_pane/git.rs`

Port of kruh's `git.ts`. Uses `crate::process::command("git")` for cross-platform compatibility.

### `get_snapshot(project_path: &str) -> Option<String>`

```rust
use crate::process::command;

pub fn get_snapshot(project_path: &str) -> Option<String> {
    let output = command("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}
```

### `get_diff_stat(project_path: &str, before: &str, after: &str) -> Option<String>`

```rust
pub fn get_diff_stat(project_path: &str, before: &str, after: &str) -> Option<String> {
    let range = format!("{before}..{after}");
    let output = command("git")
        .args(["diff", "--stat", &range])
        .current_dir(project_path)
        .output()
        .ok()?;

    if output.status.success() {
        let stat = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stat.is_empty() { None } else { Some(stat) }
    } else {
        None
    }
}
```

### `get_working_tree_diff_stat(project_path: &str) -> Option<String>`

```rust
pub fn get_working_tree_diff_stat(project_path: &str) -> Option<String> {
    let output = command("git")
        .args(["diff", "--stat", "HEAD"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if output.status.success() {
        let stat = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stat.is_empty() { None } else { Some(stat) }
    } else {
        None
    }
}
```

## New file: `src/views/layout/kruh_pane/agent.rs`

Port of kruh's `agents.ts` process management. Uses the same pattern as Okena's PTY reader threads.

### `AgentHandle` struct

```rust
use std::process::Child;
use async_channel::Receiver;

pub struct AgentHandle {
    child: Child,
    pub stdout_receiver: Receiver<String>,
    pub stderr_receiver: Receiver<String>,
    reader_thread: Option<std::thread::JoinHandle<()>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}
```

### `spawn_agent(config: &KruhConfig, project_path: &str, prompt: &str) -> std::io::Result<AgentHandle>`

```rust
use std::process::{Command, Stdio};
use std::io::BufRead;
use super::config::{find_agent, KruhConfig};
use crate::process::command;

pub fn spawn_agent(config: &KruhConfig, project_path: &str, prompt: &str) -> std::io::Result<AgentHandle> {
    let agent_def = find_agent(&config.agent)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("Unknown agent: {}", config.agent)))?;

    let args = agent_def.build_command(config, prompt);
    // args[0] is the binary, rest are arguments

    let mut cmd = command(&args[0]);
    cmd.args(&args[1..])
        .current_dir(project_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;

    // Stdout reader thread
    let stdout = child.stdout.take().unwrap();
    let (stdout_tx, stdout_rx) = async_channel::unbounded();
    let reader_thread = std::thread::Builder::new()
        .name("kruh-agent-stdout".into())
        .spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if stdout_tx.send_blocking(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })?;

    // Stderr reader thread
    let stderr = child.stderr.take().unwrap();
    let (stderr_tx, stderr_rx) = async_channel::unbounded();
    let stderr_thread = std::thread::Builder::new()
        .name("kruh-agent-stderr".into())
        .spawn(move || {
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if stderr_tx.send_blocking(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })?;

    Ok(AgentHandle {
        child,
        stdout_receiver: stdout_rx,
        stderr_receiver: stderr_rx,
        reader_thread: Some(reader_thread),
        stderr_thread: Some(stderr_thread),
    })
}
```

### `AgentHandle` methods

```rust
impl AgentHandle {
    /// Check if the agent process has exited. Returns exit code if done.
    pub fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        self.child.try_wait().map(|status| status.map(|s| s.code().unwrap_or(-1)))
    }

    /// Kill the agent process. Sends SIGTERM first, SIGKILL after 3 seconds.
    pub fn kill(&mut self) {
        // First try graceful termination
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }

        // Wait up to 3 seconds, then force kill
        let start = std::time::Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {
                    if start.elapsed() > std::time::Duration::from_secs(3) {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(_) => return,
            }
        }
    }
}

impl Drop for AgentHandle {
    fn drop(&mut self) {
        self.kill();
        if let Some(thread) = self.reader_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}
```

Note: The `libc` crate may need to be added to Cargo.toml if not already present (check first — it's common in Rust projects). Alternatively, use `nix::sys::signal` if the `nix` crate is available. On unix systems `self.child.kill()` sends SIGKILL directly — the graceful SIGTERM approach is preferred.

## Acceptance Criteria

- `get_snapshot()` returns HEAD commit hash
- `get_diff_stat()` returns diff stat between two commits
- `spawn_agent()` correctly builds and spawns the agent process
- Stdout/stderr are streamed line-by-line through async channels
- `kill()` gracefully terminates with SIGTERM → SIGKILL escalation
- `Drop` impl ensures cleanup
- `cargo build` succeeds
