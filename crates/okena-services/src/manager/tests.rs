use super::commands::OkenaLaunchFailure;
use super::*;
use crate::config::{OkenaProjectConfig, PreparedProjectConfig, ServiceDefinition};
use okena_terminal::backend::{LocalBackend, TerminalBackend, TerminalLaunchPlan};
use okena_terminal::pty_manager::PtyManager;
use okena_terminal::session_backend::SessionBackend;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::TerminalTransport;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::{Future, ready};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Clone)]
struct NoopHandle;

struct NoopAsyncCx;

#[derive(Clone, Default)]
struct RecordingCx {
    spawned: Arc<AtomicUsize>,
    notifications: Arc<AtomicUsize>,
}

impl ServiceCx for RecordingCx {
    type Handle = NoopHandle;
    type AsyncCx = NoopAsyncCx;

    fn notify(&mut self) {
        self.notifications.fetch_add(1, Ordering::Relaxed);
    }

    fn spawn_main<F>(&self, _f: F)
    where
        F: AsyncFnOnce(Self::Handle, &mut Self::AsyncCx) + 'static,
    {
        self.spawned.fetch_add(1, Ordering::Relaxed);
    }
}

impl ServiceHandle for NoopHandle {
    type AsyncCx = NoopAsyncCx;

    fn update<R>(
        &self,
        _cx: &mut Self::AsyncCx,
        _f: impl FnOnce(&mut ServiceManager, &mut <Self::AsyncCx as ServiceAsyncCx>::ReentryCx<'_>) -> R,
    ) -> Option<R> {
        None
    }
}

impl ServiceAsyncCx for NoopAsyncCx {
    type ReentryCx<'a> = RecordingCx;

    fn spawn_blocking<T>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> impl Future<Output = T>
    where
        T: Send + 'static,
    {
        future
    }

    fn timer(&self, _duration: Duration) -> impl Future<Output = ()> {
        ready(())
    }
}

#[derive(Clone)]
struct ExecutingHandle {
    manager: Weak<RefCell<ServiceManager>>,
    executor: Rc<smol::LocalExecutor<'static>>,
    notifications: Arc<AtomicUsize>,
}

struct ExecutingAsyncCx;

struct ExecutingCx {
    handle: ExecutingHandle,
}

impl ServiceCx for ExecutingCx {
    type Handle = ExecutingHandle;
    type AsyncCx = ExecutingAsyncCx;

    fn notify(&mut self) {
        self.handle.notifications.fetch_add(1, Ordering::Relaxed);
    }

    fn spawn_main<F>(&self, f: F)
    where
        F: AsyncFnOnce(Self::Handle, &mut Self::AsyncCx) + 'static,
    {
        let handle = self.handle.clone();
        let executor = handle.executor.clone();
        executor
            .spawn(async move {
                f(handle, &mut ExecutingAsyncCx).await;
            })
            .detach();
    }
}

impl ServiceHandle for ExecutingHandle {
    type AsyncCx = ExecutingAsyncCx;

    fn update<R>(
        &self,
        _cx: &mut Self::AsyncCx,
        f: impl FnOnce(&mut ServiceManager, &mut <Self::AsyncCx as ServiceAsyncCx>::ReentryCx<'_>) -> R,
    ) -> Option<R> {
        let manager = self.manager.upgrade()?;
        let mut manager = manager.try_borrow_mut().ok()?;
        let mut cx = ExecutingCx {
            handle: self.clone(),
        };
        Some(f(&mut manager, &mut cx))
    }
}

impl ServiceAsyncCx for ExecutingAsyncCx {
    type ReentryCx<'a> = ExecutingCx;

    fn spawn_blocking<T>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> impl Future<Output = T>
    where
        T: Send + 'static,
    {
        smol::unblock(move || smol::block_on(future))
    }

    fn timer(&self, duration: Duration) -> impl Future<Output = ()> {
        async move {
            smol::Timer::after(duration).await;
        }
    }
}

struct BarrierDockerRunner {
    events: async_channel::Sender<DockerMutationKind>,
    release_start: async_channel::Receiver<()>,
}

struct TimeoutDockerRunner {
    events: async_channel::Sender<DockerMutationKind>,
}

struct ProjectBarrierDockerRunner {
    events: async_channel::Sender<(String, String)>,
    releases: async_channel::Receiver<()>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

struct RecordingTerminalBackend {
    local: LocalBackend,
    plans: async_channel::Sender<TerminalLaunchPlan>,
}

struct BarrierRestartBackend {
    local: LocalBackend,
    pid_lookups: async_channel::Sender<String>,
    release_pid_lookup: async_channel::Receiver<()>,
    kills: async_channel::Sender<String>,
    plans: async_channel::Sender<TerminalLaunchPlan>,
}

impl TerminalBackend for RecordingTerminalBackend {
    fn transport(&self) -> Arc<dyn TerminalTransport> {
        self.local.transport()
    }

    fn create_terminal(&self, cwd: &str, shell: Option<&ShellType>) -> anyhow::Result<String> {
        self.local.create_terminal(cwd, shell)
    }

    fn reconnect_terminal(
        &self,
        terminal_id: &str,
        _cwd: &str,
        _shell: Option<&ShellType>,
    ) -> anyhow::Result<String> {
        Ok(terminal_id.to_string())
    }

    fn reconnect_terminal_with_plan(
        &self,
        terminal_id: &str,
        _cwd: &str,
        plan: &TerminalLaunchPlan,
    ) -> anyhow::Result<String> {
        self.plans
            .send_blocking(plan.clone())
            .expect("record terminal launch plan");
        Ok(terminal_id.to_string())
    }

    fn kill(&self, terminal_id: &str) {
        self.local.kill(terminal_id);
    }

    fn capture_buffer(&self, terminal_id: &str) -> Option<PathBuf> {
        self.local.capture_buffer(terminal_id)
    }

