use crate::services::config::{load_project_config, ServiceDefinition};
use crate::services::docker_compose;
use crate::services::port_detect;
use crate::terminal::backend::TerminalBackend;
use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::views::root::TerminalsRegistry;
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
        let config = match load_project_config(project_path) {
            Ok(Some(config)) => config,
            Ok(None) => {
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

        let shell = if cfg!(windows) {
            ShellType::Custom {
                path: "cmd".to_string(),
                args: vec!["/C".to_string(), command],
            }
        } else {
            ShellType::Custom {
                path: "sh".to_string(),
                args: vec!["-c".to_string(), command],
            }
        };

        match self.backend.reconnect_terminal(saved_terminal_id, &cwd, Some(&shell)) {
            Ok(terminal_id) => {
                let terminal = Arc::new(Terminal::new(
                    terminal_id.clone(),
                    TerminalSize::default(),
                    self.backend.transport(),
                    cwd,
                ));
                self.terminals.lock().insert(terminal_id.clone(), terminal);

                let instance = self.instances.get_mut(&key).unwrap();
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
                            let mut cmd = crate::process::command("docker");
                            cmd.args(["compose", "-f", &compose_file, "start", &name])
                                .current_dir(&path);
                            crate::process::safe_output(&mut cmd)
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

        let shell = if cfg!(windows) {
            ShellType::Custom {
                path: "cmd".to_string(),
                args: vec!["/C".to_string(), command],
            }
        } else {
            ShellType::Custom {
                path: "sh".to_string(),
                args: vec!["-c".to_string(), command],
            }
        };

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

                let instance = self.instances.get_mut(&key).unwrap();
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
                let instance = self.instances.get_mut(&key).unwrap();
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
                            let mut cmd = crate::process::command("docker");
                            cmd.args(["compose", "-f", &compose_file, "stop", &name])
                                .current_dir(&path);
                            let _ = crate::process::safe_output(&mut cmd);
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
                            let mut cmd = crate::process::command("docker");
                            cmd.args(["compose", "-f", &compose_file, "restart", &name])
                                .current_dir(&path);
                            let _ = crate::process::safe_output(&mut cmd);
                        })
                        .await;
                    let _ = this.update(cx, |_this, cx| cx.notify());
                }).detach();
            }
            ServiceKind::Okena => {
                // Collect all descendant PIDs before killing so we can wait for them.
                let old_pids: Vec<u32> = instance
                    .terminal_id
                    .as_ref()
                    .and_then(|tid| self.backend.get_shell_pid(tid))
                    .map(|pid| port_detect::get_descendant_pids(pid).into_iter().collect())
                    .unwrap_or_default();

                // Kill old terminal
                if let Some(terminal_id) = instance.terminal_id.take() {
                    self.backend.kill(&terminal_id);
                    self.terminals.lock().remove(&terminal_id);
                    self.terminal_to_service.remove(&terminal_id);
                }

                instance.status = ServiceStatus::Restarting;
                instance.restart_count = 0;
                instance.detected_ports.clear();
                cx.notify();

                // Wait for old processes to die, then start the new one
                let pid = project_id.to_string();
                let name = service_name.to_string();
                let path = project_path.to_string();

                cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
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

    /// Start background port detection polling for a running service.
    /// Waits 2s initial delay, then polls every 3s up to 10 times.
    /// Keeps polling even after finding ports, since services like Vite
    /// bind their real port later than internal/debug ports.
    fn start_port_detection(
        &self,
        project_id: &str,
        service_name: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (project_id.to_string(), service_name.to_string());
        if self.instances.get(&key).and_then(|i| i.terminal_id.as_ref()).is_none() {
            return;
        }
        let backend = self.backend.clone();

        cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
            // Initial delay — let the service bind its port
            cx.background_executor().timer(Duration::from_secs(2)).await;

            let mut found_any = false;
            // After first ports found, do 2 more polls to catch late-binding ports
            let mut stable_count = 0u32;

            for _ in 0..10 {
                // Check if service is still running (quick, main thread only)
                let terminal_id = this.update(cx, |this, _cx| {
                    let inst = this.instances.get(&key)?;
                    if inst.status != ServiceStatus::Running {
                        return None;
                    }
                    inst.terminal_id.clone()
                }).ok().flatten();

                let Some(terminal_id) = terminal_id else { return };

                // Run PID lookup + port scanning on background thread
                // (get_service_pids may spawn lsof/tmux subprocesses)
                let backend_ref = backend.clone();
                let ports = cx.background_executor()
                    .spawn(async move {
                        let service_pids = backend_ref.get_service_pids(&terminal_id);
                        if service_pids.is_empty() {
                            return Vec::new();
                        }
                        port_detect::detect_ports_for_pids(&service_pids)
                    })
                    .await;

                if !ports.is_empty() {
                    let changed = this.update(cx, |this, cx| {
                        if let Some(inst) = this.instances.get_mut(&key) {
                            if inst.status == ServiceStatus::Running && inst.detected_ports != ports {
                                inst.detected_ports = ports;
                                cx.notify();
                                return true;
                            }
                        }
                        false
                    }).unwrap_or(false);

                    if found_any && !changed {
                        stable_count += 1;
                        if stable_count >= 2 {
                            return; // Ports stable for 2 consecutive polls, done
                        }
                    } else {
                        stable_count = 0;
                    }
                    found_any = true;
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
        docker_config: Option<&crate::services::config::DockerComposeConfig>,
        cx: &mut Context<Self>,
    ) {
        // Check if explicitly disabled
        if docker_config.as_ref().is_some_and(|dc| dc.enabled == Some(false)) {
            return;
        }

        if !docker_compose::is_docker_compose_available() {
            return;
        }

        // Resolve compose file
        let compose_file = docker_config
            .and_then(|dc| dc.file.clone())
            .or_else(|| docker_compose::detect_compose_file(project_path));

        let Some(compose_file) = compose_file else { return };

        // Get service names
        let service_names = match docker_compose::list_services(project_path, &compose_file) {
            Ok(names) => names,
            Err(e) => {
                log::warn!("Failed to list Docker Compose services for project {}: {}", project_id, e);
                return;
            }
        };

        // Determine which services are explicitly listed in the okena.yaml filter
        let filter: Option<&Vec<String>> = docker_config
            .map(|dc| &dc.services)
            .filter(|s| !s.is_empty());

        for name in &service_names {
            let is_extra = filter.is_some_and(|f| !f.contains(name));

            let key = (project_id.to_string(), name.clone());
            if !self.instances.contains_key(&key) {
                self.instances.insert(
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
        self.start_docker_status_poller(project_id, project_path, &compose_file, cx);
    }

    /// Reload Docker Compose services on config reload.
    fn reload_docker_compose_services(
        &mut self,
        project_id: &str,
        project_path: &str,
        docker_config: Option<&crate::services::config::DockerComposeConfig>,
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

        let shell = if cfg!(windows) {
            ShellType::Custom {
                path: "cmd".to_string(),
                args: vec!["/C".to_string(), command],
            }
        } else {
            ShellType::Custom {
                path: "sh".to_string(),
                args: vec!["-c".to_string(), command],
            }
        };

        match self.backend.create_terminal(&project_path, Some(&shell)) {
            Ok(terminal_id) => {
                let terminal = Arc::new(Terminal::new(
                    terminal_id.clone(),
                    TerminalSize::default(),
                    self.backend.transport(),
                    project_path,
                ));
                self.terminals.lock().insert(terminal_id.clone(), terminal);

                let instance = self.instances.get_mut(&key).unwrap();
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

            loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }

                let path_clone = path.clone();
                let file_clone = file.clone();
                let result = cx.background_executor()
                    .spawn(async move {
                        docker_compose::poll_status(&path_clone, &file_clone)
                    })
                    .await;

                if cancel.load(Ordering::Relaxed) {
                    return;
                }

                match result {
                    Ok(statuses) => {
                        let should_stop = this.update(cx, |this, cx| {
                            let mut any_docker = false;
                            for ds in &statuses {
                                let key = (pid.clone(), ds.name.clone());
                                if let Some(inst) = this.instances.get_mut(&key) {
                                    if matches!(inst.kind, ServiceKind::DockerCompose { .. }) {
                                        any_docker = true;
                                        let new_status = docker_compose::map_docker_state(&ds.state, ds.exit_code);
                                        if inst.status != new_status {
                                            inst.status = new_status;
                                        }
                                        if inst.detected_ports != ds.ports {
                                            inst.detected_ports = ds.ports.clone();
                                        }
                                    }
                                }
                            }
                            cx.notify();
                            !any_docker
                        }).unwrap_or(true);

                        if should_stop {
                            return;
                        }
                    }
                    Err(e) => {
                        log::warn!("Docker status poll failed for project {}: {}", pid, e);
                    }
                }

                cx.background_executor().timer(Duration::from_secs(5)).await;
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
