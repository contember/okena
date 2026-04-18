use crate::config::{load_project_config, ServiceDefinition};
use crate::docker_compose;
use crate::port_detect;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::{Terminal, TerminalSize};
use okena_terminal::TerminalsRegistry;
use gpui::{Context, WeakEntity};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct ServiceManager {
    configs: HashMap<String, Vec<ServiceDefinition>>,
    instances: HashMap<(String, String), ServiceInstance>,
    terminal_to_service: HashMap<String, (String, String)>,
    project_paths: HashMap<String, String>,
    backend: Arc<dyn TerminalBackend>,
    terminals: TerminalsRegistry,
    /// Cancel tokens for Docker status pollers (project_id -> cancel flag)
    docker_pollers: HashMap<String, Arc<AtomicBool>>,
    /// Services currently undergoing port detection.
    port_detection_active: HashMap<(String, String), PortDetectionState>,
    /// Whether the centralized port detection poller task is running.
    port_detection_running: bool,
}

struct PortDetectionState {
    polls_remaining: u32,
    found_any: bool,
    stable_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServiceKind {
    Okena,
    DockerCompose { compose_file: String },
}

pub struct ServiceInstance {
    pub definition: ServiceDefinition,
    pub kind: ServiceKind,
    pub status: ServiceStatus,
    /// For Okena services: the process PTY. For Docker services: the log viewer PTY (ephemeral).
    pub terminal_id: Option<String>,
    pub restart_count: u32,
    pub detected_ports: Vec<u16>,
    /// Docker service not listed in okena.yaml filter — shown in "Other" section.
    pub is_extra: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServiceStatus {
    Stopped,
    Starting,
    Running,
    Crashed { exit_code: Option<u32> },
    Restarting,
}

impl ServiceStatus {
    /// Convert an API status string (from `ApiServiceInfo.status`) into a `ServiceStatus`.
    pub fn from_api(status: &str, exit_code: Option<u32>) -> Self {
        match status {
            "running" => Self::Running,
            "starting" => Self::Starting,
            "restarting" => Self::Restarting,
            "crashed" => Self::Crashed { exit_code },
            _ => Self::Stopped,
        }
    }
}

const MAX_RESTART_COUNT: u32 = 5;

impl ServiceManager {
    pub fn new(backend: Arc<dyn TerminalBackend>, terminals: TerminalsRegistry) -> Self {
        Self {
            configs: HashMap::new(),
            instances: HashMap::new(),
            terminal_to_service: HashMap::new(),
            project_paths: HashMap::new(),
            backend,
            terminals,
            docker_pollers: HashMap::new(),
            port_detection_active: HashMap::new(),
            port_detection_running: false,
        }
    }