    fn supports_buffer_capture(&self) -> bool {
        self.local.supports_buffer_capture()
    }

    fn is_remote(&self) -> bool {
        self.local.is_remote()
    }

    fn get_shell_pid(&self, terminal_id: &str) -> Option<u32> {
        self.local.get_shell_pid(terminal_id)
    }

    fn get_service_pids(&self, terminal_id: &str) -> Vec<u32> {
        self.local.get_service_pids(terminal_id)
    }
}

impl TerminalBackend for BarrierRestartBackend {
    fn transport(&self) -> Arc<dyn TerminalTransport> {
        self.local.transport()
    }

    fn create_terminal(&self, cwd: &str, shell: Option<&ShellType>) -> anyhow::Result<String> {
        self.local.create_terminal(cwd, shell)
    }

    fn reconnect_terminal(
        &self,
        terminal_id: &str,
        _cwd: &str,
        _shell: Option<&ShellType>,
    ) -> anyhow::Result<String> {
        Ok(terminal_id.to_string())
    }

    fn reconnect_terminal_with_plan(
        &self,
        terminal_id: &str,
        _cwd: &str,
        plan: &TerminalLaunchPlan,
    ) -> anyhow::Result<String> {
        self.plans
            .send_blocking(plan.clone())
            .expect("record terminal launch plan");
        Ok(terminal_id.to_string())
    }

    fn kill(&self, terminal_id: &str) {
        self.kills
            .send_blocking(terminal_id.to_string())
            .expect("record terminal kill");
    }

    fn capture_buffer(&self, terminal_id: &str) -> Option<PathBuf> {
        self.local.capture_buffer(terminal_id)
    }

    fn supports_buffer_capture(&self) -> bool {
        self.local.supports_buffer_capture()
    }

    fn is_remote(&self) -> bool {
        self.local.is_remote()
    }

    fn get_shell_pid(&self, terminal_id: &str) -> Option<u32> {
        self.local.get_shell_pid(terminal_id)
    }

    fn get_service_pids(&self, terminal_id: &str) -> Vec<u32> {
        self.pid_lookups
            .send_blocking(terminal_id.to_string())
            .expect("record PID lookup");
        self.release_pid_lookup
            .recv_blocking()
            .expect("release PID lookup");
        Vec::new()
    }
}

impl commands::DockerMutationRunner for BarrierDockerRunner {
    fn run(&self, mutation: &DockerMutation) -> crate::ServiceResult<()> {
        self.events
            .send_blocking(mutation.kind)
            .expect("record Docker mutation");
        if mutation.kind == DockerMutationKind::Start {
            self.release_start
                .recv_blocking()
                .expect("release Docker start");
        }
        Ok(())
    }
}

impl commands::DockerMutationRunner for TimeoutDockerRunner {
    fn run(&self, mutation: &DockerMutation) -> crate::ServiceResult<()> {
        self.events
            .send_blocking(mutation.kind)
            .expect("record Docker mutation");
        if mutation.kind == DockerMutationKind::Start {
            return Err(crate::ServiceError::CommandFailed(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "simulated Docker timeout",
            )));
        }
        Ok(())
    }
}

impl commands::DockerMutationRunner for ProjectBarrierDockerRunner {
    fn run(&self, mutation: &DockerMutation) -> crate::ServiceResult<()> {
        let in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(in_flight, Ordering::SeqCst);
        self.events
            .send_blocking((mutation.project_id.clone(), mutation.service_name.clone()))
            .expect("record Docker mutation");
        self.releases
            .recv_blocking()
            .expect("release Docker mutation");
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

struct ProjectDir(PathBuf);

impl ProjectDir {
    fn with_config(config: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "okena-services-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create test project");
        std::fs::write(path.join("okena.yaml"), config).expect("write test config");
        Self(path)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    fn write_config(&self, config: &str) {
        std::fs::write(self.0.join("okena.yaml"), config).expect("rewrite test config");
    }
}

impl Drop for ProjectDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove test project");
    }
}

fn manager() -> ServiceManager {
    let (pty_manager, _events) = PtyManager::new(SessionBackend::None);
    let backend = Arc::new(LocalBackend::new(Arc::new(pty_manager)));
    let terminals: okena_terminal::TerminalsRegistry = Arc::new(Default::default());
    ServiceManager::new(backend, terminals)
}

fn recording_manager() -> (ServiceManager, async_channel::Receiver<TerminalLaunchPlan>) {
    let (pty_manager, _events) = PtyManager::new(SessionBackend::None);
    let local = LocalBackend::new(Arc::new(pty_manager));
    let (plans_tx, plans_rx) = async_channel::unbounded();
    let backend = Arc::new(RecordingTerminalBackend {
        local,
        plans: plans_tx,
    });
    let terminals: okena_terminal::TerminalsRegistry = Arc::new(Default::default());
    (ServiceManager::new(backend, terminals), plans_rx)
}

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
        ServiceStatus::Crashed { exit_code: Some(1) }
    );
    assert_eq!(instance.restart_count, MAX_RESTART_COUNT);
}

