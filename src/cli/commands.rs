use crate::cli::{api_get, api_post, discover_server, ensure_token};
use crate::remote::auth::{generate_pairing_code, pair_code_path};
use okena_core::api::StateResponse;

/// Check if `--json` flag is present in args.
fn has_json_flag(args: &[String]) -> bool {
    args.iter().any(|a| a == "--json")
}

/// Filter out known flags from args, returning only positional args.
fn positional_args(args: &[String]) -> Vec<&str> {
    args.iter()
        .filter(|a| !a.starts_with("--"))
        .map(|s| s.as_str())
        .collect()
}

pub fn cli_pair() -> i32 {
    let code = generate_pairing_code();
    let path = pair_code_path();

    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("Failed to create config directory: {e}");
            return 1;
        }
    }

    if let Err(e) = std::fs::write(&path, &code) {
        eprintln!("Failed to write pairing code: {e}");
        return 1;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            eprintln!("Warning: failed to set file permissions: {e}");
        }
    }

    println!("{code}");
    eprintln!("Expires in 60s — run `okena pair` again for a fresh code.");
    0
}

pub fn cli_health(args: &[String]) -> i32 {
    let json_mode = has_json_flag(args);

    let (host, port) = match discover_server() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    let url = format!("http://{}:{}/health", host, port);
    let resp = match reqwest::blocking::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to connect: {e}");
            return 1;
        }
    };

    let body = match resp.text() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to read response: {e}");
            return 1;
        }
    };

    if json_mode {
        println!("{body}");
    } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
        // tab-separated: status version uptime
        println!(
            "{}\t{}\t{}",
            v.get("status").and_then(|s| s.as_str()).unwrap_or("unknown"),
            v.get("version").and_then(|s| s.as_str()).unwrap_or("unknown"),
            v.get("uptime_secs").and_then(|s| s.as_u64()).unwrap_or(0),
        );
    } else {
        println!("{body}");
    }
    0
}

