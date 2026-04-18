//! Start / stop / restart individual services, plus PTY-exit handling.

use super::{MAX_RESTART_COUNT, ServiceKind, ServiceManager, ServiceStatus, is_process_alive};
use crate::port_detect;
use gpui::{Context, WeakEntity};
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::{Terminal, TerminalSize};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

impl ServiceManager {
    /// Start a service by spawning a PTY (Okena) or running `docker compose start` (Docker).
    pub fn start_service(
        &mut self,
        project_id: &str,
        service_name: &str,
        project_path: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (project_id.to_string(), service_name.to_string());
        let instance = match self.instances.get_mut(&key) {
            Some(i) => i,
            None => {
                log::error!(
                    "Cannot start unknown service '{}' for project {}",
                    service_name,
                    project_id
                );
                return;
            }
        };

        // Don't start if already running
        if instance.status == ServiceStatus::Running || instance.status == ServiceStatus::Starting {
            return;
        }

        match &instance.kind {
            ServiceKind::DockerCompose { compose_file } => {
                let compose_file = compose_file.clone();
                let path = project_path.to_string();
                let name = service_name.to_string();
                instance.status = ServiceStatus::Starting;
                cx.notify();

                // Fire-and-forget: status poller will pick up the change
                let log_name = name.clone();
                cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
                    let result = cx.background_executor()
                        .spawn(async move {
                            let mut cmd = okena_core::process::command("docker");
                            cmd.args(["compose", "-f", &compose_file, "start", &name])
                                .current_dir(&path);
                            okena_core::process::safe_output(&mut cmd)
                        })
                        .await;
                    if let Ok(output) = result {
                        if !output.status.success() {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            log::error!("docker compose start failed for '{}': {}", log_name, stderr.trim());
                        }
                    }
                    // Trigger an immediate status poll
                    let _ = this.update(cx, |_this, cx| cx.notify());
                }).detach();
            }
            ServiceKind::Okena => {
                self.start_okena_service(project_id, service_name, project_path, cx);
            }
        }
    }

    /// Start an Okena service by spawning a PTY with the service command.
    fn start_okena_service(
        &mut self,
        project_id: &str,
        service_name: &str,
        project_path: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (project_id.to_string(), service_name.to_string());
        let instance = match self.instances.get_mut(&key) {
            Some(i) => i,
            None => return,
        };

        // Clean up old terminal from a previous crash (kept for viewing crash output)
        if let Some(old_tid) = instance.terminal_id.take() {
            self.terminals.lock().remove(&old_tid);
            self.terminal_to_service.remove(&old_tid);
        }

        let command = instance.definition.command.clone();
        let cwd_relative = instance.definition.cwd.clone();
        let cwd = Path::new(project_path)
            .join(&cwd_relative)
            .to_string_lossy()
            .to_string();

        let shell = ShellType::for_command(command);

        instance.status = ServiceStatus::Starting;

        match self.backend.create_terminal(&cwd, Some(&shell)) {
            Ok(terminal_id) => {
                let terminal = Arc::new(Terminal::new(
                    terminal_id.clone(),
                    TerminalSize::default(),
                    self.backend.transport(),
                    cwd,
                ));
                self.terminals.lock().insert(terminal_id.clone(), terminal);

                #[allow(
                    clippy::expect_used,
                    reason = "instance set to Starting a few lines above, absence is a bug"
                )]
                let instance = self.instances.get_mut(&key).expect("bug: service instance must exist");
                instance.status = ServiceStatus::Running;
                instance.terminal_id = Some(terminal_id.clone());
                self.terminal_to_service.insert(
                    terminal_id,
                    (project_id.to_string(), service_name.to_string()),
                );
            }
            Err(e) => {
                log::error!(
                    "Failed to start service '{}' for project {}: {}",
                    service_name,
                    project_id,
                    e
                );
                #[allow(
                    clippy::expect_used,
                    reason = "instance set to Starting a few lines above, absence is a bug"
                )]
                let instance = self.instances.get_mut(&key).expect("bug: service instance must exist");
                instance.status = ServiceStatus::Crashed { exit_code: None };
            }
        }

        cx.notify();

        // Start port detection if service is now running
        if self.instances.get(&key).is_some_and(|i| i.status == ServiceStatus::Running) {
            self.start_port_detection(project_id, service_name, cx);
        }
    }

    /// Stop a running service.
    pub fn stop_service(
        &mut self,
        project_id: &str,
        service_name: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (project_id.to_string(), service_name.to_string());
        let instance = match self.instances.get_mut(&key) {
            Some(i) => i,
            None => return,
        };

        // Kill log viewer PTY if any (for both kinds)
        if let Some(terminal_id) = instance.terminal_id.take() {
            self.backend.kill(&terminal_id);
            self.terminals.lock().remove(&terminal_id);
            self.terminal_to_service.remove(&terminal_id);
        }

        match &instance.kind {
            ServiceKind::DockerCompose { compose_file } => {
                let compose_file = compose_file.clone();
                let path = self.project_paths.get(project_id).cloned().unwrap_or_default();
                let name = service_name.to_string();
                instance.status = ServiceStatus::Stopped;
                instance.detected_ports.clear();
                cx.notify();

                cx.spawn(async move |_this: WeakEntity<ServiceManager>, cx| {
                    cx.background_executor()
                        .spawn(async move {
                            let mut cmd = okena_core::process::command("docker");
                            cmd.args(["compose", "-f", &compose_file, "stop", &name])
                                .current_dir(&path);
                            let _ = okena_core::process::safe_output(&mut cmd);
                        })
                        .await;
                }).detach();
            }
            ServiceKind::Okena => {
                instance.status = ServiceStatus::Stopped;
                instance.restart_count = 0;
                instance.detected_ports.clear();
                cx.notify();
            }
        }
    }

    /// Restart a service: kill the old process, wait for it to die, then start a new one.
    pub fn restart_service(
        &mut self,
        project_id: &str,
        service_name: &str,
        project_path: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (project_id.to_string(), service_name.to_string());
        let instance = match self.instances.get_mut(&key) {
            Some(i) => i,
            None => return,
        };

        match &instance.kind {
            ServiceKind::DockerCompose { compose_file } => {
                let compose_file = compose_file.clone();
                let path = project_path.to_string();
                let name = service_name.to_string();

                // Kill log viewer PTY if any
                if let Some(terminal_id) = instance.terminal_id.take() {
                    self.backend.kill(&terminal_id);
                    self.terminals.lock().remove(&terminal_id);
                    self.terminal_to_service.remove(&terminal_id);
                }

                instance.status = ServiceStatus::Restarting;
                instance.detected_ports.clear();
                cx.notify();

                cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
                    cx.background_executor()
                        .spawn(async move {
                            let mut cmd = okena_core::process::command("docker");
                            cmd.args(["compose", "-f", &compose_file, "restart", &name])
                                .current_dir(&path);
                            let _ = okena_core::process::safe_output(&mut cmd);
                        })
                        .await;
                    let _ = this.update(cx, |_this, cx| cx.notify());
                }).detach();
            }
            ServiceKind::Okena => {
                // Take terminal_id now to prevent concurrent access.
                // The PtyManager handle is NOT removed yet — that happens in kill() below.
                let terminal_id = instance.terminal_id.take();

                instance.status = ServiceStatus::Restarting;
                instance.restart_count = 0;
                instance.detected_ports.clear();
                cx.notify();

                let pid = project_id.to_string();
                let name = service_name.to_string();
                let path = project_path.to_string();
                let backend = self.backend.clone();
                let terminals = self.terminals.clone();

                cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
                    // Collect descendant PIDs on background executor.
                    // get_service_pids() may spawn subprocesses (lsof/tmux)
                    // and get_descendant_pids() may call pgrep/wmic.
                    let old_pids: Vec<u32> = if let Some(ref tid) = terminal_id {
                        let tid = tid.clone();
                        let backend_ref = backend.clone();
                        cx.background_executor()
                            .spawn(async move {
                                backend_ref.get_service_pids(&tid)
                                    .into_iter()
                                    .flat_map(|p| port_detect::get_descendant_pids(p))
                                    .collect()
                            })
                            .await
                    } else {
                        Vec::new()
                    };

                    // Kill old terminal (backend.kill spawns a bg thread internally)
                    if let Some(ref tid) = terminal_id {
                        backend.kill(tid);
                        terminals.lock().remove(tid);
                        let tid = tid.clone();
                        let _ = this.update(cx, |this, _cx| {
                            this.terminal_to_service.remove(&tid);
                        });
                    }

                    // Wait for old processes to die
                    if !old_pids.is_empty() {
                        for _ in 0..100 {
                            if old_pids.iter().all(|&p| !is_process_alive(p)) {
                                break;
                            }
                            cx.background_executor().timer(Duration::from_millis(50)).await;
                        }
                    }

                    let _ = this.update(cx, |this, cx| {
                        let key = (pid.clone(), name.clone());
                        if let Some(instance) = this.instances.get(&key) {
                            if instance.status == ServiceStatus::Restarting {
                                this.start_service(&pid, &name, &path, cx);
                            }
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// Start all services for a project.
    pub fn start_all(
        &mut self,
        project_id: &str,
        project_path: &str,
        cx: &mut Context<Self>,
    ) {
        let names: Vec<String> = self
            .instances
            .keys()
            .filter(|(pid, _)| pid == project_id)
            .map(|(_, name)| name.clone())
            .collect();

        for name in names {
            self.start_service(project_id, &name, project_path, cx);
        }
    }

    /// Stop all services for a project.
    pub fn stop_all(&mut self, project_id: &str, cx: &mut Context<Self>) {
        let names: Vec<String> = self
            .instances
            .keys()
            .filter(|(pid, _)| pid == project_id)
            .map(|(_, name)| name.clone())
            .collect();

        for name in names {
            self.stop_service(project_id, &name, cx);
        }
    }

    /// Handle a terminal exit event. If the terminal belongs to a service,
    /// handle crash/restart logic. Returns `true` if this was a service terminal.
    pub fn handle_service_exit(
        &mut self,
        terminal_id: &str,
        exit_code: Option<u32>,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = match self.terminal_to_service.remove(terminal_id) {
            Some(key) => key,
            None => return false, // Not a service terminal
        };

        let (project_id, service_name) = key.clone();

        let instance = match self.instances.get_mut(&key) {
            Some(i) => i,
            None => return true,
        };

        // Docker log viewer PTY exited — clear terminal_id but don't change service status.
        // The Docker status poller manages the service status independently.
        if matches!(instance.kind, ServiceKind::DockerCompose { .. }) {
            instance.terminal_id = None;
            self.terminals.lock().remove(terminal_id);
            cx.notify();
            return true;
        }

        // Okena service exit handling
        instance.detected_ports.clear();

        if instance.definition.restart_on_crash && instance.restart_count < MAX_RESTART_COUNT {
            // Auto-restart: clean up old terminal, will create new one
            instance.terminal_id = None;
            self.terminals.lock().remove(terminal_id);
            instance.status = ServiceStatus::Restarting;
            instance.restart_count += 1;

            let delay = Duration::from_millis(instance.definition.restart_delay_ms);

            cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(project_path) = this.project_paths.get(&project_id).cloned() {
                        this.start_service(&project_id, &service_name, &project_path, cx);
                    }
                });
            })
            .detach();
        } else {
            // Crash without restart: keep terminal_id and Terminal in registry
            // so the user can see the crash output until they manually restart.
            instance.status = ServiceStatus::Crashed { exit_code };
        }

        cx.notify();
        true
    }
}