#[test]
fn handle_exit_no_restart() {
    let (_key, mut instance) = make_instance("proj1", "svc1", false, 0, ServiceStatus::Running);
    simulate_exit(&mut instance, None);
    assert_eq!(instance.status, ServiceStatus::Crashed { exit_code: None });
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
fn clean_okena_exit_stops_without_scheduling_restart() {
    let path = "/project";
    let mut manager = manager();
    manager.project_paths.insert("project".into(), path.into());
    manager.begin_project_incarnation("project", path);
    let (key, instance) = make_instance("project", "web", true, 2, ServiceStatus::Running);
    let terminal_id = instance.terminal_id.clone().expect("running terminal");
    manager.instances.insert(key.clone(), instance);
    manager
        .terminal_to_service
        .insert(terminal_id.clone(), key.clone());
    let mut cx = RecordingCx::default();

    assert!(manager.handle_service_exit(&terminal_id, Some(0), &mut cx));

    assert_eq!(manager.instances[&key].status, ServiceStatus::Stopped);
    assert_eq!(manager.instances[&key].terminal_id, None);
    assert_eq!(manager.instances[&key].restart_count, 0);
    assert!(!manager.terminal_to_service.contains_key(&terminal_id));
    assert_eq!(cx.spawned.load(Ordering::Relaxed), 0);
}

#[test]
fn scheduled_restart_requires_the_same_restarting_state() {
    let mut manager = manager();
    let (key, instance) = make_instance("project", "web", true, 1, ServiceStatus::Restarting);
    let scheduled_restart_count = instance.restart_count;
    manager.instances.insert(key.clone(), instance);

    assert!(manager.scheduled_okena_restart_is_current(&key, scheduled_restart_count));

    manager.stop_service("project", "web", &mut RecordingCx::default());

    assert!(
        !manager.scheduled_okena_restart_is_current(&key, scheduled_restart_count),
        "a manual stop must invalidate the pending restart"
    );
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
            instance
                .terminal_id
                .as_ref()
                .map(|tid| (name.clone(), tid.clone()))
        })
        .collect();

    assert_eq!(ids.len(), 1);
    assert_eq!(ids.get("web"), Some(&"term-web".to_string()));
    assert!(!ids.contains_key("api")); // No terminal_id
}

#[test]
fn from_api_maps_known_statuses() {
    assert_eq!(
        ServiceStatus::from_api("running", None),
        ServiceStatus::Running
    );
    assert_eq!(
        ServiceStatus::from_api("starting", None),
        ServiceStatus::Starting
    );
    assert_eq!(
        ServiceStatus::from_api("restarting", None),
        ServiceStatus::Restarting
    );
    assert_eq!(
        ServiceStatus::from_api("crashed", None),
        ServiceStatus::Crashed { exit_code: None }
    );
    assert_eq!(
        ServiceStatus::from_api("crashed", Some(1)),
        ServiceStatus::Crashed { exit_code: Some(1) }
    );
    assert_eq!(
        ServiceStatus::from_api("stopped", None),
        ServiceStatus::Stopped
    );
    assert_eq!(
        ServiceStatus::from_api("unknown", None),
        ServiceStatus::Stopped
    );
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
            kind: ServiceKind::DockerCompose {
                compose_file: "docker-compose.yml".to_string(),
            },
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
            instance
                .terminal_id
                .as_ref()
                .map(|tid| (name.clone(), tid.clone()))
        })
        .collect();

    assert_eq!(ids.len(), 1);
    assert!(ids.contains_key("web"));
    assert!(!ids.contains_key("db")); // Docker service excluded
}

#[test]
fn docker_start_then_stop_dispatches_in_request_order() {
    let path = "/project";
    let key = ("project".to_string(), "web".to_string());
    let mut manager = manager();
    manager.project_paths.insert(key.0.clone(), path.into());
    manager.begin_project_incarnation(&key.0, path);
    let (_, mut instance) = make_docker_instance(&key.0, &key.1, ServiceStatus::Stopped);
    instance.terminal_id = None;
    manager.instances.insert(key.clone(), instance);
    let mut cx = RecordingCx::default();

    manager.start_service(&key.0, &key.1, path, &mut cx);
    let scope = manager
        .docker_mutations
        .active
        .keys()
        .next()
        .unwrap()
        .clone();
    let active_generation = manager.docker_mutations.active[&scope].current.generation;
    assert_eq!(cx.spawned.load(Ordering::Relaxed), 1);

    manager.stop_service(&key.0, &key.1, &mut cx);

    assert_eq!(cx.spawned.load(Ordering::Relaxed), 1);
    assert_eq!(manager.instances[&key].status, ServiceStatus::Stopped);
    assert_eq!(
        manager.docker_mutations.active[&scope]
            .pending
            .iter()
            .map(|mutation| mutation.kind)
            .collect::<Vec<_>>(),
        vec![DockerMutationKind::Stop]
    );

    let stop = manager
        .docker_mutations
        .finish(&scope, active_generation)
        .expect("stop must dispatch after start completes");
    assert_eq!(stop.kind, DockerMutationKind::Stop);
    assert!(
        manager
            .docker_mutations
            .finish(&scope, stop.generation)
            .is_none()
    );
    assert!(!manager.docker_mutations.active.contains_key(&scope));
}

#[test]
fn docker_start_then_immediate_stop_waits_for_running_compose_command() {
    let path = "/project";
    let key = ("project".to_string(), "web".to_string());
    let (events_tx, events_rx) = async_channel::unbounded();
    let (release_tx, release_rx) = async_channel::bounded(1);
    let mut service_manager = manager();
    service_manager.docker_mutation_runner = Arc::new(BarrierDockerRunner {
        events: events_tx,
        release_start: release_rx,
    });
    service_manager
        .project_paths
        .insert(key.0.clone(), path.into());
    service_manager.begin_project_incarnation(&key.0, path);
    let (_, mut instance) = make_docker_instance(&key.0, &key.1, ServiceStatus::Stopped);
    instance.terminal_id = None;
    service_manager.instances.insert(key.clone(), instance);

    let executor = Rc::new(smol::LocalExecutor::new());
    let service_manager = Rc::new(RefCell::new(service_manager));
    let handle = ExecutingHandle {
        manager: Rc::downgrade(&service_manager),
        executor: executor.clone(),
        notifications: Arc::new(AtomicUsize::new(0)),
    };

    smol::block_on(executor.run(async {
        let mut cx = ExecutingCx { handle };
        {
            let mut manager = service_manager.borrow_mut();
            manager.start_service(&key.0, &key.1, path, &mut cx);
            manager.stop_service(&key.0, &key.1, &mut cx);
        }

        assert_eq!(events_rx.recv().await, Ok(DockerMutationKind::Start));
        assert!(
            events_rx.try_recv().is_err(),
            "stop must not dispatch while compose up is blocked"
        );

        release_tx.send(()).await.expect("release compose up");
        assert_eq!(events_rx.recv().await, Ok(DockerMutationKind::Stop));

        while service_manager
            .borrow()
            .docker_mutations
            .active
            .values()
            .any(|active| active.current.project_id == key.0)
        {
            smol::Timer::after(Duration::from_millis(1)).await;
        }
        assert_eq!(
            service_manager.borrow().instances[&key].status,
            ServiceStatus::Stopped
        );
    }));
}

