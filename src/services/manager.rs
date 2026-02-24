use crate::services::config::{load_project_config, ServiceDefinition};
use crate::services::port_detect;
use crate::terminal::backend::TerminalBackend;
use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::views::root::TerminalsRegistry;
use gpui::{Context, WeakEntity};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub struct ServiceManager {
    configs: HashMap<String, Vec<ServiceDefinition>>,
    instances: HashMap<(String, String), ServiceInstance>,
    terminal_to_service: HashMap<String, (String, String)>,
    project_paths: HashMap<String, String>,
    backend: Arc<dyn TerminalBackend>,
    terminals: TerminalsRegistry,
}

pub struct ServiceInstance {
    pub definition: ServiceDefinition,
    pub project_id: String,
    pub status: ServiceStatus,
    pub terminal_id: Option<String>,
    pub restart_count: u32,
    pub detected_ports: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServiceStatus {
    Stopped,
    Starting,
    Running,
    Crashed { exit_code: Option<u32> },
    Restarting,
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
        }
    }

    /// Parse `okena.yaml` for a project, create `ServiceInstance` entries,
    /// reconnect to saved sessions, and auto-start services where configured.
    pub fn load_project_services(
        &mut self,
        project_id: &str,
        project_path: &str,
        saved_terminal_ids: &HashMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        let config = match load_project_config(project_path) {
            Ok(Some(config)) => config,
            Ok(None) => return,
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
                    project_id: project_id.to_string(),
                    status: ServiceStatus::Stopped,
                    terminal_id: None,
                    restart_count: 0,
                    detected_ports: Vec::new(),
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
    pub fn service_terminal_ids(&self, project_id: &str) -> HashMap<String, String> {
        self.instances
            .iter()
            .filter(|((pid, _), _)| pid == project_id)
            .filter_map(|((_, name), instance)| {
                instance.terminal_id.as_ref().map(|tid| (name.clone(), tid.clone()))
            })
            .collect()
    }

    /// Stop all running services for a project and remove all instances/configs.
    pub fn unload_project_services(&mut self, project_id: &str, cx: &mut Context<Self>) {
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
    /// keep unchanged running services as-is.
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

        // Stop and remove services that no longer exist in config
        let removed_keys: Vec<(String, String)> = self
            .instances
            .keys()
            .filter(|(pid, name)| pid == project_id && !new_names.contains(name))
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

        // Add new services (ones not already present)
        for def in &new_config.services {
            let key = (project_id.to_string(), def.name.clone());
            if !self.instances.contains_key(&key) {
                self.instances.insert(
                    key,
                    ServiceInstance {
                        definition: def.clone(),
                        project_id: project_id.to_string(),
                        status: ServiceStatus::Stopped,
                        terminal_id: None,
                        restart_count: 0,
                        detected_ports: Vec::new(),
                    },
                );
            }
        }

        self.configs
            .insert(project_id.to_string(), new_config.services);
        cx.notify();
    }

    /// Start a service by spawning a PTY with the service command.
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

        if let Some(terminal_id) = instance.terminal_id.take() {
            self.backend.kill(&terminal_id);
            self.terminals.lock().remove(&terminal_id);
            self.terminal_to_service.remove(&terminal_id);
        }

        instance.status = ServiceStatus::Stopped;
        instance.restart_count = 0;
        instance.detected_ports.clear();
        cx.notify();
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

        // Get PID before killing so we can wait for it
        let old_pid = instance.terminal_id.as_ref()
            .and_then(|tid| self.backend.get_shell_pid(tid));

        // Kill old terminal
        if let Some(terminal_id) = instance.terminal_id.take() {
            self.backend.kill(&terminal_id);
            self.terminals.lock().remove(&terminal_id);
            self.terminal_to_service.remove(&terminal_id);
        }

        instance.status = ServiceStatus::Restarting;
        instance.restart_count = 0;
        cx.notify();

        // Wait for old process to die, then start the new one
        let pid = project_id.to_string();
        let name = service_name.to_string();
        let path = project_path.to_string();

        cx.spawn(async move |this: WeakEntity<ServiceManager>, cx| {
            // Poll until old process exits (max ~5s)
            if let Some(old_pid) = old_pid {
                for _ in 0..100 {
                    if !is_process_alive(old_pid) {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_millis(50)).await;
                }
            }

            let _ = this.update(cx, |this, cx| {
                // Only start if still in Restarting state (user might have stopped it meanwhile)
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
    /// handle crash/restart logic. Returns early for non-service terminals.
    pub fn handle_service_exit(
        &mut self,
        terminal_id: &str,
        exit_code: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        let key = match self.terminal_to_service.remove(terminal_id) {
            Some(key) => key,
            None => return, // Not a service terminal
        };

        let (project_id, service_name) = key.clone();

        let instance = match self.instances.get_mut(&key) {
            Some(i) => i,
            None => return,
        };

        instance.terminal_id = None;
        instance.detected_ports.clear();

        if instance.definition.restart_on_crash && instance.restart_count < MAX_RESTART_COUNT {
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
            instance.status = ServiceStatus::Crashed { exit_code };
        }

        cx.notify();
    }

    /// Get all service instances for a project (in config order).
    pub fn services_for_project(&self, project_id: &str) -> Vec<&ServiceInstance> {
        if let Some(defs) = self.configs.get(project_id) {
            defs.iter()
                .filter_map(|def| {
                    self.instances
                        .get(&(project_id.to_string(), def.name.clone()))
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Access the instances map (for status inspection).
    pub fn instances(&self) -> &HashMap<(String, String), ServiceInstance> {
        &self.instances
    }

    /// Get the stored project path for a project.
    pub fn project_path(&self, project_id: &str) -> Option<&String> {
        self.project_paths.get(project_id)
    }

    /// Count of services currently in Running status.
    pub fn total_running_count(&self) -> usize {
        self.instances
            .values()
            .filter(|i| i.status == ServiceStatus::Running)
            .count()
    }

    /// Whether the project has any service definitions loaded.
    pub fn has_services(&self, project_id: &str) -> bool {
        self.configs
            .get(project_id)
            .is_some_and(|v| !v.is_empty())
    }

    /// Look up the terminal_id for a service.
    pub fn terminal_id_for(&self, project_id: &str, service_name: &str) -> Option<&String> {
        self.instances
            .get(&(project_id.to_string(), service_name.to_string()))
            .and_then(|i| i.terminal_id.as_ref())
    }

    /// Get detected listening ports for a service.
    pub fn detected_ports(&self, project_id: &str, service_name: &str) -> &[u16] {
        self.instances
            .get(&(project_id.to_string(), service_name.to_string()))
            .map(|i| i.detected_ports.as_slice())
            .unwrap_or(&[])
    }

    /// Start background port detection polling for a running service.
    /// Waits 2s initial delay, then polls every 2s up to 5 times.
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

            for _ in 0..5 {
                // Check if service is still running
                let pid = this.update(cx, |this, _cx| {
                    let inst = this.instances.get(&key)?;
                    if inst.status != ServiceStatus::Running {
                        return None;
                    }
                    inst.terminal_id.as_ref()
                        .and_then(|tid| backend.get_shell_pid(tid))
                }).ok().flatten();

                let Some(pid) = pid else { return };

                let ports = cx.background_executor()
                    .spawn(async move { port_detect::detect_ports_for_pid(pid) })
                    .await;

                if !ports.is_empty() {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(inst) = this.instances.get_mut(&key) {
                            if inst.status == ServiceStatus::Running {
                                inst.detected_ports = ports;
                                cx.notify();
                            }
                        }
                    });
                    return;
                }

                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        })
        .detach();
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
                project_id: project_id.to_string(),
                status,
                terminal_id: Some(format!("term-{}", name)),
                restart_count,
                detected_ports: Vec::new(),
            },
        )
    }

    /// Simulates the exit-handling state transition logic from handle_service_exit.
    fn simulate_exit(instance: &mut ServiceInstance, exit_code: Option<u32>) {
        instance.terminal_id = None;
        if instance.definition.restart_on_crash && instance.restart_count < MAX_RESTART_COUNT {
            instance.status = ServiceStatus::Restarting;
            instance.restart_count += 1;
        } else {
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
}