    /// Parse `okena.yaml` for a project, create `ServiceInstance` entries,
    /// reconnect to saved sessions, and auto-start services where configured.
    /// Also loads Docker Compose services if detected.
    pub fn load_project_services(
        &mut self,
        project_id: &str,
        project_path: &str,
        saved_terminal_ids: &HashMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        log::info!("[services] load_project_services project_id={} path={}", project_id, project_path);
        let config = match load_project_config(project_path) {
            Ok(Some(config)) => {
                log::info!("[services] Found okena.yaml with {} services", config.services.len());
                config
            }
            Ok(None) => {
                log::info!("[services] No okena.yaml found at {}", project_path);
                // No okena.yaml — still try Docker Compose auto-detection
                self.project_paths
                    .insert(project_id.to_string(), project_path.to_string());
                self.load_docker_compose_services(project_id, project_path, None, cx);
                return;
            }
            Err(e) => {
                log::error!("Failed to load okena.yaml for project {}: {}", project_id, e);
                return;
            }
        };

        self.project_paths
            .insert(project_id.to_string(), project_path.to_string());

        let auto_start_names: Vec<String> = config
            .services
            .iter()
            .filter(|s| s.auto_start)
            .map(|s| s.name.clone())
            .collect();

        for def in &config.services {
            let key = (project_id.to_string(), def.name.clone());
            self.instances.insert(
                key,
                ServiceInstance {
                    definition: def.clone(),
                    kind: ServiceKind::Okena,
                    status: ServiceStatus::Stopped,
                    terminal_id: None,
                    restart_count: 0,
                    detected_ports: Vec::new(),
                    is_extra: false,
                },
            );
        }

        self.configs
            .insert(project_id.to_string(), config.services);

        // Try to reconnect services that have saved terminal IDs
        for def in self.configs.get(project_id).cloned().unwrap_or_default() {
            if let Some(saved_id) = saved_terminal_ids.get(&def.name) {
                self.reconnect_service(project_id, &def.name, project_path, saved_id, cx);
            }
        }

        // Auto-start services that weren't reconnected
        for name in auto_start_names {
            let key = (project_id.to_string(), name.clone());
            if let Some(instance) = self.instances.get(&key) {
                if instance.status == ServiceStatus::Stopped {
                    self.start_service(project_id, &name, project_path, cx);
                }
            }
        }

        // Load Docker Compose services
        self.load_docker_compose_services(project_id, project_path, config.docker_compose.as_ref(), cx);

        cx.notify();
    }

    /// Try to reconnect a service to an existing session backend session.
    fn reconnect_service(
        &mut self,
        project_id: &str,
        service_name: &str,
        project_path: &str,
        saved_terminal_id: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (project_id.to_string(), service_name.to_string());
        let instance = match self.instances.get_mut(&key) {
            Some(i) => i,
            None => return,
        };

        let command = instance.definition.command.clone();
        let cwd_relative = instance.definition.cwd.clone();
        let cwd = Path::new(project_path)
            .join(&cwd_relative)
            .to_string_lossy()
            .to_string();

        let shell = ShellType::for_command(command);

        match self.backend.reconnect_terminal(saved_terminal_id, &cwd, Some(&shell)) {
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
                    reason = "instance inserted earlier in this function, absence is a bug"
                )]
                let instance = self.instances.get_mut(&key).expect("bug: service instance must exist");
                instance.status = ServiceStatus::Running;
                instance.terminal_id = Some(terminal_id.clone());
                self.terminal_to_service.insert(
                    terminal_id,
                    (project_id.to_string(), service_name.to_string()),
                );
                log::info!("Reconnected service '{}' for project {} (terminal {})", service_name, project_id, saved_terminal_id);
                self.start_port_detection(project_id, service_name, cx);
            }
            Err(e) => {
                log::warn!(
                    "Failed to reconnect service '{}' for project {} (terminal {}): {}",
                    service_name, project_id, saved_terminal_id, e
                );
                // Leave as Stopped — auto_start will create a fresh terminal if configured
            }
        }

