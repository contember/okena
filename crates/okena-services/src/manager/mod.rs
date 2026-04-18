//! Service manager: owns Okena + Docker Compose service state per project.
//!
//! Split into submodules by concern:
//! - [`lifecycle`]      — load / unload / reload project service sets
//! - [`commands`]       — start / stop / restart individual services
//! - [`docker`]         — Docker Compose discovery, log viewers, status polling
//! - [`port_detection`] — centralized listening-port discovery poller

mod commands;
mod docker;
mod lifecycle;
mod port_detection;

use crate::config::ServiceDefinition;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::TerminalsRegistry;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub struct ServiceManager {
    pub(super) configs: HashMap<String, Vec<ServiceDefinition>>,
    pub(super) instances: HashMap<(String, String), ServiceInstance>,
    pub(super) terminal_to_service: HashMap<String, (String, String)>,
    pub(super) project_paths: HashMap<String, String>,
    pub(super) backend: Arc<dyn TerminalBackend>,
    pub(super) terminals: TerminalsRegistry,
    /// Cancel tokens for Docker status pollers (project_id -> cancel flag)
    pub(super) docker_pollers: HashMap<String, Arc<AtomicBool>>,
    /// Services currently undergoing port detection.
    pub(super) port_detection_active: HashMap<(String, String), PortDetectionState>,
    /// Whether the centralized port detection poller task is running.
    pub(super) port_detection_running: bool,
}

pub(super) struct PortDetectionState {
    pub(super) polls_remaining: u32,
    pub(super) found_any: bool,
    pub(super) stable_count: u32,
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

pub(super) const MAX_RESTART_COUNT: u32 = 5;

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
}

/// Check if a process with the given PID is still alive.
pub(super) fn is_process_alive(pid: u32) -> bool {
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
mod tests;
