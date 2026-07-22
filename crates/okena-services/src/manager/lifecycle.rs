//! Project service set lifecycle: load, unload, reload `okena.yaml`.

use super::{
    ServiceCx, ServiceInstance, ServiceKind, ServiceLoadStatus, ServiceManager, ServiceStatus,
    commands::OkenaLaunchFailure,
};
use crate::config::{PreparedProjectConfig, load_project_config, prepare_project_config};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

impl ServiceManager {
    /// Parse `okena.yaml` for a project, create `ServiceInstance` entries,
    /// reconnect to saved sessions, and auto-start services where configured.
    /// Also loads Docker Compose services if detected.
    pub fn load_project_services(
        &mut self,
        project_id: &str,
        project_path: &str,
        saved_terminal_ids: &HashMap<String, String>,
        cx: &mut impl ServiceCx,
    ) -> ServiceLoadStatus {
        self.load_project_services_with_auto_start(
            project_id,
            project_path,
            saved_terminal_ids,
            true,
            cx,
        )
    }

    /// Reload definitions for a backend migration, preserving explicit stopped state.
    pub fn load_project_services_for_backend_migration(
        &mut self,
        project_id: &str,
        project_path: &str,
        cx: &mut impl ServiceCx,
    ) -> ServiceLoadStatus {
        self.load_project_services_with_auto_start(
            project_id,
            project_path,
            &HashMap::new(),
            false,
            cx,
        )
    }

    fn load_project_services_with_auto_start(
        &mut self,
        project_id: &str,
        project_path: &str,
        saved_terminal_ids: &HashMap<String, String>,
        start_auto_services: bool,
        cx: &mut impl ServiceCx,
    ) -> ServiceLoadStatus {
        let prepared = match load_project_config(project_path) {
            Ok(config) => {
                let detected_compose_file =
                    crate::docker_compose::detect_compose_file(project_path);
                PreparedProjectConfig::Loaded {
                    config,
                    detected_compose_file,
                }
            }
            Err(error) => PreparedProjectConfig::Failed(error.to_string()),
        };
        self.load_project_services_prepared_with_auto_start(
            project_id,
            project_path,
            saved_terminal_ids,
            prepared,
            start_auto_services,
            cx,
        )
    }

    /// Apply a config previously read away from the owning reactor.
    pub fn load_project_services_prepared(
        &mut self,
        project_id: &str,
        project_path: &str,
        saved_terminal_ids: &HashMap<String, String>,
        prepared: PreparedProjectConfig,
        cx: &mut impl ServiceCx,
    ) -> ServiceLoadStatus {
        self.load_project_services_prepared_with_auto_start(
            project_id,
            project_path,
            saved_terminal_ids,
            prepared,
            true,
            cx,
        )
    }

    /// Apply a prepared config without starting services from config defaults.
    pub fn load_project_services_prepared_without_auto_start(
        &mut self,
        project_id: &str,
        project_path: &str,
        prepared: PreparedProjectConfig,
        cx: &mut impl ServiceCx,
    ) -> ServiceLoadStatus {
        self.load_project_services_prepared_with_auto_start(
            project_id,
            project_path,
            &HashMap::new(),
            prepared,
            false,
            cx,
        )
    }