#[test]
fn docker_mutation_timeout_releases_queue_without_overwriting_newer_intent() {
    let path = "/project";
    let key = ("project".to_string(), "web".to_string());
    let (events_tx, events_rx) = async_channel::unbounded();
    let mut service_manager = manager();
    service_manager.docker_mutation_runner = Arc::new(TimeoutDockerRunner { events: events_tx });
    service_manager
        .project_paths
        .insert(key.0.clone(), path.into());
    service_manager.begin_project_incarnation(&key.0, path);
    let (_, mut instance) = make_docker_instance(&key.0, &key.1, ServiceStatus::Stopped);
    instance.terminal_id = None;
    service_manager.instances.insert(key.clone(), instance);

    let executor = Rc::new(smol::LocalExecutor::new());
    let service_manager = Rc::new(RefCell::new(service_manager));
    let handle = ExecutingHandle {
        manager: Rc::downgrade(&service_manager),
        executor: executor.clone(),
        notifications: Arc::new(AtomicUsize::new(0)),
    };

    smol::block_on(executor.run(async {
        let mut cx = ExecutingCx { handle };
        {
            let mut manager = service_manager.borrow_mut();
            manager.start_service(&key.0, &key.1, path, &mut cx);
            manager.stop_service(&key.0, &key.1, &mut cx);
        }

        assert_eq!(events_rx.recv().await, Ok(DockerMutationKind::Start));
        assert_eq!(events_rx.recv().await, Ok(DockerMutationKind::Stop));
        while service_manager
            .borrow()
            .docker_mutations
            .active
            .values()
            .any(|active| active.current.project_id == key.0)
        {
            smol::Timer::after(Duration::from_millis(1)).await;
        }
        assert_eq!(
            service_manager.borrow().instances[&key].status,
            ServiceStatus::Stopped
        );
    }));
}

#[test]
fn docker_start_all_timeouts_restore_each_stopped_status() {
    let path = "/project";
    let project_id = "project";
    let (events_tx, events_rx) = async_channel::unbounded();
    let mut service_manager = manager();
    service_manager.docker_mutation_runner = Arc::new(TimeoutDockerRunner { events: events_tx });
    service_manager
        .project_paths
        .insert(project_id.into(), path.into());
    service_manager.begin_project_incarnation(project_id, path);
    for name in ["web", "worker"] {
        let (key, instance) = make_docker_instance(project_id, name, ServiceStatus::Stopped);
        service_manager.instances.insert(key, instance);
    }

    let executor = Rc::new(smol::LocalExecutor::new());
    let service_manager = Rc::new(RefCell::new(service_manager));
    let handle = ExecutingHandle {
        manager: Rc::downgrade(&service_manager),
        executor: executor.clone(),
        notifications: Arc::new(AtomicUsize::new(0)),
    };

    smol::block_on(executor.run(async {
        let mut cx = ExecutingCx { handle };
        service_manager
            .borrow_mut()
            .start_all(project_id, path, &mut cx);

        assert_eq!(events_rx.recv().await, Ok(DockerMutationKind::Start));
        assert_eq!(events_rx.recv().await, Ok(DockerMutationKind::Start));
        while service_manager
            .borrow()
            .docker_mutations
            .active
            .values()
            .any(|active| active.current.project_id == project_id)
        {
            smol::Timer::after(Duration::from_millis(1)).await;
        }
        for name in ["web", "worker"] {
            assert_eq!(
                service_manager.borrow().instances[&(project_id.into(), name.into())].status,
                ServiceStatus::Stopped
            );
        }
    }));
}

#[test]
fn docker_start_all_serializes_mutations_for_one_compose_project() {
    let path = "/project";
    let project_id = "project";
    let (events_tx, events_rx) = async_channel::unbounded();
    let (release_tx, release_rx) = async_channel::unbounded();
    let runner = Arc::new(ProjectBarrierDockerRunner {
        events: events_tx,
        releases: release_rx,
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
    });
    let mut service_manager = manager();
    service_manager.docker_mutation_runner = runner.clone();
    service_manager
        .project_paths
        .insert(project_id.into(), path.into());
    service_manager.begin_project_incarnation(project_id, path);
    for name in ["web", "worker"] {
        let (key, instance) = make_docker_instance(project_id, name, ServiceStatus::Stopped);
        service_manager.instances.insert(key, instance);
    }

    let executor = Rc::new(smol::LocalExecutor::new());
    let service_manager = Rc::new(RefCell::new(service_manager));
    let handle = ExecutingHandle {
        manager: Rc::downgrade(&service_manager),
        executor: executor.clone(),
        notifications: Arc::new(AtomicUsize::new(0)),
    };

    smol::block_on(executor.run(async {
        let mut cx = ExecutingCx { handle };
        service_manager
            .borrow_mut()
            .start_all(project_id, path, &mut cx);

        let first = events_rx.recv().await.expect("first Docker mutation");
        assert!(events_rx.try_recv().is_err());
        release_tx.send(()).await.expect("release first mutation");
        let second = events_rx.recv().await.expect("second Docker mutation");
        assert_ne!(first.1, second.1);
        release_tx.send(()).await.expect("release second mutation");

        while !service_manager.borrow().docker_mutations.active.is_empty() {
            smol::Timer::after(Duration::from_millis(1)).await;
        }
        assert_eq!(runner.max_in_flight.load(Ordering::SeqCst), 1);
    }));
}

