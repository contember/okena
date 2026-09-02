//! Docker Compose service discovery, log-viewer PTYs, and status polling.

use super::{
    ServiceAsyncCx, ServiceCx, ServiceHandle, ServiceInstance, ServiceKind, ServiceManager,
    ServiceStatus,
};
use crate::config::ServiceDefinition;
use crate::docker_compose;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::{Terminal, TerminalSize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Steady-state gap between docker status polls for one project.
const DOCKER_POLL_INTERVAL: Duration = Duration::from_secs(5);

const DOCKER_DISCOVERY_RETRY_DELAYS: [Duration; 5] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(40),
    Duration::from_secs(60),
];

/// Per-project offset within the poll interval, so pollers wake staggered
/// instead of together. Stable for a given project id.
fn poll_stagger(project_id: &str) -> Duration {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project_id.hash(&mut hasher);
    Duration::from_millis(hasher.finish() % DOCKER_POLL_INTERVAL.as_millis() as u64)
}

fn reconcile_docker_statuses(
    instances: &mut HashMap<(String, String), ServiceInstance>,
    project_id: &str,
    compose_file: &str,
    statuses: &[docker_compose::DockerServiceStatus],
    protected_services: &HashSet<String>,
) -> (bool, bool) {
    let statuses_by_name: HashMap<&str, &docker_compose::DockerServiceStatus> = statuses
        .iter()
        .map(|status| (status.name.as_str(), status))
        .collect();
    let mut has_definitions = false;
    let mut changed = false;

    for ((pid, service_name), instance) in instances.iter_mut() {
        if pid != project_id
            || !matches!(
                &instance.kind,
                ServiceKind::DockerCompose {
                    compose_file: instance_file
                } if instance_file == compose_file
            )
        {
            continue;
        }
        has_definitions = true;

        // A snapshot taken across an active mutation can describe either side
        // of that mutation, so let the mutation's requested state win for now.
        if protected_services.contains(service_name) {
            continue;
        }

        let (new_status, new_ports) = match statuses_by_name.get(service_name.as_str()) {
            Some(status) => (
                docker_compose::map_docker_state(&status.state, status.exit_code),
                status.ports.clone(),
            ),
            None => (ServiceStatus::Stopped, Vec::new()),
        };
        if instance.status != new_status {
            instance.status = new_status;
            changed = true;
        }
        if instance.detected_ports != new_ports {
            instance.detected_ports = new_ports;
            changed = true;
        }
    }

    (has_definitions, changed)
}

fn reconcile_docker_poll_result(
    instances: &mut HashMap<(String, String), ServiceInstance>,
    mutations: &super::DockerMutationQueue,
    project_id: &str,
    compose_file: &str,
    statuses: &[docker_compose::DockerServiceStatus],
    protected_at_probe: &HashSet<String>,
    completed_generation_at_probe: u64,
) -> Option<(bool, bool)> {
    if mutations.completed_generation(project_id) != completed_generation_at_probe {
        return None;
    }
    let mut protected_services = protected_at_probe.clone();
    protected_services.extend(
        mutations
            .active_service_names(project_id)
            .map(str::to_string),
    );
    Some(reconcile_docker_statuses(
        instances,
        project_id,
        compose_file,
        statuses,
        &protected_services,
    ))
}

