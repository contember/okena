use crate::cli::resolve;
use crate::cli::{api_get, api_post, discover_server, ensure_token};
use crate::remote::auth::{generate_pairing_code, pair_code_path};
use okena_core::api::{ApiProject, StateResponse};

pub fn cli_pair() -> i32 {
    let code = generate_pairing_code();
    let path = pair_code_path();

    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("Failed to create config directory: {e}");
            return 1;
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

    // Show the TLS cert fingerprint (if the server has a persisted cert) so the
    // host can read it out for the connecting client to verify out-of-band.
    // Goes to stderr to keep stdout a clean, pipeable pairing code.
    if let Some(fp) =
        crate::remote::tls::read_fingerprint(&crate::workspace::persistence::config_dir())
    {
        eprintln!(
            "TLS certificate fingerprint (SHA-256) — verify it matches the connecting client:"
        );
        eprintln!("  {}", okena_core::client::tls::format_fingerprint(&fp));
    }
    eprintln!("Expires in 60s — run `okena pair` again for a fresh code.");
    0
}

pub fn cli_health(json_mode: bool) -> i32 {
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

pub fn cli_action(json: &str) -> i32 {
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
pub fn cli_services(project_filter: Option<&str>, json_mode: bool) -> i32 {
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
pub fn cli_service(
    verb: &str,
    service_name: &str,
    project_filter: Option<&str>,
    json_mode: bool,
) -> i32 {
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

    let state = match fetch_state(&token) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    let project_id = match resolve_project_id_in_state(&state, project_filter) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    // Fail fast on an unknown service name instead of POSTing the action and
    // polling for up to 30s on a status that will never appear.
    if let Some(project) = state.projects.iter().find(|p| p.id == project_id)
        && !project.services.iter().any(|s| s.name == service_name)
    {
        eprintln!("No service named '{service_name}' in project '{}'.", project.name);
        let available: Vec<&str> = project.services.iter().map(|s| s.name.as_str()).collect();
        if available.is_empty() {
            eprintln!("That project has no services.");
        } else {
            eprintln!("Available: {}", available.join(", "));
        }
        return 1;
    }

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
pub fn cli_whoami(json_mode: bool) -> i32 {
    let terminal_id = match std::env::var("OKENA_TERMINAL_ID") {
        Ok(id) => id,
        Err(_) => {
            let msg = "Not running inside an Okena terminal (OKENA_TERMINAL_ID not set).";
            if json_mode {
                eprintln!("{}", serde_json::json!({ "error": msg }));
            } else {
                eprintln!("{msg}");
            }
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

pub(crate) fn fetch_state(token: &str) -> Result<StateResponse, String> {
    let body = api_get("/v1/state", token)?;
    serde_json::from_str(&body).map_err(|e| format!("Failed to parse state: {e}"))
}

/// Resolve a project ID from a name/id filter, or pick the only project if
/// unambiguous. Operates over an already-fetched state (no I/O) so callers that
/// need the state for follow-up validation don't fetch it twice.
fn resolve_project_id_in_state(
    state: &StateResponse,
    filter: Option<&str>,
) -> Result<String, String> {
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

// ── Shared helpers for the new command surface ───────────────────────────────

/// Fetch state and run a closure that builds an action JSON body, then POST it.
/// `build` receives the parsed state so it can resolve filters → ids/paths.
/// Returns the (possibly empty) response body on success.
fn with_state_post<F>(build: F) -> i32
where
    F: FnOnce(&StateResponse) -> Result<serde_json::Value, String>,
{
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
    let body = match build(&state) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    post_action(&token, &body)
}

/// POST an action body and print any non-empty response on stdout.
fn post_action(token: &str, body: &serde_json::Value) -> i32 {
    match api_post("/v1/actions", token, &body.to_string()) {
        Ok(resp) => {
            if !resp.trim().is_empty() {
                println!("{}", resp.trim());
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// POST an action and parse the JSON response, printing selected id field(s).
/// `id_fields` are response keys to print (each on its own line) — used for
/// commands that return new ids (e.g. `project_id`, `terminal_ids`).
fn post_action_print_ids(token: &str, body: &serde_json::Value, id_fields: &[&str]) -> i32 {
    match api_post("/v1/actions", token, &body.to_string()) {
        Ok(resp) => {
            print_response_ids(&resp, id_fields);
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// Print the requested id field(s) from an action response body. Handles both
/// scalar string fields and arrays of strings (e.g. `terminal_ids`).
fn print_response_ids(resp: &str, id_fields: &[&str]) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(resp) else {
        // No JSON body — print raw if any.
        if !resp.trim().is_empty() {
            println!("{}", resp.trim());
        }
        return;
    };
    for field in id_fields {
        match v.get(field) {
            Some(serde_json::Value::String(s)) => println!("{s}"),
            Some(serde_json::Value::Array(arr)) => {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        println!("{s}");
                    }
                }
            }
            _ => {}
        }
    }
}

/// Resolve a folder by exact id or case-insensitive name.
fn resolve_folder_id(state: &StateResponse, filter: &str) -> Result<String, String> {
    if let Some(f) = state.folders.iter().find(|f| f.id == filter) {
        return Ok(f.id.clone());
    }
    if let Some(f) = state
        .folders
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(filter))
    {
        return Ok(f.id.clone());
    }
    let available = if state.folders.is_empty() {
        "(none)".to_string()
    } else {
        state
            .folders
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(format!("Folder not found: {filter}\nFolders: {available}"))
}

/// Validate a color against FolderColor's serde names (all lowercase).
fn validate_color(color: &str) -> Result<String, String> {
    const COLORS: &[&str] = &[
        "default", "red", "orange", "yellow", "lime", "green", "teal", "cyan", "blue", "indigo",
        "purple", "pink",
    ];
    let lc = color.to_ascii_lowercase();
    if COLORS.contains(&lc.as_str()) {
        Ok(lc)
    } else {
        Err(format!(
            "Invalid color: {color}\nValid colors: {}",
            COLORS.join(", ")
        ))
    }
}

/// Map a user key string to a serialized `SpecialKey` (the JSON the action body
/// carries). Named keys serialize to a bare string (e.g. `"Enter"`); a generic
/// `ctrl-<letter>` chord serializes to `{"Ctrl":"l"}`. Supports the canonical
/// names plus friendly aliases.
fn map_special_key(key: &str) -> Result<serde_json::Value, String> {
    let k = key.to_ascii_lowercase().replace(['-', '_', ' '], "");
    let named = match k.as_str() {
        "enter" | "return" | "cr" => Some("Enter"),
        "escape" | "esc" => Some("Escape"),
        "ctrlc" => Some("CtrlC"),
        "ctrld" => Some("CtrlD"),
        "ctrlz" => Some("CtrlZ"),
        "tab" => Some("Tab"),
        "arrowup" | "up" => Some("ArrowUp"),
        "arrowdown" | "down" => Some("ArrowDown"),
        "arrowleft" | "left" => Some("ArrowLeft"),
        "arrowright" | "right" => Some("ArrowRight"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "pageup" | "pgup" => Some("PageUp"),
        "pagedown" | "pgdn" => Some("PageDown"),
        "backspace" | "bs" => Some("Backspace"),
        "delete" | "del" => Some("Delete"),
        _ => None,
    };
    if let Some(name) = named {
        return Ok(serde_json::Value::String(name.to_string()));
    }

    // Generic ctrl-<letter> chord (e.g. ctrl-l, ctrl-a, ctrl-u). Separators were
    // already stripped above, so "ctrl-l" arrives here as "ctrll".
    if let Some(rest) = k.strip_prefix("ctrl")
        && rest.chars().count() == 1
        && let Some(c) = rest.chars().next()
        && c.is_ascii_alphabetic()
    {
        return Ok(serde_json::json!({ "Ctrl": c }));
    }

    Err(format!(
        "Unknown key: {key}\nValid keys: enter, esc, tab, up, down, left, right, home, end, pageup, pagedown, backspace, delete, ctrl-<a-z> (e.g. ctrl-c, ctrl-l, ctrl-u)"
    ))
}

/// Parse a split/direction argument ("h"/"v" or full names).
fn parse_direction(dir: &str) -> Result<&'static str, String> {
    match dir.to_ascii_lowercase().as_str() {
        "h" | "horizontal" => Ok("horizontal"),
        "v" | "vertical" => Ok("vertical"),
        _ => Err(format!("Invalid direction: {dir} (use h or v)")),
    }
}

// ── Orientation ──────────────────────────────────────────────────────────────

/// `okena ls [--json]`
pub fn cli_ls(json_mode: bool) -> i32 {
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
    if json_mode {
        // Structured form of the text overview: windows with their visible
        // projects (ids resolved to names) + focus, and per-project the
        // overview-relevant fields (hidden, git, terminals, layout tree).
        // (For the full raw dump use `okena state`.)
        let name_for = |pid: &str| {
            state
                .projects
                .iter()
                .find(|p| p.id == pid)
                .map(|p| p.name.clone())
        };
        let windows: Vec<serde_json::Value> = state
            .windows
            .iter()
            .map(|w| {
                let visible: Vec<serde_json::Value> = w
                    .visible_project_ids
                    .iter()
                    .map(|pid| serde_json::json!({ "id": pid, "name": name_for(pid) }))
                    .collect();
                serde_json::json!({
                    "id": w.id,
                    "kind": w.kind,
                    "active": w.active,
                    "focused_project_id": w.focused_project_id,
                    "focused_terminal_id": w.focused_terminal_id,
                    "fullscreen": w.fullscreen,
                    "visible_projects": visible,
                })
            })
            .collect();
        let projects: Vec<serde_json::Value> = state
            .projects
            .iter()
            .map(|p| {
                let terminals: Vec<String> = resolve::project_terminals(p)
                    .iter()
                    .map(|t| t.terminal_id.clone())
                    .collect();
                serde_json::json!({
                    "id": p.id,
                    "name": p.name,
                    "path": p.path,
                    "hidden": !p.show_in_overview,
                    "git": p.git_status,
                    "terminals": terminals,
                    "layout": p.layout,
                })
            })
            .collect();
        let out = serde_json::json!({ "windows": windows, "projects": projects });
        match serde_json::to_string_pretty(&out) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("Failed to serialize: {e}");
                return 1;
            }
        }
        return 0;
    }
    render_ls(&state);
    0
}

/// Resolve a project id to its display name, falling back to the id.
fn project_name_for(state: &StateResponse, id: &str) -> String {
    state
        .projects
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| id.to_string())
}

/// Resolve a project id + terminal id to "<proj>/<term-name>".
fn term_label(state: &StateResponse, project_id: &str, terminal_id: &str) -> String {
    let proj = state.projects.iter().find(|p| p.id == project_id);
    let pname = proj.map(|p| p.name.as_str()).unwrap_or(project_id);
    let tname = proj
        .and_then(|p| p.terminal_names.get(terminal_id))
        .map(|s| s.as_str())
        .unwrap_or(terminal_id);
    format!("{pname}/{tname}")
}

/// Shorten a window id for display: "main" stays, UUIDs are truncated to 8.
fn short_window_id(id: &str) -> String {
    if id == "main" {
        id.to_string()
    } else {
        id.chars().take(8).collect()
    }
}

/// Render the human-readable `ls` overview (windows + projects + layout tree).
fn render_ls(state: &StateResponse) {
    if !state.windows.is_empty() {
        println!("WINDOWS");
        for w in &state.windows {
            let marker = if w.active { "*" } else { "" };
            let id = short_window_id(&w.id);
            let mut parts: Vec<String> = Vec::new();
            if let (Some(pid), Some(tid)) = (&w.focused_project_id, &w.focused_terminal_id) {
                parts.push(format!("focus {}", term_label(state, pid, tid)));
            } else if let Some(pid) = &w.focused_project_id {
                parts.push(format!("focus {}", project_name_for(state, pid)));
            }
            if !w.visible_project_ids.is_empty() {
                let names: Vec<String> = w
                    .visible_project_ids
                    .iter()
                    .map(|pid| project_name_for(state, pid))
                    .collect();
                parts.push(format!("visible: {}", names.join(", ")));
            }
            if let Some(fs) = &w.fullscreen {
                parts.push(format!(
                    "[fullscreen {}]",
                    term_label(state, &fs.project_id, &fs.terminal_id)
                ));
            }
            println!("  {id}{marker}\t{}", parts.join("   "));
        }
    }

    println!("PROJECTS");
    for p in &state.projects {
        let mut header = format!("  {}\t{}", p.name, p.path);
        if let Some(git) = &p.git_status
            && let Some(branch) = &git.branch
        {
            header.push_str(&format!(
                "\t[{} +{} -{}]",
                branch, git.lines_added, git.lines_removed
            ));
        }
        if !p.show_in_overview {
            header.push_str(" (hidden)");
        }
        println!("{header}");
        if let Some(layout) = &p.layout {
            render_layout(layout, p, 2);
        }
    }
}

/// Recursively render a project's layout tree with indentation.
fn render_layout(node: &okena_core::api::ApiLayoutNode, project: &ApiProject, depth: usize) {
    use okena_core::api::ApiLayoutNode;
    let indent = "  ".repeat(depth);
    match node {
        ApiLayoutNode::Terminal {
            terminal_id,
            minimized,
            detached,
        } => {
            let id = terminal_id.as_deref().unwrap_or("(empty)");
            let short = if id.len() > 8 { &id[..8] } else { id };
            let name = terminal_id
                .as_ref()
                .and_then(|tid| project.terminal_names.get(tid))
                .map(|s| s.as_str())
                .unwrap_or("");
            let mut flags = Vec::new();
            if *minimized {
                flags.push("minimized");
            }
            if *detached {
                flags.push("detached");
            }
            let flag_str = if flags.is_empty() {
                String::new()
            } else {
                format!(" ({})", flags.join(", "))
            };
            println!("{indent}term {short} {name}{flag_str}");
        }
        ApiLayoutNode::Split {
            direction,
            children,
            ..
        } => {
            let d = match direction {
                okena_core::types::SplitDirection::Horizontal => "h",
                okena_core::types::SplitDirection::Vertical => "v",
            };
            println!("{indent}split({d})");
            for child in children {
                render_layout(child, project, depth + 1);
            }
        }
        ApiLayoutNode::Tabs {
            children,
            active_tab,
        } => {
            println!("{indent}tabs");
            for (i, child) in children.iter().enumerate() {
                let active = if i == *active_tab { "*" } else { " " };
                print!("{}{active}", "  ".repeat(depth + 1));
                // Render the tab child inline (its own indent absorbed by prefix).
                render_layout_inline(child, project, depth + 1);
            }
        }
    }
}

/// Render a tabs child after the active-marker prefix has been printed.
fn render_layout_inline(node: &okena_core::api::ApiLayoutNode, project: &ApiProject, depth: usize) {
    use okena_core::api::ApiLayoutNode;
    match node {
        ApiLayoutNode::Terminal { .. } => {
            // Re-render the terminal line without its leading indent.
            let mut buf = Vec::new();
            render_terminal_line(node, project, &mut buf);
            println!("{}", String::from_utf8_lossy(&buf).trim_start());
        }
        _ => {
            println!();
            render_layout(node, project, depth + 1);
        }
    }
}

/// Helper: format a Terminal node line into a buffer (used by inline tab render).
fn render_terminal_line(
    node: &okena_core::api::ApiLayoutNode,
    project: &ApiProject,
    buf: &mut Vec<u8>,
) {
    use okena_core::api::ApiLayoutNode;
    use std::io::Write as _;
    if let ApiLayoutNode::Terminal {
        terminal_id,
        minimized,
        detached,
    } = node
    {
        let id = terminal_id.as_deref().unwrap_or("(empty)");
        let short = if id.len() > 8 { &id[..8] } else { id };
        let name = terminal_id
            .as_ref()
            .and_then(|tid| project.terminal_names.get(tid))
            .map(|s| s.as_str())
            .unwrap_or("");
        let mut flags = Vec::new();
        if *minimized {
            flags.push("minimized");
        }
        if *detached {
            flags.push("detached");
        }
        let flag_str = if flags.is_empty() {
            String::new()
        } else {
            format!(" ({})", flags.join(", "))
        };
        let _ = write!(buf, "term {short} {name}{flag_str}");
    }
}

/// `okena term ls [project] [--json]`
pub fn cli_term_ls(project_filter: Option<&str>, json_mode: bool) -> i32 {
    with_state_read(|state| {
        let projects: Vec<&ApiProject> = match project_filter {
            Some(f) => vec![resolve::resolve_project(state, f)?],
            None => state.projects.iter().collect(),
        };
        if json_mode {
            let mut entries = Vec::new();
            for p in &projects {
                for t in resolve::project_terminals(p) {
                    entries.push(serde_json::json!({
                        "terminal_id": t.terminal_id,
                        "name": t.name,
                        "project_id": p.id,
                        "project_name": p.name,
                    }));
                }
            }
            #[allow(
                clippy::unwrap_used,
                reason = "entries is a Vec of serde_json::Value — serialization is infallible"
            )]
            let out = serde_json::to_string_pretty(&entries).unwrap();
            println!("{out}");
        } else {
            for p in &projects {
                for t in resolve::project_terminals(p) {
                    // terminal_id \t name \t project_name
                    println!("{}\t{}\t{}", t.terminal_id, t.name, p.name);
                }
            }
        }
        Ok(())
    })
}

/// Fetch state and run a read-only closure (for listing commands).
fn with_state_read<F>(f: F) -> i32
where
    F: FnOnce(&StateResponse) -> Result<(), String>,
{
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
    match f(&state) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

// ── Projects ─────────────────────────────────────────────────────────────────

/// `okena project add <path> [--name <n>] [--hidden] [--folder <f>]`
pub fn cli_project_add(
    path: &str,
    name: Option<&str>,
    hidden: bool,
    folder: Option<&str>,
    window: Option<&str>,
) -> i32 {
    // Canonicalize the path to an absolute path (resolve relative to CWD).
    let abs = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Cannot resolve path '{path}': {e}");
            return 1;
        }
    };
    let abs_str = abs.to_string_lossy().into_owned();
    let project_name = name.map(|s| s.to_string()).unwrap_or_else(|| {
        abs.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| abs_str.clone())
    });

    let token = match ensure_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    let body = serde_json::json!({
        "action": "add_project",
        "name": project_name,
        "path": abs_str,
    });
    let resp = match api_post("/v1/actions", &token, &body.to_string()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap_or(serde_json::Value::Null);
    let project_id = match v.get("project_id").and_then(|x| x.as_str()) {
        Some(id) => id.to_string(),
        None => {
            eprintln!("add_project did not return a project_id.\n{resp}");
            return 1;
        }
    };
    println!("{project_id}");

    // Follow-up: hide.
    if hidden {
        let mut hide_body = serde_json::json!({
            "action": "set_project_show_in_overview",
            "project_id": project_id,
            "show": false,
        });
        if let Some(w) = window
            && let Err(e) = apply_window(&mut hide_body, &token, w)
        {
            eprintln!("{e}");
            return 1;
        }
        if let Err(e) = api_post("/v1/actions", &token, &hide_body.to_string()) {
            eprintln!("Warning: failed to hide project: {e}");
            return 1;
        }
    }

    // Follow-up: move into a folder.
    if let Some(folder_filter) = folder {
        let state = match fetch_state(&token) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Warning: could not resolve folder: {e}");
                return 1;
            }
        };
        let folder_id = match resolve_folder_id(&state, folder_filter) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        };
        let move_body = serde_json::json!({
            "action": "move_project_to_folder",
            "project_id": project_id,
            "folder_id": folder_id,
        });
        if let Err(e) = api_post("/v1/actions", &token, &move_body.to_string()) {
            eprintln!("Warning: failed to move project into folder: {e}");
            return 1;
        }
    }

    0
}

/// Resolve a `--window` filter against state and inject it into `body`.
/// Returns `Err` with a message on failure so the caller can abort instead of
/// silently landing the action on the focused window.
fn apply_window(body: &mut serde_json::Value, token: &str, window: &str) -> Result<(), String> {
    let win = fetch_state(token).and_then(|s| resolve::resolve_window(&s, window))?;
    body["window"] = serde_json::Value::String(win);
    Ok(())
}

/// `okena project rm <project>` — unlinks the project from Okena (the folder on
/// disk is untouched; for worktrees use `okena worktree rm` to remove the checkout).
pub fn cli_project_rm(project: &str) -> i32 {
    with_state_post(|state| {
        let p = resolve::resolve_project(state, project)?;
        if p.worktree_info.is_some() {
            eprintln!(
                "Hint: '{}' is a worktree project — consider `okena worktree rm` instead.",
                p.name
            );
        }
        Ok(serde_json::json!({
            "action": "delete_project",
            "project_id": p.id,
        }))
    })
}

/// `okena project show|hide <project> [--window]`
pub fn cli_project_show(project: &str, show: bool, window: Option<&str>) -> i32 {
    with_state_post(|state| {
        let p = resolve::resolve_project(state, project)?;
        let mut body = serde_json::json!({
            "action": "set_project_show_in_overview",
            "project_id": p.id,
            "show": show,
        });
        if let Some(w) = window {
            body["window"] = serde_json::Value::String(resolve::resolve_window(state, w)?);
        }
        Ok(body)
    })
}

/// `okena project rename <project> <name>`
pub fn cli_project_rename(project: &str, name: &str) -> i32 {
    with_state_post(|state| {
        let p = resolve::resolve_project(state, project)?;
        Ok(serde_json::json!({
            "action": "rename_project",
            "project_id": p.id,
            "name": name,
        }))
    })
}

/// `okena project color <project> <color>`
pub fn cli_project_color(project: &str, color: &str) -> i32 {
    let color = match validate_color(color) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    with_state_post(|state| {
        let p = resolve::resolve_project(state, project)?;
        Ok(serde_json::json!({
            "action": "set_project_color",
            "project_id": p.id,
            "color": color,
        }))
    })
}

/// `okena project focus <project> [--window]`
pub fn cli_project_focus(project: &str, window: Option<&str>) -> i32 {
    with_state_post(|state| {
        let p = resolve::resolve_project(state, project)?;
        let terminal_id = resolve::first_terminal_id(p)
            .ok_or_else(|| format!("Project '{}' has no terminal to focus.", p.name))?;
        let mut body = serde_json::json!({
            "action": "focus_terminal",
            "project_id": p.id,
            "terminal_id": terminal_id,
        });
        if let Some(w) = window {
            body["window"] = serde_json::Value::String(resolve::resolve_window(state, w)?);
        }
        Ok(body)
    })
}

// ── Worktrees ────────────────────────────────────────────────────────────────

/// `okena worktree add <project> <branch> [--new-branch]`
pub fn cli_worktree_add(project: &str, branch: &str, new_branch: bool) -> i32 {
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
    let p = match resolve::resolve_project(&state, project) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let body = serde_json::json!({
        "action": "create_worktree",
        "project_id": p.id,
        "branch": branch,
        "create_branch": new_branch,
    });
    match api_post("/v1/actions", &token, &body.to_string()) {
        Ok(resp) => {
            // Returns {project_id, terminal_id, path} — print project_id and path.
            print_response_ids(&resp, &["project_id", "path"]);
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// `okena worktree rm <worktree> [--force]`
pub fn cli_worktree_rm(worktree: &str, force: bool) -> i32 {
    with_state_post(|state| {
        let p = resolve::resolve_project(state, worktree)?;
        Ok(serde_json::json!({
            "action": "remove_worktree_project",
            "project_id": p.id,
            "force": force,
        }))
    })
}

// ── Folders ──────────────────────────────────────────────────────────────────

/// `okena folder add <name>`
pub fn cli_folder_add(name: &str) -> i32 {
    let token = match ensure_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let body = serde_json::json!({ "action": "create_folder", "name": name });
    post_action_print_ids(&token, &body, &["folder_id"])
}

/// `okena folder rm <folder>`
pub fn cli_folder_rm(folder: &str) -> i32 {
    with_state_post(|state| {
        let folder_id = resolve_folder_id(state, folder)?;
        Ok(serde_json::json!({ "action": "delete_folder", "folder_id": folder_id }))
    })
}

/// `okena folder rename <folder> <name>`
pub fn cli_folder_rename(folder: &str, name: &str) -> i32 {
    with_state_post(|state| {
        let folder_id = resolve_folder_id(state, folder)?;
        Ok(serde_json::json!({
            "action": "rename_folder",
            "folder_id": folder_id,
            "name": name,
        }))
    })
}

// ── Terminals & layout ───────────────────────────────────────────────────────

/// `okena term new <project>`
pub fn cli_term_new(project: &str) -> i32 {
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
    let p = match resolve::resolve_project(&state, project) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let body = serde_json::json!({ "action": "create_terminal", "project_id": p.id });
    post_action_print_ids(&token, &body, &["terminal_ids"])
}

/// `okena term close <terminal>`
pub fn cli_term_close(terminal: &str) -> i32 {
    with_state_post(|state| {
        let (project_id, terminal_id) = resolve::resolve_terminal(state, terminal)?;
        Ok(serde_json::json!({
            "action": "close_terminal",
            "project_id": project_id,
            "terminal_id": terminal_id,
        }))
    })
}

/// `okena term focus <terminal> [--window]`
pub fn cli_term_focus(terminal: &str, window: Option<&str>) -> i32 {
    with_state_post(|state| {
        let (project_id, terminal_id) = resolve::resolve_terminal(state, terminal)?;
        let mut body = serde_json::json!({
            "action": "focus_terminal",
            "project_id": project_id,
            "terminal_id": terminal_id,
        });
        if let Some(w) = window {
            body["window"] = serde_json::Value::String(resolve::resolve_window(state, w)?);
        }
        Ok(body)
    })
}

/// `okena term rename <terminal> <name>`
pub fn cli_term_rename(terminal: &str, name: &str) -> i32 {
    with_state_post(|state| {
        let (project_id, terminal_id) = resolve::resolve_terminal(state, terminal)?;
        Ok(serde_json::json!({
            "action": "rename_terminal",
            "project_id": project_id,
            "terminal_id": terminal_id,
            "name": name,
        }))
    })
}

/// `okena term split <terminal> <h|v>`
pub fn cli_term_split(terminal: &str, direction: &str) -> i32 {
    let direction = match parse_direction(direction) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
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
    let state = match fetch_state(&token) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let (project_id, terminal_id) = match resolve::resolve_terminal(&state, terminal) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let p = match state.projects.iter().find(|p| p.id == project_id) {
        Some(p) => p,
        None => {
            eprintln!("Internal error: resolved project {project_id} missing from state");
            return 1;
        }
    };
    let path = match resolve::resolve_terminal_path(p, &terminal_id) {
        Some(path) => path,
        None => {
            eprintln!("Could not resolve layout path for terminal {terminal_id}");
            return 1;
        }
    };
    let body = serde_json::json!({
        "action": "split_terminal",
        "project_id": project_id,
        "path": path,
        "direction": direction,
    });
    post_action_print_ids(&token, &body, &["terminal_ids"])
}

/// `okena term tab <terminal>`
pub fn cli_term_tab(terminal: &str) -> i32 {
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
    let (project_id, terminal_id) = match resolve::resolve_terminal(&state, terminal) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let p = match state.projects.iter().find(|p| p.id == project_id) {
        Some(p) => p,
        None => {
            eprintln!("Internal error: resolved project {project_id} missing from state");
            return 1;
        }
    };
    let path = match resolve::resolve_terminal_path(p, &terminal_id) {
        Some(path) => path,
        None => {
            eprintln!("Could not resolve layout path for terminal {terminal_id}");
            return 1;
        }
    };
    // `in_group: false` mirrors the UI's add-tab button: if the terminal's
    // parent is already a Tabs container the new tab joins it, otherwise the
    // terminal is wrapped in a fresh Tabs group. `in_group: true` only works
    // when `path` points *at* an existing Tabs node, which we never produce
    // here (we always resolve to a terminal leaf) — so it was a silent no-op.
    let body = serde_json::json!({
        "action": "add_tab",
        "project_id": project_id,
        "path": path,
        "in_group": false,
    });
    post_action_print_ids(&token, &body, &["terminal_ids"])
}

/// `okena term minimize <terminal>`
pub fn cli_term_minimize(terminal: &str) -> i32 {
    with_state_post(|state| {
        let (project_id, terminal_id) = resolve::resolve_terminal(state, terminal)?;
        Ok(serde_json::json!({
            "action": "toggle_minimized",
            "project_id": project_id,
            "terminal_id": terminal_id,
        }))
    })
}

/// `okena term fullscreen <terminal> [--off] [--window]`
pub fn cli_term_fullscreen(terminal: &str, off: bool, window: Option<&str>) -> i32 {
    with_state_post(|state| {
        let (project_id, terminal_id) = resolve::resolve_terminal(state, terminal)?;
        let mut body = serde_json::json!({
            "action": "set_fullscreen",
            "project_id": project_id,
            "terminal_id": if off { serde_json::Value::Null } else { serde_json::Value::String(terminal_id) },
        });
        if let Some(w) = window {
            body["window"] = serde_json::Value::String(resolve::resolve_window(state, w)?);
        }
        Ok(body)
    })
}

// ── I/O (the agent loop) ─────────────────────────────────────────────────────

/// `okena send <terminal> <text...>`
pub fn cli_send(terminal: &str, text: &[String]) -> i32 {
    let text = text.join(" ");
    with_state_post(|state| {
        let (_project_id, terminal_id) = resolve::resolve_terminal(state, terminal)?;
        Ok(serde_json::json!({
            "action": "send_text",
            "terminal_id": terminal_id,
            "text": text,
        }))
    })
}

/// `okena run <terminal> <command...>`
pub fn cli_run(terminal: &str, command: &[String]) -> i32 {
    let command = command.join(" ");
    with_state_post(|state| {
        let (_project_id, terminal_id) = resolve::resolve_terminal(state, terminal)?;
        Ok(serde_json::json!({
            "action": "run_command",
            "terminal_id": terminal_id,
            "command": command,
        }))
    })
}

/// `okena key <terminal> <key>`
pub fn cli_key(terminal: &str, key: &str) -> i32 {
    let key_name = match map_special_key(key) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    with_state_post(|state| {
        let (_project_id, terminal_id) = resolve::resolve_terminal(state, terminal)?;
        Ok(serde_json::json!({
            "action": "send_special_key",
            "terminal_id": terminal_id,
            "key": key_name,
        }))
    })
}

/// `okena read <terminal> [--json]`
pub fn cli_read(terminal: &str, json_mode: bool) -> i32 {
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
    let (_project_id, terminal_id) = match resolve::resolve_terminal(&state, terminal) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let body = serde_json::json!({ "action": "read_content", "terminal_id": terminal_id });
    match api_post("/v1/actions", &token, &body.to_string()) {
        Ok(resp) => {
            if json_mode {
                println!("{}", resp.trim());
            } else {
                let v: serde_json::Value =
                    serde_json::from_str(&resp).unwrap_or(serde_json::Value::Null);
                match v.get("content").and_then(|c| c.as_str()) {
                    Some(content) => println!("{content}"),
                    None => println!("{}", resp.trim()),
                }
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