#[test]
fn docker_mutations_for_unrelated_compose_projects_can_overlap() {
    let (events_tx, events_rx) = async_channel::unbounded();
    let (release_tx, release_rx) = async_channel::unbounded();
    let runner = Arc::new(ProjectBarrierDockerRunner {
        events: events_tx,
        releases: release_rx,
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
    });
    let mut service_manager = manager();
    service_manager.docker_mutation_runner = runner.clone();
    for (project_id, path) in [("project-a", "/project-a"), ("project-b", "/project-b")] {
        service_manager
            .project_paths
            .insert(project_id.into(), path.into());
        service_manager.begin_project_incarnation(project_id, path);
        let (key, instance) = make_docker_instance(project_id, "web", ServiceStatus::Stopped);
        service_manager.instances.insert(key, instance);
    }

    let executor = Rc::new(smol::LocalExecutor::new());
    let service_manager = Rc::new(RefCell::new(service_manager));
    let handle = ExecutingHandle {
        manager: Rc::downgrade(&service_manager),
        executor: executor.clone(),
        notifications: Arc::new(AtomicUsize::new(0)),
    };

    smol::block_on(executor.run(async {
        let mut cx = ExecutingCx { handle };
        {
            let mut manager = service_manager.borrow_mut();
            manager.start_service("project-a", "web", "/project-a", &mut cx);
            manager.start_service("project-b", "web", "/project-b", &mut cx);
        }

        let first = events_rx.recv().await.expect("first Docker mutation");
        let second = events_rx.recv().await.expect("overlapping Docker mutation");
        assert_ne!(first.0, second.0);
        assert_eq!(runner.in_flight.load(Ordering::SeqCst), 2);
        release_tx.send(()).await.expect("release first mutation");
        release_tx.send(()).await.expect("release second mutation");

        while !service_manager.borrow().docker_mutations.active.is_empty() {
            smol::Timer::after(Duration::from_millis(1)).await;
        }
        assert_eq!(runner.max_in_flight.load(Ordering::SeqCst), 2);
    }));
}

#[test]
fn project_unload_discards_queued_docker_mutations() {
    let path = "/project";
    let project_id = "project";
    let (events_tx, events_rx) = async_channel::unbounded();
    let (release_tx, release_rx) = async_channel::unbounded();
    let runner = Arc::new(ProjectBarrierDockerRunner {
        events: events_tx,
        releases: release_rx,
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
    });
    let mut service_manager = manager();
    service_manager.docker_mutation_runner = runner;
    service_manager
        .project_paths
        .insert(project_id.into(), path.into());
    service_manager.begin_project_incarnation(project_id, path);
    for name in ["web", "worker"] {
        let (key, instance) = make_docker_instance(project_id, name, ServiceStatus::Stopped);
        service_manager.instances.insert(key, instance);
    }

    let executor = Rc::new(smol::LocalExecutor::new());
    let service_manager = Rc::new(RefCell::new(service_manager));
    let handle = ExecutingHandle {
        manager: Rc::downgrade(&service_manager),
        executor: executor.clone(),
        notifications: Arc::new(AtomicUsize::new(0)),
    };

    smol::block_on(executor.run(async {
        let mut cx = ExecutingCx { handle };
        service_manager
            .borrow_mut()
            .start_all(project_id, path, &mut cx);
        events_rx.recv().await.expect("first Docker mutation");
        service_manager
            .borrow_mut()
            .unload_project_services(project_id, &mut cx);
        release_tx.send(()).await.expect("release active mutation");

        while !service_manager.borrow().docker_mutations.active.is_empty() {
            smol::Timer::after(Duration::from_millis(1)).await;
        }
        assert!(events_rx.try_recv().is_err());
    }));
}

#[test]
fn docker_mutations_remain_serialized_across_project_incarnations() {
    let key = ("project".to_string(), "web".to_string());
    let mut queue = DockerMutationQueue::default();
    let first_incarnation = ProjectIncarnation {
        generation: 1,
        path: "/repo".into(),
    };
    let replacement_incarnation = ProjectIncarnation {
        generation: 2,
        path: "/repo".into(),
    };

    let start = queue
        .enqueue(
            key.clone(),
            Some(first_incarnation),
            "/repo".into(),
            "compose.yml".into(),
            DockerMutationKind::Start,
        )
        .expect("first mutation dispatches immediately");
    assert!(
        queue
            .enqueue(
                key.clone(),
                Some(replacement_incarnation.clone()),
                "/repo".into(),
                "compose.yml".into(),
                DockerMutationKind::Stop,
            )
            .is_none(),
        "replacement work must not race the active mutation"
    );

    let scope = start.scope();
    let stop = queue
        .finish(&scope, start.generation)
        .expect("replacement stop dispatches after the old mutation drains");
    assert_eq!(stop.kind, DockerMutationKind::Stop);
    assert_eq!(stop.project_incarnation, Some(replacement_incarnation));
}

#[test]
fn docker_mutation_scope_follows_compose_path_across_workspace_projects() {
    let mut queue = DockerMutationQueue::default();
    let first = queue
        .enqueue(
            ("project-a".into(), "web".into()),
            None,
            "/repo".into(),
            "compose.yml".into(),
            DockerMutationKind::Start,
        )
        .expect("first Compose mutation dispatches");

    assert!(
        queue
            .enqueue(
                ("project-b".into(), "worker".into()),
                None,
                "/repo".into(),
                "compose.yml".into(),
                DockerMutationKind::Stop,
            )
            .is_none(),
        "the same Compose project must have one external mutation owner"
    );
    assert_eq!(queue.active[&first.scope()].pending.len(), 1);
}

#[test]
fn project_reload_invalidates_old_async_incarnation() {
    let mut lifecycles = ProjectLifecycles::default();
    let old = lifecycles.begin("project", "/repo");
    assert!(lifecycles.is_current("project", &old));

    let replacement = lifecycles.begin("project", "/repo");

    assert!(!lifecycles.is_current("project", &old));
    assert!(lifecycles.is_current("project", &replacement));
    assert_eq!(
        lifecycles.get("project", "/repo"),
        Some(replacement.clone())
    );
    assert_ne!(old, replacement);
}