    fn load_project_services_prepared_with_auto_start(
        &mut self,
        project_id: &str,
        project_path: &str,
        saved_terminal_ids: &HashMap<String, String>,
        prepared: PreparedProjectConfig,
        start_auto_services: bool,
        cx: &mut impl ServiceCx,
    ) -> ServiceLoadStatus {
        log::info!(
            "[services] load_project_services project_id={} path={}",
            project_id,
            project_path
        );
        let config = match prepared {
            PreparedProjectConfig::Loaded {
                config: Some(config),
                detected_compose_file,
            } => {
                log::info!(
                    "[services] Found okena.yaml with {} services",
                    config.services.len()
                );
                (config, detected_compose_file)
            }
            PreparedProjectConfig::Loaded {
                config: None,
                detected_compose_file,
            } => {
                log::info!("[services] No okena.yaml found at {}", project_path);
                // No okena.yaml — still try Docker Compose auto-detection
                self.begin_project_incarnation(project_id, project_path);
                self.project_paths
                    .insert(project_id.to_string(), project_path.to_string());
                self.load_docker_compose_services_prepared(
                    project_id,
                    project_path,
                    None,
                    detected_compose_file,
                    cx,
                );
                cx.notify();
                return ServiceLoadStatus::Loaded;
            }
            PreparedProjectConfig::Missing => {
                log::warn!("Project path disappeared before service apply: {project_path}");
                return ServiceLoadStatus::Failed;
            }
            PreparedProjectConfig::Failed(error) => {
                log::error!(
                    "Failed to load okena.yaml for project {}: {}",
                    project_id,
                    error
                );
                cx.notify();
                return ServiceLoadStatus::Failed;
            }
        };

        self.begin_project_incarnation(project_id, project_path);
        self.project_paths
            .insert(project_id.to_string(), project_path.to_string());

        let (config, detected_compose_file) = config;
        let auto_start_names: Vec<String> = if start_auto_services {
            config
                .services
                .iter()
                .filter(|s| s.auto_start)
                .map(|s| s.name.clone())
                .collect()
        } else {
            Vec::new()
        };

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

        self.configs.insert(project_id.to_string(), config.services);

        // Try to reconnect services that have saved terminal IDs
        for def in self.configs.get(project_id).cloned().unwrap_or_default() {
            if let Some(saved_id) = saved_terminal_ids.get(&def.name) {
                self.reconnect_service(project_id, &def.name, project_path, saved_id, cx);
            }
        }

        // Auto-start services that weren't reconnected
        for name in auto_start_names {
            let key = (project_id.to_string(), name.clone());
            if let Some(instance) = self.instances.get(&key)
                && instance.status == ServiceStatus::Stopped
            {
                self.start_service(project_id, &name, project_path, cx);
            }
        }

        // Load Docker Compose services
        self.load_docker_compose_services_prepared(
            project_id,
            project_path,
            config.docker_compose.as_ref(),
            detected_compose_file,
            cx,
        );

        cx.notify();
        ServiceLoadStatus::Loaded
    }

    /// Try to reconnect a service to an existing session backend session.
    fn reconnect_service(
        &mut self,
        project_id: &str,
        service_name: &str,
        project_path: &str,
        saved_terminal_id: &str,
        cx: &mut impl ServiceCx,
    ) {
        let key = (project_id.to_string(), service_name.to_string());
        let instance = match self.instances.get_mut(&key) {
            Some(i) => i,
            None => return,
        };

        let auto_start = instance.definition.auto_start;
        self.begin_okena_terminal_launch(
            project_id,
            service_name,
            project_path,
            saved_terminal_id.to_string(),
            OkenaLaunchFailure::Reconnect { auto_start },
            cx,
        );
    }

    /// Stop all running services for a project and remove all instances/configs.
    pub fn unload_project_services(&mut self, project_id: &str, cx: &mut impl ServiceCx) {
        self.unload_project_services_inner(project_id, &HashSet::new(), true, cx);
    }

    /// Invalidate service lifecycles without killing PTYs on the reactor thread.
    pub fn unload_project_services_for_backend_migration(
        &mut self,
        project_id: &str,
        cx: &mut impl ServiceCx,
    ) -> Vec<String> {
        self.unload_project_services_inner(project_id, &HashSet::new(), false, cx)
    }

    /// Unload manager state while leaving selected persistent backend sessions alive.
    /// Used only while reconciling a replacement workspace snapshot that will
    /// immediately reconnect those same terminal IDs.
    pub fn unload_project_services_preserving(
        &mut self,
        project_id: &str,
        preserved_terminal_ids: &HashSet<String>,
        cx: &mut impl ServiceCx,
    ) {
        self.unload_project_services_inner(project_id, preserved_terminal_ids, true, cx);
    }

    /// Finish a replacement reconciliation by killing preserved sessions that
    /// the freshly loaded service set did not successfully reconnect.
    pub fn kill_unclaimed_preserved_sessions(
        &self,
        project_id: &str,
        preserved_terminal_ids: &HashSet<String>,
    ) {
        let claimed: HashSet<String> = self
            .service_terminal_ids(project_id)
            .into_values()
            .collect();
        for terminal_id in preserved_terminal_ids.difference(&claimed) {
            self.backend.kill(terminal_id);
        }
    }