        cx.notify();
    }

    /// Get the current mapping of service_name -> terminal_id for a project.
    /// Used to persist terminal IDs across restarts.
    /// Docker services are excluded (their log PTYs are ephemeral).
    pub fn service_terminal_ids(&self, project_id: &str) -> HashMap<String, String> {
        self.instances
            .iter()
            .filter(|((pid, _), inst)| pid == project_id && inst.kind == ServiceKind::Okena)
            .filter_map(|((_, name), instance)| {
                instance.terminal_id.as_ref().map(|tid| (name.clone(), tid.clone()))
            })
            .collect()
    }

    /// Stop all running services for a project and remove all instances/configs.
    pub fn unload_project_services(&mut self, project_id: &str, cx: &mut Context<Self>) {
        // Stop Docker status poller
        if let Some(cancel) = self.docker_pollers.remove(project_id) {
            cancel.store(true, Ordering::Relaxed);
        }

        let keys: Vec<(String, String)> = self
            .instances
            .keys()
            .filter(|(pid, _)| pid == project_id)
            .cloned()
            .collect();

        for key in keys {
            if let Some(instance) = self.instances.get(&key) {
                if let Some(terminal_id) = &instance.terminal_id {
                    self.backend.kill(terminal_id);
                    self.terminals.lock().remove(terminal_id);
                    self.terminal_to_service.remove(terminal_id);
                }
            }
            self.instances.remove(&key);
        }

        self.configs.remove(project_id);
        self.project_paths.remove(project_id);
        cx.notify();
    }

    /// Re-read `okena.yaml`. Stop removed services, add new ones,
    /// keep unchanged running services as-is. Also reloads Docker services.
    pub fn reload_project_services(
        &mut self,
        project_id: &str,
        project_path: &str,
        cx: &mut Context<Self>,
    ) {
        let new_config = match load_project_config(project_path) {
            Ok(Some(config)) => config,
            Ok(None) => {
                self.unload_project_services(project_id, cx);
                return;
            }
            Err(e) => {
                log::error!(
                    "Failed to reload okena.yaml for project {}: {}",
                    project_id,
                    e
                );
                return;
            }
        };

        self.project_paths
            .insert(project_id.to_string(), project_path.to_string());

        let new_names: std::collections::HashSet<String> =
            new_config.services.iter().map(|s| s.name.clone()).collect();

        // Stop and remove Okena services that no longer exist in config
        let removed_keys: Vec<(String, String)> = self
            .instances
            .keys()
            .filter(|(pid, name)| {
                pid == project_id
                    && !new_names.contains(name)
                    && self.instances.get(&(pid.clone(), name.clone()))
                        .is_some_and(|i| i.kind == ServiceKind::Okena)
            })
            .cloned()
            .collect();

        for key in removed_keys {
            if let Some(instance) = self.instances.get(&key) {
                if let Some(terminal_id) = &instance.terminal_id {
                    self.backend.kill(terminal_id);
                    self.terminals.lock().remove(terminal_id);
                    self.terminal_to_service.remove(terminal_id);
                }
            }
            self.instances.remove(&key);
        }

        // Add new services or update definitions for existing ones
        for def in &new_config.services {
            let key = (project_id.to_string(), def.name.clone());
            if let Some(instance) = self.instances.get_mut(&key) {
                instance.definition = def.clone();
            } else {
                self.instances.insert(
                    key,
                    ServiceInstance {
                        definition: def.clone(),
                        kind: ServiceKind::Okena,
                        status: ServiceStatus::Stopped,
                        terminal_id: None,
                        restart_count: 0,
                        detected_ports: Vec::new(),
                        is_extra: false,
                    },
                );
            }
        }

        self.configs
            .insert(project_id.to_string(), new_config.services.clone());

        // Reload Docker Compose services
        self.reload_docker_compose_services(project_id, project_path, new_config.docker_compose.as_ref(), cx);

        cx.notify();
    }

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

    /// Get all service instances for a project (Okena in config order, then Docker).
    pub fn services_for_project(&self, project_id: &str) -> Vec<&ServiceInstance> {
        let mut result: Vec<&ServiceInstance> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Okena services first, in config order
        if let Some(defs) = self.configs.get(project_id) {
            for def in defs {
                let key = (project_id.to_string(), def.name.clone());
                if let Some(inst) = self.instances.get(&key) {
                    seen.insert(def.name.clone());
                    result.push(inst);
                }
            }
        }

        // Docker services (sorted by name, non-extra before extra)
        let mut docker: Vec<&ServiceInstance> = self.instances.iter()
            .filter(|((pid, name), inst)| {
                pid == project_id
                    && matches!(inst.kind, ServiceKind::DockerCompose { .. })
                    && !seen.contains(name)
            })
            .map(|(_, inst)| inst)
            .collect();
        docker.sort_by(|a, b| {
            a.is_extra.cmp(&b.is_extra)
                .then_with(|| a.definition.name.cmp(&b.definition.name))
        });
        result.extend(docker);

        result
    }

    /// Access the instances map (for status inspection).
    pub fn instances(&self) -> &HashMap<(String, String), ServiceInstance> {
        &self.instances
    }

    /// Get the stored project path for a project.
    pub fn project_path(&self, project_id: &str) -> Option<&String> {
        self.project_paths.get(project_id)
    }

    /// Update the stored on-disk path for a project (e.g. after directory rename).
    /// Only updates existing entries — projects that haven't been loaded yet will
    /// pick up the new path when `load_project_services` is next called.
    pub fn update_project_path(&mut self, project_id: &str, new_path: &str) {
        if let Some(entry) = self.project_paths.get_mut(project_id) {
            *entry = new_path.to_string();
        }
    }

    /// Whether the project has any service definitions loaded (Okena or Docker).
    pub fn has_services(&self, project_id: &str) -> bool {
        self.configs
            .get(project_id)
            .is_some_and(|v| !v.is_empty())
            || self.instances.keys().any(|(pid, _)| pid == project_id)
    }

    /// Look up the terminal_id for a service.
    pub fn terminal_id_for(&self, project_id: &str, service_name: &str) -> Option<&String> {
        self.instances
            .get(&(project_id.to_string(), service_name.to_string()))
            .and_then(|i| i.terminal_id.as_ref())
    }

    /// Register a service for centralized port detection polling.
    /// The centralized poller calls `ss`/`lsof`/`netstat` once per cycle
    /// and distributes results to all registered services.
    fn start_port_detection(
        &mut self,
        project_id: &str,
        service_name: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (project_id.to_string(), service_name.to_string());
        if self.instances.get(&key).and_then(|i| i.terminal_id.as_ref()).is_none() {
            return;
        }
        self.port_detection_active.insert(
            key,
            PortDetectionState {
                polls_remaining: 10,
                found_any: false,
                stable_count: 0,
            },
        );
        self.ensure_port_detection_poller(cx);
    }

    /// Ensure the centralized port detection poller is running.
    /// One poller handles all services: builds the process tree once,
    /// calls the port scanner once, then distributes results.
    fn ensure_port_detection_poller(&mut self, cx: &mut Context<Self>) {
        if self.port_detection_running {
            return;
        }
        self.port_detection_running = true;
        let backend = self.backend.clone();

        cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
            // Initial delay — let newly started services bind their ports
            cx.background_executor().timer(Duration::from_secs(2)).await;

            loop {
                // Collect all services that need port detection + their terminal IDs
                let services: Vec<((String, String), String)> = this
                    .update(cx, |this, _| {
                        this.port_detection_active
                            .keys()
                            .filter_map(|key| {
                                let inst = this.instances.get(key)?;
                                if inst.status != ServiceStatus::Running {
                                    return None;
                                }
                                let tid = inst.terminal_id.clone()?;
                                Some((key.clone(), tid))
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                if services.is_empty() {
                    let _ = this.update(cx, |this, _| {
                        this.port_detection_active.clear();
                        this.port_detection_running = false;
                    });
                    return;
                }

                // Background: get PIDs per service, build process tree ONCE,
                // scan ports ONCE, distribute results.
                let backend_ref = backend.clone();
                let results: Vec<((String, String), Vec<u16>)> = cx
                    .background_executor()
                    .spawn(async move {
                        // Get root PIDs for all services in one batch.
                        // On Linux+dtach this reads /proc once instead of spawning lsof per terminal.
                        let terminal_ids: Vec<&str> =
                            services.iter().map(|(_, tid)| tid.as_str()).collect();
                        let batch_pids = backend_ref.get_batch_service_pids(&terminal_ids);
                        let service_root_pids: Vec<((String, String), Vec<u32>)> = services
                            .iter()
                            .map(|(key, tid)| {
                                let pids = batch_pids
                                    .get(tid.as_str())
                                    .cloned()
                                    .unwrap_or_default();
                                (key.clone(), pids)
                            })
                            .collect();

                        // Build process tree ONCE for all services
                        let tree = port_detect::build_process_tree();

                        // Expand to descendant PIDs per service
                        let mut all_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();
                        let service_pid_sets: Vec<(
                            (String, String),
                            std::collections::HashSet<u32>,
                        )> = service_root_pids
                            .into_iter()
                            .map(|(key, roots)| {
                                let mut pids = std::collections::HashSet::new();
                                for &pid in &roots {
                                    pids.extend(port_detect::descendants_from_tree(&tree, pid));
                                }
                                all_pids.extend(&pids);
                                (key, pids)
                            })
                            .collect();

                        if all_pids.is_empty() {
                            return service_pid_sets
                                .into_iter()
                                .map(|(k, _)| (k, Vec::new()))
                                .collect();
                        }

                        // ONE system call for port scanning
                        let all_port_pairs = port_detect::get_listening_port_pairs();

                        // Distribute ports to each service
                        service_pid_sets
                            .into_iter()
                            .map(|(key, pids)| {
                                let ports = port_detect::ports_for_pids(&all_port_pairs, &pids);
                                (key, ports)
                            })
                            .collect()
                    })
                    .await;

                // Update instances and port detection state
                let has_remaining = this
                    .update(cx, |this, cx| {
                        let mut changed = false;
                        let mut keys_to_remove = Vec::new();

                        for (key, ports) in results {
                            let Some(state) = this.port_detection_active.get_mut(&key) else {
                                continue;
                            };

                            state.polls_remaining = state.polls_remaining.saturating_sub(1);

                            if !ports.is_empty() {
                                let ports_changed =
                                    if let Some(inst) = this.instances.get_mut(&key) {
                                        if inst.status == ServiceStatus::Running
                                            && inst.detected_ports != ports
                                        {
                                            inst.detected_ports = ports;
                                            true
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    };

                                if state.found_any && !ports_changed {
                                    state.stable_count += 1;
                                    if state.stable_count >= 2 {
                                        keys_to_remove.push(key.clone());
                                        continue;
                                    }
                                } else {
                                    state.stable_count = 0;
                                }
                                state.found_any = true;
                                if ports_changed {
                                    changed = true;
                                }
                            }

                            if state.polls_remaining == 0 {
                                keys_to_remove.push(key.clone());
                            }
                        }

                        for key in keys_to_remove {
                            this.port_detection_active.remove(&key);
                        }

                        if changed {
                            cx.notify();
                        }

                        !this.port_detection_active.is_empty()
                    })
                    .unwrap_or(false);

                if !has_remaining {
                    let _ = this.update(cx, |this, _| {
                        this.port_detection_running = false;
                    });
                    return;
                }

                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        })
        .detach();
    }

    /// Load Docker Compose services for a project.
    /// If `docker_config` is None, auto-detects compose file.
    /// If `docker_config.enabled` is explicitly false, skips.
    fn load_docker_compose_services(
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
    fn reload_docker_compose_services(
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

/// Check if a process with the given PID is still alive.
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // signal 0 doesn't send a signal but checks if process exists
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On non-Unix, conservatively assume alive (caller will time out)
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_instance(
        project_id: &str,
        name: &str,
        restart_on_crash: bool,
        restart_count: u32,
        status: ServiceStatus,
    ) -> ((String, String), ServiceInstance) {
        let def = ServiceDefinition {
            name: name.to_string(),
            command: "echo test".to_string(),
            cwd: ".".to_string(),
            env: HashMap::new(),
            auto_start: false,
            restart_on_crash,
            restart_delay_ms: 1000,
        };
        (
            (project_id.to_string(), name.to_string()),
            ServiceInstance {
                definition: def,
                kind: ServiceKind::Okena,
                status,
                terminal_id: Some(format!("term-{}", name)),
                restart_count,
                detected_ports: Vec::new(),
                is_extra: false,
            },
        )
    }

    /// Simulates the exit-handling state transition logic from handle_service_exit.
    fn simulate_exit(instance: &mut ServiceInstance, exit_code: Option<u32>) {
        if instance.definition.restart_on_crash && instance.restart_count < MAX_RESTART_COUNT {
            // Auto-restart: clear terminal
            instance.terminal_id = None;
            instance.status = ServiceStatus::Restarting;
            instance.restart_count += 1;
        } else {
            // Crash without restart: keep terminal_id for viewing crash output
            instance.status = ServiceStatus::Crashed { exit_code };
        }
    }

    #[test]
    fn handle_exit_triggers_restart() {
        let (_key, mut instance) = make_instance("proj1", "svc1", true, 0, ServiceStatus::Running);
        simulate_exit(&mut instance, Some(1));
        assert_eq!(instance.status, ServiceStatus::Restarting);
        assert_eq!(instance.restart_count, 1);
    }

    #[test]
    fn handle_exit_caps_restarts() {
        let (_key, mut instance) = make_instance(
            "proj1",
            "svc1",
            true,
            MAX_RESTART_COUNT,
            ServiceStatus::Running,
        );
        simulate_exit(&mut instance, Some(1));
        assert_eq!(
            instance.status,
            ServiceStatus::Crashed {
                exit_code: Some(1)
            }
        );
        assert_eq!(instance.restart_count, MAX_RESTART_COUNT);
    }

    #[test]
    fn handle_exit_no_restart() {
        let (_key, mut instance) =
            make_instance("proj1", "svc1", false, 0, ServiceStatus::Running);
        simulate_exit(&mut instance, None);
        assert_eq!(
            instance.status,
            ServiceStatus::Crashed { exit_code: None }
        );
        assert_eq!(instance.restart_count, 0);
        // Terminal should be preserved for viewing crash output
        assert!(instance.terminal_id.is_some());
    }

    #[test]
    fn handle_exit_restart_clears_terminal() {
        let (_key, mut instance) = make_instance("proj1", "svc1", true, 0, ServiceStatus::Running);
        simulate_exit(&mut instance, Some(1));
        assert_eq!(instance.status, ServiceStatus::Restarting);
        // Terminal should be cleared for auto-restart
        assert!(instance.terminal_id.is_none());
    }

    #[test]
    fn unload_removes_instances() {
        let mut instances: HashMap<(String, String), ServiceInstance> = HashMap::new();
        let mut configs: HashMap<String, Vec<ServiceDefinition>> = HashMap::new();

        let (key1, inst1) = make_instance("proj1", "svc1", false, 0, ServiceStatus::Stopped);
        let (key2, inst2) = make_instance("proj1", "svc2", false, 0, ServiceStatus::Stopped);
        let (key3, inst3) = make_instance("proj2", "svc1", false, 0, ServiceStatus::Stopped);

        instances.insert(key1, inst1);
        instances.insert(key2, inst2);
        instances.insert(key3, inst3);
        configs.insert("proj1".to_string(), vec![]);
        configs.insert("proj2".to_string(), vec![]);

        // Simulate unload for proj1
        let keys: Vec<(String, String)> = instances
            .keys()
            .filter(|(pid, _)| pid == "proj1")
            .cloned()
            .collect();
        for key in keys {
            instances.remove(&key);
        }
        configs.remove("proj1");

        assert_eq!(instances.len(), 1);
        assert!(instances.contains_key(&("proj2".to_string(), "svc1".to_string())));
        assert!(!configs.contains_key("proj1"));
        assert!(configs.contains_key("proj2"));
    }

    #[test]
    fn service_terminal_ids_returns_running_services() {
        let mut instances: HashMap<(String, String), ServiceInstance> = HashMap::new();

        let (key1, inst1) = make_instance("proj1", "web", false, 0, ServiceStatus::Running);
        let (key2, mut inst2) = make_instance("proj1", "api", false, 0, ServiceStatus::Stopped);
        inst2.terminal_id = None; // Stopped service has no terminal
        let (key3, inst3) = make_instance("proj2", "db", false, 0, ServiceStatus::Running);

        instances.insert(key1, inst1);
        instances.insert(key2, inst2);
        instances.insert(key3, inst3);

        // Simulate service_terminal_ids for proj1
        let ids: HashMap<String, String> = instances
            .iter()
            .filter(|((pid, _), _)| pid == "proj1")
            .filter_map(|((_, name), instance)| {
                instance.terminal_id.as_ref().map(|tid| (name.clone(), tid.clone()))
            })
            .collect();

        assert_eq!(ids.len(), 1);
        assert_eq!(ids.get("web"), Some(&"term-web".to_string()));
        assert!(!ids.contains_key("api")); // No terminal_id
    }

    #[test]
    fn from_api_maps_known_statuses() {
        assert_eq!(ServiceStatus::from_api("running", None), ServiceStatus::Running);
        assert_eq!(ServiceStatus::from_api("starting", None), ServiceStatus::Starting);
        assert_eq!(ServiceStatus::from_api("restarting", None), ServiceStatus::Restarting);
        assert_eq!(ServiceStatus::from_api("crashed", None), ServiceStatus::Crashed { exit_code: None });
        assert_eq!(ServiceStatus::from_api("crashed", Some(1)), ServiceStatus::Crashed { exit_code: Some(1) });
        assert_eq!(ServiceStatus::from_api("stopped", None), ServiceStatus::Stopped);
        assert_eq!(ServiceStatus::from_api("unknown", None), ServiceStatus::Stopped);
        assert_eq!(ServiceStatus::from_api("", None), ServiceStatus::Stopped);
    }

    fn make_docker_instance(
        project_id: &str,
        name: &str,
        status: ServiceStatus,
    ) -> ((String, String), ServiceInstance) {
        let def = ServiceDefinition {
            name: name.to_string(),
            command: String::new(),
            cwd: ".".to_string(),
            env: HashMap::new(),
            auto_start: false,
            restart_on_crash: false,
            restart_delay_ms: 0,
        };
        (
            (project_id.to_string(), name.to_string()),
            ServiceInstance {
                definition: def,
                kind: ServiceKind::DockerCompose { compose_file: "docker-compose.yml".to_string() },
                status,
                terminal_id: Some(format!("term-{}", name)),
                restart_count: 0,
                detected_ports: Vec::new(),
                is_extra: false,
            },
        )
    }

    #[test]
    fn handle_exit_docker_log_viewer() {
        // Docker log PTY exit should clear terminal_id but not change status
        let (_key, mut instance) = make_docker_instance("proj1", "web", ServiceStatus::Running);
        assert!(instance.terminal_id.is_some());

        // Simulate Docker exit handling: just clear terminal
        if matches!(instance.kind, ServiceKind::DockerCompose { .. }) {
            instance.terminal_id = None;
            // status should remain unchanged
        }

        assert_eq!(instance.status, ServiceStatus::Running);
        assert!(instance.terminal_id.is_none());
    }

    #[test]
    fn docker_service_terminal_ids_excluded() {
        let mut instances: HashMap<(String, String), ServiceInstance> = HashMap::new();

        let (key1, inst1) = make_instance("proj1", "web", false, 0, ServiceStatus::Running);
        let (key2, inst2) = make_docker_instance("proj1", "db", ServiceStatus::Running);

        instances.insert(key1, inst1);
        instances.insert(key2, inst2);

        // Simulate service_terminal_ids with Docker filtering
        let ids: HashMap<String, String> = instances
            .iter()
            .filter(|((pid, _), inst)| pid == "proj1" && inst.kind == ServiceKind::Okena)
            .filter_map(|((_, name), instance)| {
                instance.terminal_id.as_ref().map(|tid| (name.clone(), tid.clone()))
            })
            .collect();

        assert_eq!(ids.len(), 1);
        assert!(ids.contains_key("web"));
        assert!(!ids.contains_key("db")); // Docker service excluded
    }
}