#[test]
fn project_incarnation_rejects_reused_id_at_different_path() {
    let mut lifecycles = ProjectLifecycles::default();
    let old = lifecycles.begin("project", "/old");
    let replacement = lifecycles.begin("project", "/new");

    assert!(!lifecycles.is_current("project", &old));
    assert!(lifecycles.get("project", "/old").is_none());
    assert_eq!(lifecycles.get("project", "/new"), Some(replacement));
}

#[test]
fn project_unload_invalidates_delayed_callbacks() {
    let mut lifecycles = ProjectLifecycles::default();
    let loaded = lifecycles.begin("project", "/repo");

    lifecycles.invalidate("project");

    assert!(!lifecycles.is_current("project", &loaded));
    assert!(lifecycles.get("project", "/repo").is_none());
}

#[test]
fn malformed_reload_preserves_current_incarnation() {
    let project = ProjectDir::with_config("services: [");
    let path = project.path();
    let mut manager = manager();
    manager.project_paths.insert("project".into(), path.clone());
    let incarnation = manager.begin_project_incarnation("project", &path);
    let mut cx = RecordingCx::default();

    manager.reload_project_services("project", &path, &mut cx);

    assert_eq!(
        manager.project_incarnation("project", &path),
        Some(incarnation)
    );
    assert_eq!(cx.spawned.load(Ordering::Relaxed), 0);
}

#[test]
fn prepared_reload_does_not_probe_the_project_path() {
    let path = "/path/that/does/not/exist";
    let mut manager = manager();
    manager.project_paths.insert("project".into(), path.into());
    manager.begin_project_incarnation("project", path);
    let mut cx = RecordingCx::default();

    let status = manager.reload_project_services_prepared(
        "project",
        path,
        PreparedProjectConfig::Loaded {
            config: Some(OkenaProjectConfig {
                services: vec![ServiceDefinition {
                    name: "prepared".into(),
                    command: "echo prepared".into(),
                    cwd: ".".into(),
                    env: HashMap::new(),
                    auto_start: false,
                    restart_on_crash: false,
                    restart_delay_ms: 1000,
                }],
                docker_compose: None,
            }),
            detected_compose_file: None,
        },
        &mut cx,
    );

    assert_eq!(status, ServiceLoadStatus::Loaded);
    assert!(
        manager
            .instances()
            .contains_key(&("project".into(), "prepared".into()))
    );
}

#[test]
fn reload_replaces_docker_name_collision_with_okena_service() {
    let path = "/project";
    let key = ("project".to_string(), "web".to_string());
    let mut manager = manager();
    manager.project_paths.insert(key.0.clone(), path.into());
    manager.begin_project_incarnation(&key.0, path);
    let (_, docker) = make_docker_instance(&key.0, &key.1, ServiceStatus::Running);
    let old_terminal_id = docker.terminal_id.clone().expect("Docker log terminal");
    manager.instances.insert(key.clone(), docker);
    manager
        .terminal_to_service
        .insert(old_terminal_id.clone(), key.clone());
    let mut cx = RecordingCx::default();

    let status = manager.reload_project_services_prepared(
        &key.0,
        path,
        PreparedProjectConfig::Loaded {
            config: Some(OkenaProjectConfig {
                services: vec![ServiceDefinition {
                    name: key.1.clone(),
                    command: "bun run dev".into(),
                    cwd: ".".into(),
                    env: HashMap::new(),
                    auto_start: false,
                    restart_on_crash: false,
                    restart_delay_ms: 1000,
                }],
                docker_compose: None,
            }),
            detected_compose_file: None,
        },
        &mut cx,
    );

    assert_eq!(status, ServiceLoadStatus::Loaded);
    assert_eq!(manager.instances[&key].kind, ServiceKind::Okena);
    assert_eq!(manager.instances[&key].terminal_id, None);
    assert!(!manager.terminal_to_service.contains_key(&old_terminal_id));
}

#[test]
fn malformed_initial_load_reports_failure_without_claiming_path() {
    let project = ProjectDir::with_config("services: [");
    let path = project.path();
    let mut manager = manager();
    let mut cx = RecordingCx::default();

    let status = manager.load_project_services("project", &path, &HashMap::new(), &mut cx);

    assert_eq!(status, ServiceLoadStatus::Failed);
    assert_eq!(manager.project_path("project"), None);
    assert_eq!(manager.project_incarnation("project", &path), None);
    assert_eq!(cx.notifications.load(Ordering::Relaxed), 1);
}

#[test]
fn empty_loaded_project_publishes_an_explicit_empty_writeback() {
    let project = ProjectDir::with_config("services: []\n");
    let path = project.path();
    let mut manager = manager();
    let mut cx = RecordingCx::default();
    manager.set_project_writeback_owner("project", &path, 7);

    let status = manager.load_project_services("project", &path, &HashMap::new(), &mut cx);

    assert_eq!(status, ServiceLoadStatus::Loaded);
    assert_eq!(
        manager.service_terminal_writebacks(),
        vec![ServiceTerminalWriteback {
            project_id: "project".into(),
            project_path: path,
            data_replacement_epoch: 7,
            terminal_ids: HashMap::new(),
        }]
    );
    assert!(cx.notifications.load(Ordering::Relaxed) > 0);
}

#[test]
fn backend_migration_reload_does_not_restart_a_stopped_auto_start_service() {
    let project = ProjectDir::with_config(
        "services:\n  - name: web\n    command: echo web\n    auto_start: true\n",
    );
    let path = project.path();
    let mut manager = manager();
    let mut cx = RecordingCx::default();

    let status = manager.load_project_services_for_backend_migration("project", &path, &mut cx);

    assert_eq!(status, ServiceLoadStatus::Loaded);
    assert_eq!(
        manager
            .instances()
            .get(&("project".to_string(), "web".to_string()))
            .map(|instance| &instance.status),
        Some(&ServiceStatus::Stopped)
    );
    assert_eq!(cx.spawned.load(Ordering::Relaxed), 0);
}

