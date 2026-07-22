use super::*;
use crate::config::ServiceDefinition;
use okena_terminal::backend::LocalBackend;
use okena_terminal::pty_manager::PtyManager;
use okena_terminal::session_backend::SessionBackend;
use std::collections::HashMap;
use std::future::{Future, ready};
use std::path::PathBuf;
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