impl ServiceManager {
    /// Load Docker Compose services from an off-reactor filesystem snapshot.
    pub(super) fn load_docker_compose_services_prepared(
        &mut self,
        project_id: &str,
        project_path: &str,
        docker_config: Option<&crate::config::DockerComposeConfig>,
        detected_compose_file: Option<String>,
        cx: &mut impl ServiceCx,
    ) {
        // Check if explicitly disabled
        if docker_config
            .as_ref()
            .is_some_and(|dc| dc.enabled == Some(false))
        {
            return;
        }

        // Resolve compose file (fast filesystem check, OK on main thread)
        let compose_file = docker_config
            .and_then(|dc| dc.file.clone())
            .or(detected_compose_file);

        let Some(compose_file) = compose_file else {
            return;
        };
        let Some(incarnation) = self.project_incarnation(project_id, project_path) else {
            return;
        };

        // Extract what we need from the reference before spawning
        let filter: Option<Vec<String>> = docker_config
            .map(|dc| dc.services.clone())
            .filter(|s| !s.is_empty());

        let project_id = project_id.to_string();
        let project_path = project_path.to_string();

        // Move docker subprocess calls to background executor
        cx.spawn_main(async move |this, cx| {
            let mut retry_delays = DOCKER_DISCOVERY_RETRY_DELAYS.into_iter();
            let service_names = loop {
                let is_current = this
                    .update(cx, |this, _| {
                        this.is_project_incarnation_current(&project_id, &incarnation)
                    })
                    .unwrap_or(false);
                if !is_current {
                    return;
                }

                let path = project_path.clone();
                let file = compose_file.clone();
                let discovered = smol::unblock(move || {
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
                .await;

                if let Some(service_names) = discovered {
                    break service_names;
                }
                let Some(delay) = retry_delays.next() else {
                    log::warn!(
                        "Giving up Docker Compose discovery for project {} after bounded retries",
                        project_id
                    );
                    return;
                };
                cx.timer(delay).await;
            };

            let _ = this.update(cx, |this, cx| {
                if !this.is_project_incarnation_current(&project_id, &incarnation) {
                    return;
                }
                for name in &service_names {
                    let is_extra = filter.as_ref().is_some_and(|f| !f.contains(name));

                    let key = (project_id.clone(), name.clone());
                    this.instances
                        .entry(key)
                        .or_insert_with(|| ServiceInstance {
                            definition: ServiceDefinition {
                                name: name.clone(),
                                command: String::new(),
                                cwd: ".".to_string(),
                                env: HashMap::new(),
                                auto_start: false,
                                restart_on_crash: false,
                                restart_delay_ms: 0,
                            },
                            kind: ServiceKind::DockerCompose {
                                compose_file: compose_file.clone(),
                            },
                            status: ServiceStatus::Stopped,
                            terminal_id: None,
                            restart_count: 0,
                            detected_ports: Vec::new(),
                            is_extra,
                        });
                }

                // Start status poller
                this.start_docker_status_poller(
                    &project_id,
                    &project_path,
                    &compose_file,
                    incarnation,
                    cx,
                );
                cx.notify();
            });
        });
    }

    /// Reload Docker Compose services from an off-reactor filesystem snapshot.
    pub(super) fn reload_docker_compose_services_prepared(
        &mut self,
        project_id: &str,
        project_path: &str,
        docker_config: Option<&crate::config::DockerComposeConfig>,
        detected_compose_file: Option<String>,
        cx: &mut impl ServiceCx,
    ) {
        // Stop existing poller
        if let Some(cancel) = self.docker_pollers.remove(project_id) {
            cancel.store(true, Ordering::Relaxed);
        }

        // Remove old Docker instances
        let docker_keys: Vec<(String, String)> = self
            .instances
            .iter()
            .filter(|((pid, _), inst)| {
                pid == project_id && matches!(inst.kind, ServiceKind::DockerCompose { .. })
            })
            .map(|(k, _)| k.clone())
            .collect();

        for key in docker_keys {
            if let Some(instance) = self.instances.get(&key)
                && let Some(terminal_id) = &instance.terminal_id
            {
                self.backend.kill(terminal_id);
                self.terminals.lock().remove(terminal_id);
                self.terminal_to_service.remove(terminal_id);
            }
            self.instances.remove(&key);
        }

        // Reload
        self.load_docker_compose_services_prepared(
            project_id,
            project_path,
            docker_config,
            detected_compose_file,
            cx,
        );
    }

    /// Spawn a PTY running `docker compose logs -f --tail 200 <name>`.
    /// Stores the terminal_id on the instance.
    pub fn open_docker_logs(
        &mut self,
        project_id: &str,
        service_name: &str,
        cx: &mut impl ServiceCx,
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

        let _ = instance;
        let Some(project_incarnation) = self.project_incarnation(project_id, &project_path) else {
            return;
        };
        let terminal_id = uuid::Uuid::new_v4().to_string();
        let Some(instance) = self.instances.get_mut(&key) else {
            return;
        };
        instance.terminal_id = Some(terminal_id.clone());
        self.terminal_to_service
            .insert(terminal_id.clone(), key.clone());
        cx.notify();

        let backend = self.backend.clone();
        let terminals = self.terminals.clone();
        let project_id = project_id.to_string();
        let service_name = service_name.to_string();
        cx.spawn_main(async move |this, cx| {
            let launch_backend = backend.clone();
            let launch_id = terminal_id.clone();
            let launch_path = project_path.clone();
            let result = cx
                .spawn_blocking(move || {
                    launch_backend.reconnect_terminal(&launch_id, &launch_path, Some(&shell))
                })
                .await;
            let actual_id = result.as_ref().ok().cloned();
            let (accepted, cleanup_requested) = this
                .update(cx, |this, cx| {
                    let key = (project_id.clone(), service_name.clone());
                    let current_launch = this.instances.get(&key).is_some_and(|instance| {
                        instance.terminal_id.as_deref() == Some(&terminal_id)
                    }) && this.terminal_to_service.get(&terminal_id)
                        == Some(&key);
                    if !this.is_project_incarnation_current(&project_id, &project_incarnation)
                        || !current_launch
                    {
                        return (
                            false,
                            this.terminal_to_service.get(&terminal_id) != Some(&key),
                        );
                    }
                    match &result {
                        Ok(returned_id) if returned_id == &terminal_id => {
                            let terminal = Arc::new(Terminal::new(
                                terminal_id.clone(),
                                TerminalSize::default(),
                                backend.transport(),
                                project_path.clone(),
                            ));
                            terminals.lock().insert(terminal_id.clone(), terminal);
                            cx.notify();
                            (true, false)
                        }
                        _ => {
                            this.terminal_to_service.remove(&terminal_id);
                            if let Some(instance) = this.instances.get_mut(&key) {
                                instance.terminal_id = None;
                            }
                            cx.notify();
                            (false, true)
                        }
                    }
                })
                .unwrap_or((false, true));
            if !accepted {
                let mut cleanup_ids: std::collections::HashSet<String> = actual_id
                    .into_iter()
                    .filter(|actual_id| actual_id != &terminal_id || cleanup_requested)
                    .collect();
                if cleanup_requested {
                    cleanup_ids.insert(terminal_id);
                }
                for cleanup_id in cleanup_ids {
                    let cleanup_backend = backend.clone();
                    cx.spawn_blocking(move || cleanup_backend.kill(&cleanup_id))
                        .await;
                }
            }
        });
    }

    /// Start a background poller that updates Docker service statuses every 5s.
    fn start_docker_status_poller(
        &mut self,
        project_id: &str,
        project_path: &str,
        compose_file: &str,
        incarnation: super::ProjectIncarnation,
        cx: &mut impl ServiceCx,
    ) {
        // Cancel any existing poller for this project
        if let Some(old_cancel) = self.docker_pollers.remove(project_id) {
            old_cancel.store(true, Ordering::Relaxed);
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.docker_pollers
            .insert(project_id.to_string(), cancel.clone());

        let pid = project_id.to_string();
        let path = project_path.to_string();
        let file = compose_file.to_string();

        cx.spawn_main(async move |this, cx| {
            // Small initial delay, spread across the poll window so N projects
            // don't all wake in the same instant and pile up on the shared
            // `docker ps` snapshot. Derived from the project id so a given
            // project keeps its slot across restarts.
            cx.timer(Duration::from_secs(1) + poll_stagger(&pid)).await;

            let mut consecutive_failures: u32 = 0;

            loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let is_current = this
                    .update(cx, |this, _| {
                        this.is_project_incarnation_current(&pid, &incarnation)
                    })
                    .unwrap_or(false);
                if !is_current {
                    return;
                }

                let poll_fence = this
                    .update(cx, |this, _| {
                        if !this.is_project_incarnation_current(&pid, &incarnation) {
                            return None;
                        }
                        Some((
                            this.docker_mutations
                                .active_service_names(&pid)
                                .map(str::to_string)
                                .collect::<HashSet<_>>(),
                            this.docker_mutations.completed_generation(&pid),
                        ))
                    })
                    .flatten();
                let Some((protected_at_probe, completed_generation_at_probe)) = poll_fence else {
                    return;
                };

                let path_clone = path.clone();
                let file_clone = file.clone();
                let result = smol::unblock(move || {
                    okena_core::process::with_lane(okena_core::process::Lane::Poll, || {
                        docker_compose::poll_status(&path_clone, &file_clone)
                    })
                })
                .await;

                if cancel.load(Ordering::Relaxed) {
                    return;
                }

                match result {
                    Ok(statuses) => {
                        consecutive_failures = 0;
                        let should_stop = this
                            .update(cx, |this, cx| {
                                if !this.is_project_incarnation_current(&pid, &incarnation) {
                                    return true;
                                }
                                let Some((has_definitions, changed)) = reconcile_docker_poll_result(
                                    &mut this.instances,
                                    &this.docker_mutations,
                                    &pid,
                                    &file,
                                    &statuses,
                                    &protected_at_probe,
                                    completed_generation_at_probe,
                                ) else {
                                    return false;
                                };
                                if changed {
                                    cx.notify();
                                }
                                !has_definitions
                            })
                            .unwrap_or(true);

                        if should_stop {
                            return;
                        }
                    }
                    // Another project's poller is fetching the shared snapshot
                    // and there is nothing cached yet. Not a docker failure —
                    // keep the current statuses and retry on the normal cadence
                    // rather than backing off.
                    Err(crate::error::ServiceError::RefreshInFlight) => {}
                    Err(e) => {
                        consecutive_failures += 1;
                        log::warn!("Docker status poll failed for project {}: {}", pid, e);
                    }
                }

                // Back off on repeated failures: 5s → 10s → 20s → 40s → 60s (cap)
                let delay = if consecutive_failures == 0 {
                    DOCKER_POLL_INTERVAL
                } else {
                    Duration::from_secs(
                        (DOCKER_POLL_INTERVAL.as_secs() << consecutive_failures.min(4)).min(60),
                    )
                };
                cx.timer(delay).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn docker_instance(status: ServiceStatus, ports: Vec<u16>) -> ServiceInstance {
        ServiceInstance {
            definition: ServiceDefinition {
                name: "web".to_string(),
                command: String::new(),
                cwd: ".".to_string(),
                env: HashMap::new(),
                auto_start: false,
                restart_on_crash: false,
                restart_delay_ms: 0,
            },
            kind: ServiceKind::DockerCompose {
                compose_file: "compose.yml".to_string(),
            },
            status,
            terminal_id: None,
            restart_count: 0,
            detected_ports: ports,
            is_extra: false,
        }
    }

    #[test]
    fn empty_snapshot_stops_known_service_without_stopping_poller() {
        let key = ("project".to_string(), "web".to_string());
        let mut instances = HashMap::from([(
            key.clone(),
            docker_instance(ServiceStatus::Running, vec![8080]),
        )]);

        let (has_definitions, changed) = reconcile_docker_statuses(
            &mut instances,
            "project",
            "compose.yml",
            &[],
            &HashSet::new(),
        );

        assert!(has_definitions);
        assert!(changed);
        assert_eq!(instances[&key].status, ServiceStatus::Stopped);
        assert!(instances[&key].detected_ports.is_empty());
    }

    #[test]
    fn snapshot_does_not_override_active_mutation() {
        let key = ("project".to_string(), "web".to_string());
        let mut instances = HashMap::from([(
            key.clone(),
            docker_instance(ServiceStatus::Starting, Vec::new()),
        )]);
        let protected = HashSet::from(["web".to_string()]);

        let (has_definitions, changed) =
            reconcile_docker_statuses(&mut instances, "project", "compose.yml", &[], &protected);

        assert!(has_definitions);
        assert!(!changed);
        assert_eq!(instances[&key].status, ServiceStatus::Starting);
    }

    #[test]
    fn poll_started_before_completed_mutation_does_not_apply_stale_snapshot() {
        let key = ("project".to_string(), "web".to_string());
        let instances = Arc::new(Mutex::new(HashMap::from([(
            key.clone(),
            docker_instance(ServiceStatus::Stopped, Vec::new()),
        )])));
        let mutations = Arc::new(Mutex::new(super::super::DockerMutationQueue::default()));
        let (probe_started_tx, probe_started_rx) = std::sync::mpsc::channel();
        let (release_probe_tx, release_probe_rx) = std::sync::mpsc::channel();

        let poll_instances = instances.clone();
        let poll_mutations = mutations.clone();
        let poll = std::thread::spawn(move || {
            let completed_generation_at_probe = poll_mutations
                .lock()
                .expect("mutation lock")
                .completed_generation("project");
            probe_started_tx.send(()).expect("mark probe started");
            release_probe_rx.recv().expect("release stale probe");

            let stale_statuses = [docker_compose::DockerServiceStatus {
                name: "web".to_string(),
                state: "running".to_string(),
                exit_code: None,
                ports: vec![8080],
            }];
            let mutations = poll_mutations.lock().expect("mutation lock");
            let mut instances = poll_instances.lock().expect("instances lock");
            reconcile_docker_poll_result(
                &mut instances,
                &mutations,
                "project",
                "compose.yml",
                &stale_statuses,
                &HashSet::new(),
                completed_generation_at_probe,
            )
        });

        probe_started_rx.recv().expect("probe started");
        {
            let mut mutations = mutations.lock().expect("mutation lock");
            let mutation = mutations
                .enqueue(
                    key,
                    None,
                    "/project".to_string(),
                    "compose.yml".to_string(),
                    super::super::DockerMutationKind::Stop,
                )
                .expect("mutation starts immediately");
            assert!(
                mutations
                    .finish(&mutation.scope(), mutation.generation)
                    .is_none()
            );
        }
        release_probe_tx.send(()).expect("release probe");

        assert!(poll.join().expect("poll thread").is_none());
        let instances = instances.lock().expect("instances lock");
        assert_eq!(
            instances[&("project".into(), "web".into())].status,
            ServiceStatus::Stopped
        );
        assert!(
            instances[&("project".into(), "web".into())]
                .detected_ports
                .is_empty()
        );
    }

    #[test]
    fn discovery_retries_use_bounded_backoff() {
        assert_eq!(
            DOCKER_DISCOVERY_RETRY_DELAYS,
            [
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(20),
                Duration::from_secs(40),
                Duration::from_secs(60),
            ]
        );
    }

    #[test]
    fn poll_stagger_spreads_projects_across_the_interval() {
        let ids = [
            "project-a",
            "project-b",
            "project-c",
            "project-d",
            "project-e",
            "project-f",
        ];
        let offsets: Vec<_> = ids.iter().map(|id| poll_stagger(id)).collect();

        for offset in &offsets {
            assert!(
                *offset < DOCKER_POLL_INTERVAL,
                "an offset past the interval would skip a whole poll cycle"
            );
        }
        // Stable per id, so a project keeps its slot across restarts.
        assert_eq!(poll_stagger("project-a"), offsets[0]);
        // And genuinely spread, rather than every project landing together.
        let distinct: std::collections::HashSet<_> = offsets.iter().collect();
        assert!(
            distinct.len() >= ids.len() - 1,
            "expected distinct offsets, got {offsets:?}"
        );
    }
}