#[test]
fn initial_okena_launch_applies_service_environment() {
    let path = "/project";
    let (service_manager, plans) = recording_manager();
    let executor = Rc::new(smol::LocalExecutor::new());
    let service_manager = Rc::new(RefCell::new(service_manager));
    let handle = ExecutingHandle {
        manager: Rc::downgrade(&service_manager),
        executor: executor.clone(),
        notifications: Arc::new(AtomicUsize::new(0)),
    };

    smol::block_on(executor.run(async {
        let mut cx = ExecutingCx { handle };
        service_manager.borrow_mut().load_project_services_prepared(
            "project",
            path,
            &HashMap::new(),
            PreparedProjectConfig::Loaded {
                config: Some(OkenaProjectConfig {
                    services: vec![ServiceDefinition {
                        name: "web".into(),
                        command: "bun run dev".into(),
                        cwd: ".".into(),
                        env: HashMap::from([
                            ("PORT".into(), "4100".into()),
                            ("NODE_ENV".into(), "development".into()),
                        ]),
                        auto_start: true,
                        restart_on_crash: false,
                        restart_delay_ms: 1000,
                    }],
                    docker_compose: None,
                }),
                detected_compose_file: None,
            },
            &mut cx,
        );

        let plan = plans.recv().await.expect("initial service launch plan");
        assert_eq!(
            plan.environment,
            vec![
                ("NODE_ENV".into(), "development".into()),
                ("PORT".into(), "4100".into()),
            ]
        );
    }));
}

#[test]
fn successful_reload_rearms_restart_and_port_detection() {
    let project = ProjectDir::with_config(
        "services:\n  - name: running\n    command: echo running\n  - name: restarting\n    command: echo restarting\n    restart_on_crash: true\n    restart_delay_ms: 25\n",
    );
    let path = project.path();
    let mut manager = manager();
    manager.project_paths.insert("project".into(), path.clone());
    let old_incarnation = manager.begin_project_incarnation("project", &path);
    let (running_key, running) =
        make_instance("project", "running", false, 0, ServiceStatus::Running);
    let (restarting_key, mut restarting) =
        make_instance("project", "restarting", true, 1, ServiceStatus::Restarting);
    restarting.terminal_id = None;
    manager.instances.insert(running_key.clone(), running);
    manager.instances.insert(restarting_key.clone(), restarting);
    let mut cx = RecordingCx::default();

    manager.reload_project_services("project", &path, &mut cx);

    let new_incarnation = manager
        .project_incarnation("project", &path)
        .expect("replacement incarnation");
    assert_ne!(new_incarnation, old_incarnation);
    assert_eq!(
        manager
            .port_detection_active
            .get(&running_key)
            .map(|state| &state.project_incarnation),
        Some(&new_incarnation)
    );
    assert_eq!(
        manager.instances[&restarting_key].status,
        ServiceStatus::Restarting
    );
    assert_eq!(cx.spawned.load(Ordering::Relaxed), 2);
}

#[test]
fn starting_launch_completes_after_same_project_reload() {
    let path = "/project";
    let definition = ServiceDefinition {
        name: "web".into(),
        command: "echo web".into(),
        cwd: ".".into(),
        env: HashMap::new(),
        auto_start: false,
        restart_on_crash: false,
        restart_delay_ms: 1000,
    };
    let prepared = || PreparedProjectConfig::Loaded {
        config: Some(OkenaProjectConfig {
            services: vec![definition.clone()],
            docker_compose: None,
        }),
        detected_compose_file: None,
    };
    let mut manager = manager();
    let mut cx = RecordingCx::default();
    manager.load_project_services_prepared("project", path, &HashMap::new(), prepared(), &mut cx);

    manager.start_service("project", "web", path, &mut cx);
    let key = ("project".to_string(), "web".to_string());
    let terminal_id = manager.instances[&key]
        .terminal_id
        .clone()
        .expect("pending terminal id");
    let launch_token = manager.pending_okena_launches[&key].clone();
    assert_eq!(manager.instances[&key].status, ServiceStatus::Starting);

    manager.reload_project_services_prepared("project", path, prepared(), &mut cx);

    assert_eq!(
        manager.pending_okena_launches.get(&key),
        Some(&launch_token)
    );
    assert!(manager.complete_okena_terminal_launch(
        &key,
        &launch_token,
        &terminal_id,
        path,
        &mut cx,
    ));
    assert_eq!(manager.instances[&key].status, ServiceStatus::Running);
    assert!(manager.terminals.lock().contains_key(&terminal_id));
    assert_eq!(manager.terminal_to_service.get(&terminal_id), Some(&key));
    assert!(!manager.pending_okena_launches.contains_key(&key));
}

