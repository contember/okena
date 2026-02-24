use std::io::BufRead;
use std::process::{Child, Stdio};

use async_channel::Receiver;

use super::config::{find_agent, KruhConfig};
use crate::process::command;

pub struct AgentHandle {
    child: Child,
    pub stdout_receiver: Receiver<String>,
    pub stderr_receiver: Receiver<String>,
    reader_thread: Option<std::thread::JoinHandle<()>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

pub fn spawn_agent(
    config: &KruhConfig,
    project_path: &str,
    prompt: &str,
) -> std::io::Result<AgentHandle> {
    let agent_def = find_agent(&config.agent).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Unknown agent: {}", config.agent),
        )
    })?;

    let args = agent_def.build_command(config, prompt);

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

impl AgentHandle {
    /// Check if the agent process has exited. Returns exit code if done.
    pub fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        self.child
            .try_wait()
            .map(|status| status.map(|s| s.code().unwrap_or(-1)))
    }

    /// Kill the agent process. Sends SIGTERM first, SIGKILL after 3 seconds.
    pub fn kill(&mut self) {
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }

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