    fn unload_project_services_inner(
        &mut self,
        project_id: &str,
        preserved_terminal_ids: &HashSet<String>,
        kill_unpreserved: bool,
        cx: &mut impl ServiceCx,
    ) -> Vec<String> {
        self.invalidate_project_incarnation(project_id);

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

        let mut terminal_ids = Vec::new();
        for key in keys {
            if let Some(instance) = self.instances.get(&key)
                && let Some(terminal_id) = &instance.terminal_id
            {
                terminal_ids.push(terminal_id.clone());
                if kill_unpreserved && !preserved_terminal_ids.contains(terminal_id) {
                    self.backend.kill(terminal_id);
                }
                self.terminals.lock().remove(terminal_id);
                self.terminal_to_service.remove(terminal_id);
            }
            self.invalidate_okena_launch(&key);
            self.instances.remove(&key);
        }

        self.configs.remove(project_id);
        self.project_paths.remove(project_id);
        self.project_writeback_owners.remove(project_id);
        self.port_detection_active
            .retain(|(pid, _), _| pid != project_id);
        cx.notify();
        terminal_ids
    }

    /// Re-read `okena.yaml`. Stop removed services, add new ones,
    /// keep unchanged running services as-is. Also reloads Docker services.
    pub fn reload_project_services(
        &mut self,
        project_id: &str,
        project_path: &str,
        cx: &mut impl ServiceCx,
    ) {
        let prepared = prepare_project_config(project_path);
        self.reload_project_services_prepared(project_id, project_path, prepared, cx);
    }

    /// Apply a reload snapshot prepared away from the owning reactor.
    pub fn reload_project_services_prepared(
        &mut self,
        project_id: &str,
        project_path: &str,
        prepared: PreparedProjectConfig,
        cx: &mut impl ServiceCx,
    ) -> ServiceLoadStatus {
        let (new_config, detected_compose_file) = match prepared {
            PreparedProjectConfig::Loaded {
                config: Some(config),
                detected_compose_file,
            } => (config, detected_compose_file),
            PreparedProjectConfig::Loaded {
                config: None,
                detected_compose_file,
            } => {
                self.unload_project_services(project_id, cx);
                return self.load_project_services_prepared(
                    project_id,
                    project_path,
                    &HashMap::new(),
                    PreparedProjectConfig::Loaded {
                        config: None,
                        detected_compose_file,
                    },
                    cx,
                );
            }
            PreparedProjectConfig::Missing => {
                log::warn!("Project path disappeared before service reload: {project_path}");
                return ServiceLoadStatus::Failed;
            }
            PreparedProjectConfig::Failed(error) => {
                log::error!(
                    "Failed to reload okena.yaml for project {}: {}",
                    project_id,
                    error
                );
                return ServiceLoadStatus::Failed;
            }
        };

        self.begin_project_incarnation(project_id, project_path);

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
                    && self
                        .instances
                        .get(&(pid.clone(), name.clone()))
                        .is_some_and(|i| i.kind == ServiceKind::Okena)
            })
            .cloned()
            .collect();

        for key in removed_keys {
            if let Some(instance) = self.instances.get(&key)
                && let Some(terminal_id) = &instance.terminal_id
            {
                self.backend.kill(terminal_id);
                self.terminals.lock().remove(terminal_id);
                self.terminal_to_service.remove(terminal_id);
            }
            self.invalidate_okena_launch(&key);
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

        // Re-arm project-scoped work; pending launches carry their own reload-safe token.
        let runtime_to_rearm: Vec<(String, ServiceStatus, u64)> = self
            .instances
            .iter()
            .filter(|((pid, _), instance)| pid == project_id && instance.kind == ServiceKind::Okena)
            .map(|((_, name), instance)| {
                (
                    name.clone(),
                    instance.status.clone(),
                    instance.definition.restart_delay_ms,
                )
            })
            .collect();
        for (service_name, status, restart_delay_ms) in runtime_to_rearm {
            match status {
                ServiceStatus::Running => {
                    self.start_port_detection(project_id, &service_name, cx);
                }
                ServiceStatus::Restarting => {
                    self.schedule_okena_restart(
                        project_id,
                        &service_name,
                        project_path,
                        restart_delay_ms,
                        cx,
                    );
                }
                _ => {}
            }
        }

        // Reload Docker Compose services
        self.reload_docker_compose_services_prepared(
            project_id,
            project_path,
            new_config.docker_compose.as_ref(),
            detected_compose_file,
            cx,
        );

        cx.notify();
        ServiceLoadStatus::Loaded
    }
}