#[test]
fn reload_drains_restart_while_pid_lookup_is_pending() {
    let path = "/project";
    let key = ("project".to_string(), "web".to_string());
    let old_terminal_id = "old-service-terminal".to_string();
    let (pty_manager, _events) = PtyManager::new(SessionBackend::None);
    let (pid_lookups_tx, pid_lookups_rx) = async_channel::unbounded();
    let (release_pid_lookup_tx, release_pid_lookup_rx) = async_channel::bounded(1);
    let (kills_tx, kills_rx) = async_channel::unbounded();
    let (plans_tx, plans_rx) = async_channel::unbounded();
    let backend = Arc::new(BarrierRestartBackend {
        local: LocalBackend::new(Arc::new(pty_manager)),
        pid_lookups: pid_lookups_tx,
        release_pid_lookup: release_pid_lookup_rx,
        kills: kills_tx,
        plans: plans_tx,
    });
    let terminals: okena_terminal::TerminalsRegistry = Arc::new(Default::default());
    let mut manager = ServiceManager::new(backend, terminals);
    manager.project_paths.insert(key.0.clone(), path.into());
    manager.begin_project_incarnation(&key.0, path);
    let (_, mut instance) = make_instance(&key.0, &key.1, false, 0, ServiceStatus::Running);
    instance.terminal_id = Some(old_terminal_id.clone());
    manager.instances.insert(key.clone(), instance);
    manager
        .terminal_to_service
        .insert(old_terminal_id.clone(), key.clone());

    let executor = Rc::new(smol::LocalExecutor::new());
    let service_manager = Rc::new(RefCell::new(manager));
    let handle = ExecutingHandle {
        manager: Rc::downgrade(&service_manager),
        executor: executor.clone(),
        notifications: Arc::new(AtomicUsize::new(0)),
    };

    smol::block_on(executor.run(async {
        let mut cx = ExecutingCx { handle };
        service_manager
            .borrow_mut()
            .restart_service(&key.0, &key.1, path, &mut cx);

        assert_eq!(pid_lookups_rx.recv().await, Ok(old_terminal_id.clone()));
        assert!(
            service_manager
                .borrow()
                .pending_okena_restarts
                .contains_key(&key)
        );
        assert!(
            !service_manager
                .borrow()
                .terminal_to_service
                .contains_key(&old_terminal_id),
            "the pending restart ledger must own the old mapping"
        );

        service_manager
            .borrow_mut()
            .reload_project_services_prepared(
                &key.0,
                path,
                PreparedProjectConfig::Loaded {
                    config: Some(OkenaProjectConfig {
                        services: vec![ServiceDefinition {
                            name: key.1.clone(),
                            command: "echo web".into(),
                            cwd: ".".into(),
                            env: HashMap::new(),
                            auto_start: false,
                            restart_on_crash: false,
                            restart_delay_ms: 60_000,
                        }],
                        docker_compose: None,
                    }),
                    detected_compose_file: None,
                },
                &mut cx,
            );

        assert_eq!(kills_rx.recv().await, Ok(old_terminal_id.clone()));
        assert!(
            !service_manager
                .borrow()
                .pending_okena_restarts
                .contains_key(&key)
        );

        release_pid_lookup_tx
            .send(())
            .await
            .expect("release pending PID lookup");
        smol::Timer::after(Duration::from_millis(5)).await;
        assert!(
            kills_rx.try_recv().is_err(),
            "the stale callback must not kill a replacement reusing the drained ID"
        );
        assert!(
            plans_rx.try_recv().is_err(),
            "the stale manual restart must not launch a duplicate service"
        );
    }));
}

#[test]
fn replacement_launch_rejects_completion_from_reused_terminal_id() {
    let path = "/project";
    let mut manager = manager();
    let mut cx = RecordingCx::default();
    manager.project_paths.insert("project".into(), path.into());
    manager.begin_project_incarnation("project", path);
    let (key, mut instance) = make_instance("project", "web", false, 0, ServiceStatus::Stopped);
    instance.terminal_id = None;
    manager.instances.insert(key.clone(), instance);

    manager.begin_okena_terminal_launch(
        "project",
        "web",
        path,
        "reused-terminal".into(),
        OkenaLaunchFailure::Crashed,
        &mut cx,
    );
    let stale_token = manager.pending_okena_launches[&key].clone();
    manager.begin_okena_terminal_launch(
        "project",
        "web",
        path,
        "reused-terminal".into(),
        OkenaLaunchFailure::Crashed,
        &mut cx,
    );
    let current_token = manager.pending_okena_launches[&key].clone();

    assert_ne!(stale_token, current_token);
    assert!(!manager.complete_okena_terminal_launch(
        &key,
        &stale_token,
        "reused-terminal",
        path,
        &mut cx,
    ));
    assert!(manager.complete_okena_terminal_launch(
        &key,
        &current_token,
        "reused-terminal",
        path,
        &mut cx,
    ));
    assert_eq!(manager.instances[&key].status, ServiceStatus::Running);
}

#[test]
fn project_path_update_reloads_runtime_under_new_path() {
    let project = ProjectDir::with_config("services:\n  - name: web\n    command: echo web\n");
    let new_path = project.path();
    let mut manager = manager();
    manager
        .project_paths
        .insert("project".into(), "/old/project/path".into());
    let old_incarnation = manager.begin_project_incarnation("project", "/old/project/path");
    let (key, instance) = make_instance("project", "web", false, 0, ServiceStatus::Running);
    manager.instances.insert(key.clone(), instance);
    let mut cx = RecordingCx::default();

    assert!(manager.update_project_path("project", &new_path, &mut cx));

    let new_incarnation = manager
        .project_incarnation("project", &new_path)
        .expect("new-path incarnation");
    assert_ne!(new_incarnation, old_incarnation);
    assert_eq!(manager.project_path("project"), Some(&new_path));
    assert_eq!(
        manager
            .port_detection_active
            .get(&key)
            .map(|state| &state.project_incarnation),
        Some(&new_incarnation)
    );
    assert_eq!(cx.spawned.load(Ordering::Relaxed), 1);
}

#[test]
fn project_path_update_retries_after_parse_failure() {
    let project = ProjectDir::with_config("services: [");
    let new_path = project.path();
    let mut manager = manager();
    manager
        .project_paths
        .insert("project".into(), "/old/project/path".into());
    manager.begin_project_incarnation("project", "/old/project/path");
    let mut cx = RecordingCx::default();

    assert!(!manager.update_project_path("project", &new_path, &mut cx));
    assert_eq!(
        manager.project_path("project").map(String::as_str),
        Some("/old/project/path")
    );

    project.write_config("services: []\n");
    assert!(manager.update_project_path("project", &new_path, &mut cx));
    assert_eq!(manager.project_path("project"), Some(&new_path));
}
