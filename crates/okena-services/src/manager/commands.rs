//! Start / stop / restart individual services, plus PTY-exit handling.

use super::{
    DockerMutation, DockerMutationKind, MAX_RESTART_COUNT, OkenaLaunchToken, ServiceAsyncCx,
    ServiceCx, ServiceHandle, ServiceKind, ServiceManager, ServiceStatus,
};
use crate::port_detect;
use okena_core::process::is_process_alive;
use okena_terminal::backend::TerminalLaunchPlan;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::{Terminal, TerminalSize};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy)]
pub(super) enum OkenaLaunchFailure {
    Crashed,
    Reconnect { auto_start: bool },
}

pub(super) trait DockerMutationRunner: Send + Sync {
    fn run(&self, mutation: &DockerMutation) -> crate::ServiceResult<()>;
}

pub(super) struct CommandDockerMutationRunner;

impl DockerMutationRunner for CommandDockerMutationRunner {
    fn run(&self, mutation: &DockerMutation) -> crate::ServiceResult<()> {
        run_docker_mutation(mutation)
    }
}

impl ServiceManager {
    fn schedule_docker_mutation(
        &mut self,
        key: (String, String),
        project_path: String,
        compose_file: String,
        kind: DockerMutationKind,
        cx: &mut impl ServiceCx,
    ) {
        let project_incarnation = self.project_incarnation(&key.0, &project_path);
        let Some(first) = self.docker_mutations.enqueue(
            key.clone(),
            project_incarnation,
            project_path,
            compose_file,
            kind,
        ) else {
            return;
        };
        let runner = self.docker_mutation_runner.clone();

        cx.spawn_main(async move |this, cx| {
            let mut current = Some(first);
            while let Some(mutation) = current {
                let run = mutation.clone();
                let runner = runner.clone();
                let result = cx.spawn_blocking(async move { runner.run(&run) }).await;
                if let Err(error) = result {
                    log::error!(
                        "docker compose {} failed for '{}': {}",
                        mutation.kind.compose_argument(),
                        mutation.service_name,
                        error
                    );
                }

                current = this
                    .update(cx, |this, cx| {
                        if mutation
                            .project_incarnation
                            .as_ref()
                            .is_some_and(|incarnation| {
                                this.is_project_incarnation_current(&key.0, incarnation)
                            })
                        {
                            cx.notify();
                        }
                        this.docker_mutations.finish(&key, mutation.generation)
                    })
                    .flatten();
            }
        });
    }

    pub(super) fn complete_okena_terminal_launch(
        &mut self,
        key: &(String, String),
        launch_token: &OkenaLaunchToken,
        terminal_id: &str,
        cwd: &str,
        cx: &mut impl ServiceCx,
    ) -> bool {
        if !self.is_okena_launch_current(key, launch_token) {
            return false;
        }

        let running = self.instances.get(key).is_some_and(|instance| {
            instance.status == ServiceStatus::Starting
                && instance.terminal_id.as_deref() == Some(terminal_id)
        }) && self.terminal_to_service.get(terminal_id) == Some(key);
        let exited_without_restart = self.instances.get(key).is_some_and(|instance| {
            matches!(&instance.status, ServiceStatus::Crashed { .. })
                && instance.terminal_id.as_deref() == Some(terminal_id)
        }) && !self.terminal_to_service.contains_key(terminal_id);
        if !running && !exited_without_restart {
            return false;
        }

        let terminal = Arc::new(Terminal::new(
            terminal_id.to_string(),
            TerminalSize::default(),
            self.backend.transport(),
            cwd.to_string(),
        ));
        self.terminals
            .lock()
            .insert(terminal_id.to_string(), terminal);
        if running {
            if let Some(instance) = self.instances.get_mut(key) {
                instance.status = ServiceStatus::Running;
            }
            self.start_port_detection(&key.0, &key.1, cx);
        }
        self.finish_okena_launch(key, launch_token);
        cx.notify();
        true
    }

    pub(super) fn scheduled_okena_restart_is_current(
        &self,
        key: &(String, String),
        restart_count: u32,
    ) -> bool {
        self.instances.get(key).is_some_and(|instance| {
            instance.status == ServiceStatus::Restarting && instance.restart_count == restart_count
        })
    }

