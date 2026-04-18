//! Project service set lifecycle: load, unload, reload `okena.yaml`.

use super::{ServiceInstance, ServiceKind, ServiceManager, ServiceStatus};
use crate::config::load_project_config;
use gpui::Context;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::{Terminal, TerminalSize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

impl ServiceManager {
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
}