pub fn cli_state() -> i32 {
    // state always outputs JSON (it's the raw API response)
    let token = match ensure_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    match api_get("/v1/state", &token) {
        Ok(body) => {
            println!("{body}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

pub fn cli_action(json: Option<&str>) -> i32 {
    let json = match json {
        Some(j) => j,
        None => {
            eprintln!("Usage: okena action '<json>'");
            return 1;
        }
    };

    if serde_json::from_str::<serde_json::Value>(json).is_err() {
        eprintln!("Invalid JSON: {json}");
        return 1;
    }

    let token = match ensure_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    match api_post("/v1/actions", &token, json) {
        Ok(body) => {
            if !body.is_empty() {
                println!("{body}");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// `okena services [--json] [project]`
///
/// Default: tab-separated lines: project_name \t service_name \t status \t kind \t ports
/// --json: array of objects
pub fn cli_services(args: &[String]) -> i32 {
    let json_mode = has_json_flag(args);
    let pos = positional_args(args);
    let project_filter = pos.first().copied();

    let token = match ensure_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    let state = match fetch_state(&token) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    let projects = match project_filter {
        Some(filter) => {
            let matched: Vec<_> = state
                .projects
                .iter()
                .filter(|p| p.id == filter || p.name.eq_ignore_ascii_case(filter))
                .collect();
            if matched.is_empty() {
                eprintln!("Project not found: {filter}");
                eprintln!(
                    "Available: {}",
                    state
                        .projects
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                return 1;
            }
            matched
        }
        None => state.projects.iter().collect(),
    };

    if json_mode {
        let mut entries = Vec::new();
        for project in &projects {
            for svc in &project.services {
                entries.push(serde_json::json!({
                    "project": project.name,
                    "project_id": project.id,
                    "name": svc.name,
                    "status": svc.status,
                    "kind": svc.kind,
                    "ports": svc.ports,
                    "exit_code": svc.exit_code,
                }));
            }
        }
        #[allow(
            clippy::unwrap_used,
            reason = "entries is a Vec of serde_json::Value — serialization is infallible"
        )]
        let out = serde_json::to_string_pretty(&entries).unwrap();
        println!("{}", out);
    } else {
        for project in &projects {
            for svc in &project.services {
                let ports = svc
                    .ports
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                // project \t name \t status \t kind \t ports
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    project.name, svc.name, svc.status, svc.kind, ports
                );
            }
        }
    }

    0
}

/// `okena service <start|stop|restart> <name> [project] [--json]`
///
/// Sends the action and waits for the service to reach the target status.
/// For start/restart, also waits for port detection to stabilize.
///
/// Default output: name \t status \t ports
/// --json: object with name, status, kind, ports
pub fn cli_service(args: &[String]) -> i32 {
    let json_mode = has_json_flag(args);
    let pos = positional_args(args);
    if pos.len() < 2 {
        eprintln!("Usage: okena service <start|stop|restart> <name> [project] [--json]");
        return 1;
    }

    let verb = pos[0];
    let service_name = pos[1];
    let project_filter = pos.get(2).copied();

    let (action, target_statuses): (&str, &[&str]) = match verb {
        "start" => ("start_service", &["running"]),
        "stop" => ("stop_service", &["stopped"]),
        "restart" => ("restart_service", &["running"]),
        _ => {
            eprintln!("Unknown service action: {verb}");
            eprintln!("Use: start, stop, restart");
            return 1;
        }
    };

    let token = match ensure_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    let project_id = match resolve_project_id(&token, project_filter) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    // Send the action
    let body = serde_json::json!({
        "action": action,
        "project_id": project_id,
        "service_name": service_name,
    });

    if let Err(e) = api_post("/v1/actions", &token, &body.to_string()) {
        eprintln!("{e}");
        return 1;
    }

    // Poll until target status is reached
    let wait_for_ports = target_statuses.contains(&"running");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut reached_target = false;
    let mut last_ports: Vec<u16> = Vec::new();
    let mut ports_stable_count = 0u32;

    loop {
        if std::time::Instant::now() > deadline {
            eprintln!("Timeout waiting for service to reach target status.");
            return 1;
        }

        std::thread::sleep(std::time::Duration::from_secs(1));

        let state = match fetch_state(&token) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let svc = state
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .and_then(|p| p.services.iter().find(|s| s.name == service_name));

        let Some(svc) = svc else { continue };

        // Check for failure
        if svc.status == "crashed" {
            let exit_info = svc
                .exit_code
                .map(|c| format!(" (exit code {})", c))
                .unwrap_or_default();
            eprintln!("Service crashed{exit_info}.");
            return 1;
        }

        if target_statuses.contains(&svc.status.as_str()) {
            reached_target = true;

            if !wait_for_ports {
                // For stop: done immediately
                print_service_result(svc, json_mode);
                return 0;
            }

            // For start/restart: wait for ports to stabilize
            if svc.ports == last_ports {
                ports_stable_count += 1;
            } else {
                last_ports = svc.ports.clone();
                ports_stable_count = 0;
            }

            // Ports stable for 2 consecutive checks, or 5s after running with no ports
            if ports_stable_count >= 2 {
                print_service_result(svc, json_mode);
                return 0;
            }
        } else if reached_target {
            // Was running but status changed (e.g. crashed after start)
            eprintln!("Service status changed unexpectedly to '{}'.", svc.status);
            return 1;
        }
    }
}

fn print_service_result(svc: &okena_core::api::ApiServiceInfo, json_mode: bool) {
    if json_mode {
        println!(
            "{}",
            serde_json::json!({
                "name": svc.name,
                "status": svc.status,
                "kind": svc.kind,
                "ports": svc.ports,
                "exit_code": svc.exit_code,
            })
        );
    } else {
        let ports = svc
            .ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",");
        println!("{}\t{}\t{}", svc.name, svc.status, ports);
    }
}

/// `okena whoami [--json]`
///
/// Default: tab-separated: terminal_id \t project_id \t project_name \t project_path
/// --json: object
pub fn cli_whoami(args: &[String]) -> i32 {
    let json_mode = has_json_flag(args);

    let terminal_id = match std::env::var("OKENA_TERMINAL_ID") {
        Ok(id) => id,
        Err(_) => {
            eprintln!("Not running inside an Okena terminal (OKENA_TERMINAL_ID not set).");
            return 1;
        }
    };

    let token = match ensure_token() {
        Ok(t) => t,
        Err(e) => {
            if json_mode {
                println!(
                    "{}",
                    serde_json::json!({ "terminal_id": terminal_id })
                );
            } else {
                println!("{terminal_id}");
            }
            eprintln!("Warning: could not reach Okena server: {e}");
            return 0;
        }
    };

    match fetch_state(&token) {
        Ok(state) => {
            for project in &state.projects {
                if has_terminal_id(&project.layout, &terminal_id)
                    || project.terminal_names.contains_key(&terminal_id)
                    || project
                        .services
                        .iter()
                        .any(|s| s.terminal_id.as_deref() == Some(&terminal_id))
                {
                    if json_mode {
                        println!(
                            "{}",
                            serde_json::json!({
                                "terminal_id": terminal_id,
                                "project_id": project.id,
                                "project_name": project.name,
                                "project_path": project.path,
                            })
                        );
                    } else {
                        println!(
                            "{}\t{}\t{}\t{}",
                            terminal_id, project.id, project.name, project.path
                        );
                    }
                    return 0;
                }
            }

            // Terminal not found in any project
            if json_mode {
                println!(
                    "{}",
                    serde_json::json!({ "terminal_id": terminal_id })
                );
            } else {
                println!("{terminal_id}");
            }
            0
        }
        Err(e) => {
            if json_mode {
                println!(
                    "{}",
                    serde_json::json!({ "terminal_id": terminal_id })
                );
            } else {
                println!("{terminal_id}");
            }
            eprintln!("Warning: could not fetch state: {e}");
            0
        }
    }
}

/// Check if a layout tree contains a terminal with the given ID.
fn has_terminal_id(node: &Option<okena_core::api::ApiLayoutNode>, terminal_id: &str) -> bool {
    use okena_core::api::ApiLayoutNode;
    match node {
        None => false,
        Some(ApiLayoutNode::Terminal {
            terminal_id: Some(id),
            ..
        }) => id == terminal_id,
        Some(ApiLayoutNode::Terminal { .. }) => false,
        Some(ApiLayoutNode::Split { children, .. })
        | Some(ApiLayoutNode::Tabs { children, .. }) => children
            .iter()
            .any(|c| has_terminal_id(&Some(c.clone()), terminal_id)),
    }
}

fn fetch_state(token: &str) -> Result<StateResponse, String> {
    let body = api_get("/v1/state", token)?;
    serde_json::from_str(&body).map_err(|e| format!("Failed to parse state: {e}"))
}

/// Resolve a project ID from a name/id filter, or pick the only project if unambiguous.
fn resolve_project_id(token: &str, filter: Option<&str>) -> Result<String, String> {
    let state = fetch_state(token)?;

    match filter {
        Some(f) => {
            for p in &state.projects {
                if p.id == f || p.name.eq_ignore_ascii_case(f) {
                    return Ok(p.id.clone());
                }
            }
            Err(format!(
                "Project not found: {f}\nAvailable: {}",
                state
                    .projects
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
        None => {
            if state.projects.len() == 1 {
                Ok(state.projects[0].id.clone())
            } else if let Some(id) = &state.focused_project_id {
                Ok(id.clone())
            } else {
                Err(format!(
                    "Multiple projects — specify which one: {}",
                    state
                        .projects
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }
}