    pub(super) fn schedule_okena_restart(
        &self,
        project_id: &str,
        service_name: &str,
        project_path: &str,
        restart_delay_ms: u64,
        cx: &mut impl ServiceCx,
    ) {
        let Some(project_incarnation) = self.project_incarnation(project_id, project_path) else {
            return;
        };
        let key = (project_id.to_string(), service_name.to_string());
        let Some(restart_count) = self.instances.get(&key).and_then(|instance| {
            (instance.status == ServiceStatus::Restarting).then_some(instance.restart_count)
        }) else {
            return;
        };
        let project_id = project_id.to_string();
        let service_name = service_name.to_string();
        let project_path = project_path.to_string();
        let delay = Duration::from_millis(restart_delay_ms);

        cx.spawn_main(async move |this, cx| {
            cx.timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                let key = (project_id.clone(), service_name.clone());
                if this.is_project_incarnation_current(&project_id, &project_incarnation)
                    && this.scheduled_okena_restart_is_current(&key, restart_count)
                {
                    this.start_service(&project_id, &service_name, &project_path, cx);
                }
            });
        });
    }

    /// Start a service by spawning a PTY (Okena) or running `docker compose start` (Docker).
    pub fn start_service(
        &mut self,
        project_id: &str,
        service_name: &str,
        project_path: &str,
        cx: &mut impl ServiceCx,
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
                instance.status = ServiceStatus::Starting;
                cx.notify();
                self.schedule_docker_mutation(
                    key,
                    path,
                    compose_file,
                    DockerMutationKind::Start,
                    cx,
                );
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
        cx: &mut impl ServiceCx,
    ) {
        let key = (project_id.to_string(), service_name.to_string());
        self.invalidate_okena_restart(&key, true);
        let terminal_id = uuid::Uuid::new_v4().to_string();
        self.begin_okena_terminal_launch(
            project_id,
            service_name,
            project_path,
            terminal_id,
            OkenaLaunchFailure::Crashed,
            cx,
        );
    }

    pub(super) fn begin_okena_terminal_launch(
        &mut self,
        project_id: &str,
        service_name: &str,
        project_path: &str,
        terminal_id: String,
        failure: OkenaLaunchFailure,
        cx: &mut impl ServiceCx,
    ) {
        if self.project_incarnation(project_id, project_path).is_none() {
            return;
        }
        let key = (project_id.to_string(), service_name.to_string());
        if !self.instances.contains_key(&key) {
            return;
        }
        let launch_token = self.begin_okena_launch(&key, project_path);
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

        let launch_plan = TerminalLaunchPlan::for_shell(ShellType::for_command(command))
            .with_environment(instance.definition.env.clone().into_iter().collect());

        instance.status = ServiceStatus::Starting;
        instance.terminal_id = Some(terminal_id.clone());
        self.terminal_to_service
            .insert(terminal_id.clone(), key.clone());
        cx.notify();

        let backend = self.backend.clone();
        let project_id = project_id.to_string();
        let service_name = service_name.to_string();
        let project_path = project_path.to_string();
        cx.spawn_main(async move |this, cx| {
            let launch_backend = backend.clone();
            let launch_id = terminal_id.clone();
            let launch_cwd = cwd.clone();
            let result = cx
                .spawn_blocking(async move {
                    smol::unblock(move || {
                        launch_backend.reconnect_terminal_with_plan(
                            &launch_id,
                            &launch_cwd,
                            &launch_plan,
                        )
                    })
                    .await
                })
                .await;

            let actual_id = result.as_ref().ok().cloned();
            let (accepted, cleanup_requested) = this
                .update(cx, |this, cx| {
                    let key = (project_id.clone(), service_name.clone());
                    if result
                        .as_ref()
                        .is_ok_and(|returned_id| returned_id == &terminal_id)
                        && this.complete_okena_terminal_launch(
                            &key,
                            &launch_token,
                            &terminal_id,
                            &cwd,
                            cx,
                        )
                    {
                        return (true, false);
                    }
                    let current_launch = this.is_okena_launch_current(&key, &launch_token)
                        && this.instances.get(&key).is_some_and(|instance| {
                            instance.status == ServiceStatus::Starting
                                && instance.terminal_id.as_deref() == Some(&terminal_id)
                        })
                        && this.terminal_to_service.get(&terminal_id) == Some(&key);
                    if !current_launch {
                        let cleanup_requested =
                            this.terminal_to_service.get(&terminal_id) != Some(&key);
                        return (false, cleanup_requested);
                    }

                    match &result {
                        Ok(returned_id) if returned_id == &terminal_id => unreachable!(),
                        _ => {
                            this.terminal_to_service.remove(&terminal_id);
                            if let Some(instance) = this.instances.get_mut(&key) {
                                instance.terminal_id = None;
                                instance.status = match failure {
                                    OkenaLaunchFailure::Crashed => {
                                        ServiceStatus::Crashed { exit_code: None }
                                    }
                                    OkenaLaunchFailure::Reconnect { .. } => ServiceStatus::Stopped,
                                };
                            }
                            this.finish_okena_launch(&key, &launch_token);
                            cx.notify();
                            if matches!(failure, OkenaLaunchFailure::Reconnect { auto_start: true })
                            {
                                this.start_service(&project_id, &service_name, &project_path, cx);
                            }
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
                    cx.spawn_blocking(async move {
                        smol::unblock(move || cleanup_backend.kill(&cleanup_id)).await
                    })
                    .await;
                }
            }
        });
    }

    /// Stop a running service.
    pub fn stop_service(&mut self, project_id: &str, service_name: &str, cx: &mut impl ServiceCx) {
        let key = (project_id.to_string(), service_name.to_string());
        if !self.instances.contains_key(&key) {
            return;
        }
        self.invalidate_okena_launch(&key);
        self.invalidate_okena_restart(&key, true);
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
                let path = self
                    .project_paths
                    .get(project_id)
                    .cloned()
                    .unwrap_or_default();
                instance.status = ServiceStatus::Stopped;
                instance.detected_ports.clear();
                cx.notify();
                self.schedule_docker_mutation(
                    key,
                    path,
                    compose_file,
                    DockerMutationKind::Stop,
                    cx,
                );
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
        cx: &mut impl ServiceCx,
    ) {
        let project_incarnation = self.project_incarnation(project_id, project_path);
        let key = (project_id.to_string(), service_name.to_string());
        if !self.instances.contains_key(&key) {
            return;
        }
        self.invalidate_okena_launch(&key);
        let instance = match self.instances.get_mut(&key) {
            Some(i) => i,
            None => return,
        };

        match &instance.kind {
            ServiceKind::DockerCompose { compose_file } => {
                let compose_file = compose_file.clone();
                let path = project_path.to_string();

                // Kill log viewer PTY if any
                if let Some(terminal_id) = instance.terminal_id.take() {
                    self.backend.kill(&terminal_id);
                    self.terminals.lock().remove(&terminal_id);
                    self.terminal_to_service.remove(&terminal_id);
                }

                instance.status = ServiceStatus::Restarting;
                instance.detected_ports.clear();
                cx.notify();

                self.schedule_docker_mutation(
                    key,
                    path,
                    compose_file,
                    DockerMutationKind::Restart,
                    cx,
                );
            }
            ServiceKind::Okena => {
                // Take terminal_id now to prevent concurrent access.
                // The PtyManager handle is NOT removed yet — that happens in kill() below.
                let terminal_id = instance.terminal_id.take();

                instance.status = ServiceStatus::Restarting;
                instance.restart_count = 0;
                instance.detected_ports.clear();
                cx.notify();

                let restart_token =
                    self.begin_okena_restart(&key, project_path, terminal_id.clone());

                let pid = project_id.to_string();
                let name = service_name.to_string();
                let path = project_path.to_string();
                let backend = self.backend.clone();

                cx.spawn_main(async move |this, cx| {
                    // Collect descendant PIDs on background executor.
                    // get_service_pids() may spawn subprocesses (lsof/tmux)
                    // and get_descendant_pids() may call pgrep/wmic.
                    let old_pids: Vec<u32> = if let Some(ref tid) = terminal_id {
                        let tid = tid.clone();
                        let backend_ref = backend.clone();
                        cx.spawn_blocking(async move {
                            backend_ref
                                .get_service_pids(&tid)
                                .into_iter()
                                .flat_map(port_detect::get_descendant_pids)
                                .collect()
                        })
                        .await
                    } else {
                        Vec::new()
                    };

                    let is_current = this
                        .update(cx, |this, _| {
                            let key = (pid.clone(), name.clone());
                            this.finalize_okena_restart_terminal(&key, &restart_token)
                                && project_incarnation.as_ref().is_some_and(|incarnation| {
                                    this.is_project_incarnation_current(&pid, incarnation)
                                })
                        })
                        .unwrap_or(false);
                    if !is_current {
                        return;
                    }

                    // Wait for old processes to die
                    if !old_pids.is_empty() {
                        for _ in 0..100 {
                            if old_pids.iter().all(|&p| !is_process_alive(p)) {
                                break;
                            }
                            cx.timer(Duration::from_millis(50)).await;
                        }
                    }

                    let _ = this.update(cx, |this, cx| {
                        if !project_incarnation.as_ref().is_some_and(|incarnation| {
                            this.is_project_incarnation_current(&pid, incarnation)
                        }) {
                            return;
                        }
                        let key = (pid.clone(), name.clone());
                        if !this.is_okena_restart_current(&key, &restart_token) {
                            return;
                        }
                        this.finish_okena_restart(&key, &restart_token);
                        if let Some(instance) = this.instances.get(&key)
                            && instance.status == ServiceStatus::Restarting
                        {
                            this.start_service(&pid, &name, &path, cx);
                        }
                    });
                });
            }
        }
    }

    /// Start all services for a project.
    pub fn start_all(&mut self, project_id: &str, project_path: &str, cx: &mut impl ServiceCx) {
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
    pub fn stop_all(&mut self, project_id: &str, cx: &mut impl ServiceCx) {
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
        cx: &mut impl ServiceCx,
    ) -> bool {
        let key = match self.terminal_to_service.remove(terminal_id) {
            Some(key) => key,
            None => return false, // Not a service terminal
        };

        let (project_id, service_name) = key.clone();
        let project_path = self.project_paths.get(&project_id).cloned();

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

        if exit_code == Some(0) {
            instance.terminal_id = None;
            instance.status = ServiceStatus::Stopped;
            instance.restart_count = 0;
            self.terminals.lock().remove(terminal_id);
            self.invalidate_okena_launch(&key);
            cx.notify();
            return true;
        }

        let should_restart =
            instance.definition.restart_on_crash && instance.restart_count < MAX_RESTART_COUNT;
        if should_restart {
            // Auto-restart: clean up old terminal, will create new one
            instance.terminal_id = None;
            self.terminals.lock().remove(terminal_id);
            instance.status = ServiceStatus::Restarting;
            instance.restart_count += 1;
            let restart_delay_ms = instance.definition.restart_delay_ms;
            self.invalidate_okena_launch(&key);
            if let Some(project_path) = project_path {
                self.schedule_okena_restart(
                    &project_id,
                    &service_name,
                    &project_path,
                    restart_delay_ms,
                    cx,
                );
            }
        } else {
            // Crash without restart: keep terminal_id and Terminal in registry
            // so the user can see the crash output until they manually restart.
            instance.status = ServiceStatus::Crashed { exit_code };
        }

        cx.notify();
        true
    }
}

fn run_docker_mutation(mutation: &DockerMutation) -> crate::ServiceResult<()> {
    let mut command = okena_core::process::command("docker");
    match mutation.kind {
        DockerMutationKind::Start => {
            // `up -d` creates the service and its dependency graph; `start` does not.
            command.args([
                "compose",
                "-f",
                &mutation.compose_file,
                "up",
                "-d",
                &mutation.service_name,
            ]);
        }
        DockerMutationKind::Stop | DockerMutationKind::Restart => {
            command.args([
                "compose",
                "-f",
                &mutation.compose_file,
                mutation.kind.compose_argument(),
                &mutation.service_name,
            ]);
        }
    }
    command.current_dir(&mutation.project_path);
    let output = okena_core::process::safe_output(&mut command)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(crate::ServiceError::CommandExitError {
            context: format!("docker compose {}", mutation.kind.compose_argument()),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}
