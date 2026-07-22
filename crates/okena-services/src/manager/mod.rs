//! Service manager: owns Okena + Docker Compose service state per project.
//!
//! Split into submodules by concern:
//! - [`lifecycle`]      — load / unload / reload project service sets
//! - [`commands`]       — start / stop / restart individual services
//! - [`docker`]         — Docker Compose discovery, log viewers, status polling
//! - [`port_detection`] — centralized listening-port discovery poller

mod commands;
mod context;
mod docker;
mod lifecycle;
mod port_detection;

pub use context::{ServiceAsyncCx, ServiceCx, ServiceHandle};

use crate::config::ServiceDefinition;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub struct ServiceManager {
    pub(super) configs: HashMap<String, Vec<ServiceDefinition>>,
    pub(super) instances: HashMap<(String, String), ServiceInstance>,
    pub(super) terminal_to_service: HashMap<String, (String, String)>,
    pub(super) project_paths: HashMap<String, String>,
    /// Workspace replacement epoch owning each project's persisted terminal map.
    /// Daemon write-back uses this to reject notifications from an older snapshot.
    project_writeback_owners: HashMap<String, (String, u64)>,
    project_lifecycles: ProjectLifecycles,
    pending_okena_launches: HashMap<(String, String), OkenaLaunchToken>,
    next_okena_launch_generation: u64,
    pending_okena_restarts: HashMap<(String, String), OkenaRestartToken>,
    next_okena_restart_generation: u64,
    pub(super) backend: Arc<dyn TerminalBackend>,
    pub(super) terminals: TerminalsRegistry,
    /// Cancel tokens for Docker status pollers (project_id -> cancel flag)
    pub(super) docker_pollers: HashMap<String, Arc<AtomicBool>>,
    docker_mutations: DockerMutationQueue,
    docker_mutation_runner: Arc<dyn commands::DockerMutationRunner>,
    /// Services currently undergoing port detection.
    pub(super) port_detection_active: HashMap<(String, String), PortDetectionState>,
    /// Whether the centralized port detection poller task is running.
    pub(super) port_detection_running: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OkenaLaunchToken {
    generation: u64,
    project_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OkenaRestartToken {
    generation: u64,
    project_path: String,
    terminal_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProjectIncarnation {
    generation: u64,
    path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockerMutationKind {
    Start,
    Stop,
    Restart,
}

impl DockerMutationKind {
    fn compose_argument(self) -> &'static str {
        match self {
            Self::Start => "up",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct DockerMutationScope {
    project_path: String,
    compose_file: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DockerMutation {
    generation: u64,
    project_incarnation: Option<ProjectIncarnation>,
    project_id: String,
    project_path: String,
    compose_file: String,
    service_name: String,
    kind: DockerMutationKind,
}

impl DockerMutation {
    fn scope(&self) -> DockerMutationScope {
        DockerMutationScope {
            project_path: self.project_path.clone(),
            compose_file: self.compose_file.clone(),
        }
    }

    fn compose_identity(&self) -> ComposeProjectIdentity {
        ComposeProjectIdentity::new(&self.project_path, &self.compose_file)
    }
}

/// Filesystem identity of one Compose project configuration.
///
/// The identity resolves aliases for existing paths and keeps normalized
/// suffixes for paths that disappeared during an asynchronous operation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComposeProjectIdentity {
    project_path: PathBuf,
    compose_file: PathBuf,
}

impl ComposeProjectIdentity {
    pub fn new(project_path: impl AsRef<Path>, compose_file: impl AsRef<Path>) -> Self {
        let project_path = project_path.as_ref();
        let compose_file = compose_file.as_ref();
        let compose_file = if compose_file.is_absolute() {
            compose_file.to_path_buf()
        } else {
            project_path.join(compose_file)
        };
        Self {
            project_path: crate::docker_compose::physical_path(project_path),
            compose_file: crate::docker_compose::physical_path(&compose_file),
        }
    }

    pub fn project_path(&self) -> &Path {
        &self.project_path
    }

    pub fn compose_file(&self) -> &Path {
        &self.compose_file
    }
}

/// One active or queued external Compose mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposeMutationActivity {
    pub project_id: String,
    pub project_path: String,
    pub compose_file: String,
    pub service_name: String,
    pub kind: DockerMutationKind,
    pub queued: bool,
}

#[derive(Debug)]
struct ActiveDockerMutation {
    current: DockerMutation,
    pending: VecDeque<DockerMutation>,
}

#[derive(Default)]
struct DockerMutationQueue {
    active: HashMap<DockerMutationScope, ActiveDockerMutation>,
    next_generation: u64,
    /// Fences status probes that overlap a completed external mutation.
    completed_generation_by_project: HashMap<String, u64>,
}

impl DockerMutationQueue {
    fn enqueue(
        &mut self,
        key: (String, String),
        project_incarnation: Option<ProjectIncarnation>,
        project_path: String,
        compose_file: String,
        kind: DockerMutationKind,
    ) -> Option<DockerMutation> {
        let generation = self.next_generation.max(1);
        self.next_generation = generation
            .checked_add(1)
            .expect("Docker mutation generation exhausted");
        let mutation = DockerMutation {
            generation,
            project_incarnation,
            project_id: key.0,
            project_path,
            compose_file,
            service_name: key.1,
            kind,
        };
        let scope = mutation.scope();

        if let Some(active) = self.active.get_mut(&scope) {
            active.pending.push_back(mutation);
            return None;
        }

        self.active.insert(
            scope,
            ActiveDockerMutation {
                current: mutation.clone(),
                pending: VecDeque::new(),
            },
        );
        Some(mutation)
    }

    fn finish(&mut self, scope: &DockerMutationScope, generation: u64) -> Option<DockerMutation> {
        let active = self.active.get_mut(scope)?;
        if active.current.generation != generation {
            return None;
        }

        self.completed_generation_by_project
            .insert(active.current.project_id.clone(), generation);

        if let Some(next) = active.pending.pop_front() {
            active.current = next.clone();
            return Some(next);
        }

        self.active.remove(scope);
        None
    }

    fn completed_generation(&self, project_id: &str) -> u64 {
        self.completed_generation_by_project
            .get(project_id)
            .copied()
            .unwrap_or(0)
    }

    fn active_service_names<'a>(&'a self, project_id: &'a str) -> impl Iterator<Item = &'a str> {
        self.active
            .values()
            .flat_map(|active| std::iter::once(&active.current).chain(active.pending.iter()))
            .filter(move |mutation| mutation.project_id == project_id)
            .map(|mutation| mutation.service_name.as_str())
    }

    fn has_service_mutation(&self, project_id: &str, service_name: &str) -> bool {
        self.active_service_names(project_id)
            .any(|active_name| active_name == service_name)
    }
}

/// Opaque fence for preparing service state without holding the manager lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceProjectStateToken {
    incarnation: Option<ProjectIncarnation>,
    next_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceTerminalWriteback {
    pub project_id: String,
    pub project_path: String,
    pub data_replacement_epoch: u64,
    pub terminal_ids: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceLoadStatus {
    Loaded,
    Failed,
}

#[derive(Default)]
struct ProjectLifecycles {
    current: HashMap<String, ProjectIncarnation>,
    next_generation: u64,
}

impl ProjectLifecycles {
    fn begin(&mut self, project_id: &str, project_path: &str) -> ProjectIncarnation {
        let generation = self.next_generation.max(1);
        let incarnation = ProjectIncarnation {
            generation,
            path: project_path.to_string(),
        };
        self.next_generation = generation
            .checked_add(1)
            .expect("service project generation exhausted");
        self.current
            .insert(project_id.to_string(), incarnation.clone());
        incarnation
    }

    fn get(&self, project_id: &str, project_path: &str) -> Option<ProjectIncarnation> {
        self.current
            .get(project_id)
            .filter(|incarnation| incarnation.path == project_path)
            .cloned()
    }

    fn invalidate(&mut self, project_id: &str) {
        self.current.remove(project_id);
    }

    fn is_current(&self, project_id: &str, incarnation: &ProjectIncarnation) -> bool {
        self.current.get(project_id) == Some(incarnation)
    }
}

pub(super) struct PortDetectionState {
    pub(super) project_incarnation: ProjectIncarnation,
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

impl ServiceInstance {
    /// Project this runtime service instance onto its wire form
    /// ([`okena_core::api::ApiServiceInfo`]).
    ///
    /// Shared by both remote command loops (GUI `remote_command_loop` and the
    /// headless `daemon_command_loop`) so the `status` / `kind` string mapping
    /// lives in exactly one place. Keeping it here (rather than in the gpui-free
    /// `okena-app-core` shared builder) avoids adding an `okena-services`
    /// dependency to `okena-app-core`.
    pub fn to_api(&self) -> okena_core::api::ApiServiceInfo {
        let (status, exit_code) = match &self.status {
            ServiceStatus::Stopped => ("stopped", None),
            ServiceStatus::Starting => ("starting", None),
            ServiceStatus::Running => ("running", None),
            ServiceStatus::Crashed { exit_code } => ("crashed", *exit_code),
            ServiceStatus::Restarting => ("restarting", None),
        };
        let kind = match &self.kind {
            ServiceKind::Okena => "okena",
            ServiceKind::DockerCompose { .. } => "docker_compose",
        };
        okena_core::api::ApiServiceInfo {
            name: self.definition.name.clone(),
            status: status.to_string(),
            terminal_id: self.terminal_id.clone(),
            ports: self.detected_ports.clone(),
            exit_code,
            kind: kind.to_string(),
            is_extra: self.is_extra,
        }
    }
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
    /// Remote-action wrappers for the service commands, returning the wire
    /// [`CommandResult`] directly.
    ///
    /// Both remote command loops (GUI `remote_command_loop` and the headless
    /// `daemon_command_loop`) dispatch the service `ActionRequest`s with
    /// identical inner logic: look up the project path and, if present, run the
    /// command and reply `Ok`; otherwise reply `Err("project not found: …")`.
    /// The only difference between the loops is how they obtain `&mut self` +
    /// `cx` (entity `update` vs. `Mutex` lock + reactor cx). Centralizing the
    /// logic here (generic over any [`ServiceCx`]) keeps the two loops to pure
    /// cx-access glue.
    pub fn start_service_action(
        &mut self,
        project_id: &str,
        service_name: &str,
        cx: &mut impl ServiceCx,
    ) -> okena_core::api::CommandResult {
        match self.project_path(project_id).cloned() {
            Some(path) => {
                self.start_service(project_id, service_name, &path, cx);
                okena_core::api::CommandResult::Ok(None)
            }
            None => okena_core::api::CommandResult::Err(format!("project not found: {project_id}")),
        }
    }

    pub fn stop_service_action(
        &mut self,
        project_id: &str,
        service_name: &str,
        cx: &mut impl ServiceCx,
    ) -> okena_core::api::CommandResult {
        self.stop_service(project_id, service_name, cx);
        okena_core::api::CommandResult::Ok(None)
    }

    pub fn restart_service_action(
        &mut self,
        project_id: &str,
        service_name: &str,
        cx: &mut impl ServiceCx,
    ) -> okena_core::api::CommandResult {
        match self.project_path(project_id).cloned() {
            Some(path) => {
                self.restart_service(project_id, service_name, &path, cx);
                okena_core::api::CommandResult::Ok(None)
            }
            None => okena_core::api::CommandResult::Err(format!("project not found: {project_id}")),
        }
    }

    pub fn start_all_action(
        &mut self,
        project_id: &str,
        cx: &mut impl ServiceCx,
    ) -> okena_core::api::CommandResult {
        match self.project_path(project_id).cloned() {
            Some(path) => {
                self.start_all(project_id, &path, cx);
                okena_core::api::CommandResult::Ok(None)
            }
            None => okena_core::api::CommandResult::Err(format!("project not found: {project_id}")),
        }
    }

    pub fn stop_all_action(
        &mut self,
        project_id: &str,
        cx: &mut impl ServiceCx,
    ) -> okena_core::api::CommandResult {
        self.stop_all(project_id, cx);
        okena_core::api::CommandResult::Ok(None)
    }

    pub fn reload_services_action(
        &mut self,
        project_id: &str,
        cx: &mut impl ServiceCx,
    ) -> okena_core::api::CommandResult {
        match self.project_path(project_id).cloned() {
            Some(path) => {
                self.reload_project_services(project_id, &path, cx);
                okena_core::api::CommandResult::Ok(None)
            }
            None => okena_core::api::CommandResult::Err(format!("project not found: {project_id}")),
        }
    }
}

impl ServiceManager {
    pub fn new(backend: Arc<dyn TerminalBackend>, terminals: TerminalsRegistry) -> Self {
        Self {
            configs: HashMap::new(),
            instances: HashMap::new(),
            terminal_to_service: HashMap::new(),
            project_paths: HashMap::new(),
            project_writeback_owners: HashMap::new(),
            project_lifecycles: ProjectLifecycles::default(),
            pending_okena_launches: HashMap::new(),
            next_okena_launch_generation: 1,
            pending_okena_restarts: HashMap::new(),
            next_okena_restart_generation: 1,
            backend,
            terminals,
            docker_pollers: HashMap::new(),
            docker_mutations: DockerMutationQueue::default(),
            docker_mutation_runner: Arc::new(commands::CommandDockerMutationRunner),
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
                instance
                    .terminal_id
                    .as_ref()
                    .map(|tid| (name.clone(), tid.clone()))
            })
            .collect()
    }

    /// Report Compose subprocesses and queued mutations owned by the targets.
    ///
    /// Physical identities keep an in-flight mutation visible after a project
    /// reload or workspace replacement changes its logical project ID.
    pub fn compose_mutations_for(
        &self,
        project_ids: &HashSet<String>,
        compose_identities: &HashSet<ComposeProjectIdentity>,
    ) -> Vec<ComposeMutationActivity> {
        let mut activities = Vec::new();
        for active in self.docker_mutations.active.values() {
            for (mutation, queued) in std::iter::once((&active.current, false))
                .chain(active.pending.iter().map(|mutation| (mutation, true)))
            {
                if !project_ids.contains(&mutation.project_id)
                    && !compose_identities.contains(&mutation.compose_identity())
                {
                    continue;
                }
                activities.push(ComposeMutationActivity {
                    project_id: mutation.project_id.clone(),
                    project_path: mutation.project_path.clone(),
                    compose_file: mutation.compose_file.clone(),
                    service_name: mutation.service_name.clone(),
                    kind: mutation.kind,
                    queued,
                });
            }
        }
        activities.sort_by(|left, right| {
            left.project_path
                .cmp(&right.project_path)
                .then_with(|| left.compose_file.cmp(&right.compose_file))
                .then_with(|| left.queued.cmp(&right.queued))
                .then_with(|| left.service_name.cmp(&right.service_name))
        });
        activities
    }

    /// Okena services whose active intent must survive a backend migration.
    pub fn active_okena_service_names(&self, project_id: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .instances
            .iter()
            .filter(|((pid, _), instance)| {
                pid == project_id
                    && instance.kind == ServiceKind::Okena
                    && matches!(
                        instance.status,
                        ServiceStatus::Starting
                            | ServiceStatus::Running
                            | ServiceStatus::Restarting
                    )
            })
            .map(|((_, name), _)| name.clone())
            .collect();
        names.sort();
        names
    }

    /// Associate future terminal ownership write-back with one workspace snapshot.
    pub fn set_project_writeback_owner(
        &mut self,
        project_id: &str,
        project_path: &str,
        data_replacement_epoch: u64,
    ) {
        self.project_writeback_owners.insert(
            project_id.to_string(),
            (project_path.to_string(), data_replacement_epoch),
        );
    }

    /// Snapshot every attempted project, including projects with no service instances.
    pub fn service_terminal_writebacks(&self) -> Vec<ServiceTerminalWriteback> {
        self.project_writeback_owners
            .iter()
            .map(
                |(project_id, (project_path, data_replacement_epoch))| ServiceTerminalWriteback {
                    project_id: project_id.clone(),
                    project_path: project_path.clone(),
                    data_replacement_epoch: *data_replacement_epoch,
                    terminal_ids: self.service_terminal_ids(project_id),
                },
            )
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
        let mut docker: Vec<&ServiceInstance> = self
            .instances
            .iter()
            .filter(|((pid, name), inst)| {
                pid == project_id
                    && matches!(inst.kind, ServiceKind::DockerCompose { .. })
                    && !seen.contains(name)
            })
            .map(|(_, inst)| inst)
            .collect();
        docker.sort_by(|a, b| {
            a.is_extra
                .cmp(&b.is_extra)
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

    /// Capture the lifecycle state that must still own an async service apply.
    pub fn project_state_token(&self, project_id: &str) -> ServiceProjectStateToken {
        ServiceProjectStateToken {
            incarnation: self.project_lifecycles.current.get(project_id).cloned(),
            next_generation: self.project_lifecycles.next_generation,
        }
    }

    /// Revalidate a lifecycle snapshot after filesystem preparation completes.
    pub fn is_project_state_token_current(
        &self,
        project_id: &str,
        token: &ServiceProjectStateToken,
    ) -> bool {
        self.project_lifecycles.current.get(project_id) == token.incarnation.as_ref()
            && self.project_lifecycles.next_generation == token.next_generation
    }

    /// Reload an existing project's service lifecycle after an on-disk rename.
    /// Projects that have not been loaded yet pick up their path during load.
    /// Returns whether the manager has converged on `new_path`.
    pub fn update_project_path(
        &mut self,
        project_id: &str,
        new_path: &str,
        cx: &mut impl ServiceCx,
    ) -> bool {
        match self.project_paths.get(project_id) {
            None => true,
            Some(path) if path == new_path => true,
            Some(_) => {
                self.reload_project_services(project_id, new_path, cx);
                self.project_paths
                    .get(project_id)
                    .is_some_and(|path| path == new_path)
            }
        }
    }

    pub(super) fn begin_project_incarnation(
        &mut self,
        project_id: &str,
        project_path: &str,
    ) -> ProjectIncarnation {
        self.project_lifecycles.begin(project_id, project_path)
    }

    pub(super) fn project_incarnation(
        &self,
        project_id: &str,
        project_path: &str,
    ) -> Option<ProjectIncarnation> {
        self.project_lifecycles.get(project_id, project_path)
    }

    pub(super) fn invalidate_project_incarnation(&mut self, project_id: &str) {
        self.project_lifecycles.invalidate(project_id);
    }

    pub(super) fn is_project_incarnation_current(
        &self,
        project_id: &str,
        incarnation: &ProjectIncarnation,
    ) -> bool {
        self.project_lifecycles.is_current(project_id, incarnation)
            && self.project_paths.get(project_id) == Some(&incarnation.path)
    }

    pub(super) fn begin_okena_launch(
        &mut self,
        key: &(String, String),
        project_path: &str,
    ) -> OkenaLaunchToken {
        let generation = self.next_okena_launch_generation;
        self.next_okena_launch_generation = generation
            .checked_add(1)
            .expect("Okena service launch generation exhausted");
        let token = OkenaLaunchToken {
            generation,
            project_path: project_path.to_string(),
        };
        self.pending_okena_launches
            .insert(key.clone(), token.clone());
        token
    }

    pub(super) fn is_okena_launch_current(
        &self,
        key: &(String, String),
        token: &OkenaLaunchToken,
    ) -> bool {
        self.pending_okena_launches.get(key) == Some(token)
            && self.project_paths.get(&key.0) == Some(&token.project_path)
    }

    pub(super) fn finish_okena_launch(&mut self, key: &(String, String), token: &OkenaLaunchToken) {
        if self.pending_okena_launches.get(key) == Some(token) {
            self.pending_okena_launches.remove(key);
        }
    }

    pub(super) fn invalidate_okena_launch(&mut self, key: &(String, String)) {
        self.pending_okena_launches.remove(key);
    }

    pub(super) fn begin_okena_restart(
        &mut self,
        key: &(String, String),
        project_path: &str,
        terminal_id: Option<String>,
    ) -> OkenaRestartToken {
        self.invalidate_okena_restart(key, true);
        let generation = self.next_okena_restart_generation;
        self.next_okena_restart_generation = generation
            .checked_add(1)
            .expect("Okena service restart generation exhausted");
        let token = OkenaRestartToken {
            generation,
            project_path: project_path.to_string(),
            terminal_id,
        };
        if let Some(terminal_id) = &token.terminal_id
            && self.terminal_to_service.get(terminal_id) == Some(key)
        {
            self.terminal_to_service.remove(terminal_id);
        }
        self.pending_okena_restarts
            .insert(key.clone(), token.clone());
        token
    }

    pub(super) fn is_okena_restart_current(
        &self,
        key: &(String, String),
        token: &OkenaRestartToken,
    ) -> bool {
        self.pending_okena_restarts.get(key).is_some_and(|pending| {
            pending.generation == token.generation && pending.project_path == token.project_path
        }) && self.project_paths.get(&key.0) == Some(&token.project_path)
    }

    pub(super) fn finalize_okena_restart_terminal(
        &mut self,
        key: &(String, String),
        token: &OkenaRestartToken,
    ) -> bool {
        if !self.is_okena_restart_current(key, token) {
            return false;
        }
        let terminal_id = self
            .pending_okena_restarts
            .get_mut(key)
            .and_then(|pending| pending.terminal_id.take());
        if let Some(terminal_id) = terminal_id {
            self.backend.kill(&terminal_id);
            self.terminals.lock().remove(&terminal_id);
            if self.terminal_to_service.get(&terminal_id) == Some(key) {
                self.terminal_to_service.remove(&terminal_id);
            }
        }
        true
    }

    pub(super) fn finish_okena_restart(
        &mut self,
        key: &(String, String),
        token: &OkenaRestartToken,
    ) {
        if self.pending_okena_restarts.get(key).is_some_and(|pending| {
            pending.generation == token.generation && pending.project_path == token.project_path
        }) {
            self.pending_okena_restarts.remove(key);
        }
    }

    pub(super) fn invalidate_okena_restart(
        &mut self,
        key: &(String, String),
        kill_terminal: bool,
    ) -> Option<String> {
        let token = self.pending_okena_restarts.remove(key)?;
        let terminal_id = token.terminal_id?;
        if kill_terminal {
            self.backend.kill(&terminal_id);
        }
        self.terminals.lock().remove(&terminal_id);
        if self.terminal_to_service.get(&terminal_id) == Some(key) {
            self.terminal_to_service.remove(&terminal_id);
        }
        Some(terminal_id)
    }

    pub(super) fn drain_okena_restarts_for_project(
        &mut self,
        project_id: &str,
        kill_terminals: bool,
    ) -> Vec<String> {
        let keys: Vec<(String, String)> = self
            .pending_okena_restarts
            .keys()
            .filter(|(pending_project_id, _)| pending_project_id == project_id)
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|key| self.invalidate_okena_restart(&key, kill_terminals))
            .collect()
    }

    /// Whether the project has any service definitions loaded (Okena or Docker).
    pub fn has_services(&self, project_id: &str) -> bool {
        self.configs.get(project_id).is_some_and(|v| !v.is_empty())
            || self.instances.keys().any(|(pid, _)| pid == project_id)
    }

    /// Look up the terminal_id for a service.
    pub fn terminal_id_for(&self, project_id: &str, service_name: &str) -> Option<&String> {
        self.instances
            .get(&(project_id.to_string(), service_name.to_string()))
            .and_then(|i| i.terminal_id.as_ref())
    }
}

#[cfg(test)]
mod tests;
