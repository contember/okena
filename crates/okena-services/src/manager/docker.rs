//! Docker Compose service discovery, log-viewer PTYs, and status polling.

use super::{ServiceInstance, ServiceKind, ServiceManager, ServiceStatus};
use crate::config::ServiceDefinition;
use crate::docker_compose;
use gpui::{Context, WeakEntity};
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::{Terminal, TerminalSize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

impl ServiceManager {
    /// Load Docker Compose services for a project.
    /// If `docker_config` is None, auto-detects compose file.
    /// If `docker_config.enabled` is explicitly false, skips.
    pub(super) fn load_docker_compose_services(
        &mut self,
        project_id: &str,
        project_path: &str,
        docker_config: Option<&crate::config::DockerComposeConfig>,
        cx: &mut Context<Self>,
    ) {
        // Check if explicitly disabled
        if docker_config.as_ref().is_some_and(|dc| dc.enabled == Some(false)) {
            return;
        }

        // Resolve compose file (fast filesystem check, OK on main thread)
        let compose_file = docker_config
            .and_then(|dc| dc.file.clone())
            .or_else(|| docker_compose::detect_compose_file(project_path));

        let Some(compose_file) = compose_file else { return };

        // Extract what we need from the reference before spawning
        let filter: Option<Vec<String>> = docker_config
            .map(|dc| dc.services.clone())
            .filter(|s| !s.is_empty());

        let project_id = project_id.to_string();
        let project_path = project_path.to_string();

        // Move docker subprocess calls to background executor
        cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
            let service_names = {
                let path = project_path.clone();
                let file = compose_file.clone();
                smol::unblock(move || {
                    if !docker_compose::is_docker_compose_available() {
                        return None;
                    }
                    match docker_compose::list_services(&path, &file) {
                        Ok(names) => Some(names),
                        Err(e) => {
                            log::warn!("Failed to list Docker Compose services: {}", e);
                            None
                        }
                    }
                })
                .await
            };

            let Some(service_names) = service_names else { return };

            let _ = this.update(cx, |this, cx| {
                for name in &service_names {
                    let is_extra = filter.as_ref().is_some_and(|f| !f.contains(name));

                    let key = (project_id.clone(), name.clone());
                    if !this.instances.contains_key(&key) {
                        this.instances.insert(
                            key,
                            ServiceInstance {
                                definition: ServiceDefinition {
                                    name: name.clone(),
                                    command: String::new(),
                                    cwd: ".".to_string(),
                                    env: HashMap::new(),
                                    auto_start: false,
                                    restart_on_crash: false,
                                    restart_delay_ms: 0,
                                },
                                kind: ServiceKind::DockerCompose { compose_file: compose_file.clone() },
                                status: ServiceStatus::Stopped,
                                terminal_id: None,
                                restart_count: 0,
                                detected_ports: Vec::new(),
                                is_extra,
                            },
                        );
                    }
                }

                // Start status poller
                this.start_docker_status_poller(&project_id, &project_path, &compose_file, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reload Docker Compose services on config reload.
    pub(super) fn reload_docker_compose_services(
        &mut self,
        project_id: &str,
        project_path: &str,
        docker_config: Option<&crate::config::DockerComposeConfig>,
        cx: &mut Context<Self>,
    ) {
        // Stop existing poller
        if let Some(cancel) = self.docker_pollers.remove(project_id) {
            cancel.store(true, Ordering::Relaxed);
        }

        // Remove old Docker instances
        let docker_keys: Vec<(String, String)> = self.instances
            .iter()
            .filter(|((pid, _), inst)| pid == project_id && matches!(inst.kind, ServiceKind::DockerCompose { .. }))
            .map(|(k, _)| k.clone())
            .collect();

        for key in docker_keys {
            if let Some(instance) = self.instances.get(&key) {
                if let Some(terminal_id) = &instance.terminal_id {
                    self.backend.kill(terminal_id);
                    self.terminals.lock().remove(terminal_id);
                    self.terminal_to_service.remove(terminal_id);
                }
            }
            self.instances.remove(&key);
        }

        // Reload
        self.load_docker_compose_services(project_id, project_path, docker_config, cx);
    }

    /// Spawn a PTY running `docker compose logs -f --tail 200 <name>`.
    /// Stores the terminal_id on the instance.
    pub fn open_docker_logs(
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

        let compose_file = match &instance.kind {
            ServiceKind::DockerCompose { compose_file } => compose_file.clone(),
            ServiceKind::Okena => return,
        };

        // Kill existing log viewer if any
        if let Some(old_tid) = instance.terminal_id.take() {
            self.backend.kill(&old_tid);
            self.terminals.lock().remove(&old_tid);
            self.terminal_to_service.remove(&old_tid);
        }

        let project_path = match self.project_paths.get(project_id) {
            Some(p) => p.clone(),
            None => return,
        };

        let command = format!(
            "docker compose -f {} logs -f --tail 200 {}",
            compose_file, service_name
        );

        let shell = ShellType::for_command(command);

        match self.backend.create_terminal(&project_path, Some(&shell)) {
            Ok(terminal_id) => {
                let terminal = Arc::new(Terminal::new(
                    terminal_id.clone(),
                    TerminalSize::default(),
                    self.backend.transport(),
                    project_path,
                ));
                self.terminals.lock().insert(terminal_id.clone(), terminal);

                #[allow(
                    clippy::expect_used,
                    reason = "Docker log instance ensured earlier in this function, absence is a bug"
                )]
                let instance = self.instances.get_mut(&key).expect("bug: service instance must exist");
                instance.terminal_id = Some(terminal_id.clone());
                self.terminal_to_service.insert(
                    terminal_id,
                    (project_id.to_string(), service_name.to_string()),
                );
            }
            Err(e) => {
                log::error!(
                    "Failed to open Docker logs for '{}' in project {}: {}",
                    service_name, project_id, e
                );
            }
        }

        cx.notify();
    }

    /// Start a background poller that updates Docker service statuses every 5s.
    fn start_docker_status_poller(
        &mut self,
        project_id: &str,
        project_path: &str,
        compose_file: &str,
        cx: &mut Context<Self>,
    ) {
        // Cancel any existing poller for this project
        if let Some(old_cancel) = self.docker_pollers.remove(project_id) {
            old_cancel.store(true, Ordering::Relaxed);
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.docker_pollers.insert(project_id.to_string(), cancel.clone());

        let pid = project_id.to_string();
        let path = project_path.to_string();
        let file = compose_file.to_string();

        cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
            // Small initial delay
            cx.background_executor().timer(Duration::from_secs(1)).await;

            let mut consecutive_failures: u32 = 0;

            loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }

                let path_clone = path.clone();
                let file_clone = file.clone();
                let result = smol::unblock(move || {
                    docker_compose::poll_status(&path_clone, &file_clone)
                })
                .await;

                if cancel.load(Ordering::Relaxed) {
                    return;
                }

                match result {
                    Ok(statuses) => {
                        consecutive_failures = 0;
                        let should_stop = this.update(cx, |this, cx| {
                            let mut any_docker = false;
                            let mut changed = false;
                            for ds in &statuses {
                                let key = (pid.clone(), ds.name.clone());
                                if let Some(inst) = this.instances.get_mut(&key) {
                                    if matches!(inst.kind, ServiceKind::DockerCompose { .. }) {
                                        any_docker = true;
                                        let new_status = docker_compose::map_docker_state(&ds.state, ds.exit_code);
                                        if inst.status != new_status {
                                            inst.status = new_status;
                                            changed = true;
                                        }
                                        if inst.detected_ports != ds.ports {
                                            inst.detected_ports = ds.ports.clone();
                                            changed = true;
                                        }
                                    }
                                }
                            }
                            if changed {
                                cx.notify();
                            }
                            !any_docker
                        }).unwrap_or(true);

                        if should_stop {
                            return;
                        }
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        log::warn!("Docker status poll failed for project {}: {}", pid, e);
                    }
                }

                // Back off on repeated failures: 5s → 10s → 20s → 40s → 60s (cap)
                let delay = if consecutive_failures == 0 {
                    5
                } else {
                    (5u64 << consecutive_failures.min(4)).min(60)
                };
                cx.background_executor().timer(Duration::from_secs(delay)).await;
            }
        }).detach();
    }
}
