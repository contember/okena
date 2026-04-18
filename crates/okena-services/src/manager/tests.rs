use super::*;
use crate::config::ServiceDefinition;
use std::collections::HashMap;

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
