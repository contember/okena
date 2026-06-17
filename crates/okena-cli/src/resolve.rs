//! Pure resolvers over a parsed [`StateResponse`]. These map user-facing
//! filters (project names/ids/paths, terminal addresses, window prefixes) to
//! the concrete ids and layout paths the remote action API expects.
//!
//! Everything here is intentionally free of I/O so it can be unit-tested with
//! hand-built state values (see the `tests` module at the bottom).

use okena_core::api::{ApiLayoutNode, ApiProject, StateResponse};

/// Resolve a project by exact id, case-insensitive name, or absolute path.
///
/// On a miss, returns an error listing the available project names so the
/// caller (and any agent reading stderr) can correct the filter.
pub fn resolve_project<'a>(
    state: &'a StateResponse,
    filter: &str,
) -> Result<&'a ApiProject, String> {
    for p in &state.projects {
        if p.id == filter {
            return Ok(p);
        }
    }
    for p in &state.projects {
        if p.name.eq_ignore_ascii_case(filter) {
            return Ok(p);
        }
    }
    // Match by absolute path. Canonicalize the filter when it points at a real
    // path so `./foo`, `foo`, and trailing slashes all resolve consistently.
    let filter_abs = std::fs::canonicalize(filter)
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    for p in &state.projects {
        if p.path == filter {
            return Ok(p);
        }
        if let Some(fa) = &filter_abs {
            let proj_abs = std::fs::canonicalize(&p.path)
                .ok()
                .map(|x| x.to_string_lossy().into_owned());
            if proj_abs.as_deref() == Some(fa.as_str()) || p.path == *fa {
                return Ok(p);
            }
        }
    }

    Err(format!(
        "Project not found: {filter}\nAvailable: {}",
        available_project_names(state)
    ))
}

fn available_project_names(state: &StateResponse) -> String {
    if state.projects.is_empty() {
        return "(none)".to_string();
    }
    state
        .projects
        .iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A terminal in a project's layout, in DFS order: its id (if assigned) and
/// display name (from `terminal_names`, falling back to the id).
pub struct TerminalEntry {
    pub terminal_id: String,
    pub name: String,
}

/// List a project's terminals in DFS order (same traversal order the layout
/// path resolver uses), pairing each id with its display name.
pub fn project_terminals(project: &ApiProject) -> Vec<TerminalEntry> {
    let mut out = Vec::new();
    if let Some(layout) = &project.layout {
        collect_terminals(layout, project, &mut out);
    }
    out
}

fn collect_terminals(node: &ApiLayoutNode, project: &ApiProject, out: &mut Vec<TerminalEntry>) {
    match node {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id),
            ..
        } => {
            let name = project
                .terminal_names
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.clone());
            out.push(TerminalEntry {
                terminal_id: id.clone(),
                name,
            });
        }
        ApiLayoutNode::Terminal { .. } => {}
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_terminals(child, project, out);
            }
        }
    }
}

/// Resolve the first terminal id in a project's layout (DFS order). Used by
/// `project focus`, which needs a concrete terminal to hand to `focus_terminal`.
pub fn first_terminal_id(project: &ApiProject) -> Option<String> {
    project_terminals(project).into_iter().next().map(|t| t.terminal_id)
}

/// Resolve a terminal address into `(project_id, terminal_id)`.
///
/// Accepted forms:
/// - a bare terminal UUID — searched across every project's layout;
/// - `"<project>/<name>"` — resolve the project, then the terminal whose
///   `terminal_names` value equals `<name>` (case-insensitive);
/// - `"<project>:<index>"` — the Nth terminal (0-based) in that project's
///   layout, in DFS order.
pub fn resolve_terminal(
    state: &StateResponse,
    filter: &str,
) -> Result<(String, String), String> {
    // Form: <project>:<index>
    if let Some((proj_part, idx_part)) = filter.rsplit_once(':')
        && let Ok(index) = idx_part.parse::<usize>()
    {
        let project = resolve_project(state, proj_part)?;
        let terms = project_terminals(project);
        return match terms.get(index) {
            Some(t) => Ok((project.id.clone(), t.terminal_id.clone())),
            None => Err(format!(
                "Terminal index {index} out of range for project '{}' ({} terminal(s))",
                project.name,
                terms.len()
            )),
        };
    }

    // Form: <project>/<name>
    if let Some((proj_part, name_part)) = filter.split_once('/') {
        let project = resolve_project(state, proj_part)?;
        for (tid, tname) in &project.terminal_names {
            if tname.eq_ignore_ascii_case(name_part) {
                return Ok((project.id.clone(), tid.clone()));
            }
        }
        // Unnamed terminals aren't in `terminal_names`, but `term ls` shows
        // their id in the name column — so accept a bare terminal id scoped to
        // this project too (matches what the listing displays).
        if let Some(layout) = &project.layout
            && layout_contains_terminal(layout, name_part)
        {
            return Ok((project.id.clone(), name_part.to_string()));
        }
        return Err(format!(
            "No terminal named '{name_part}' in project '{}'.\nTerminals: {}",
            project.name,
            terminal_names_list(project)
        ));
    }

    // Form: bare terminal id — search every project's layout.
    for project in &state.projects {
        if let Some(layout) = &project.layout
            && layout_contains_terminal(layout, filter)
        {
            return Ok((project.id.clone(), filter.to_string()));
        }
    }

    Err(format!("Terminal not found: {filter}"))
}

fn terminal_names_list(project: &ApiProject) -> String {
    let mut names: Vec<&str> = project.terminal_names.values().map(|s| s.as_str()).collect();
    names.sort_unstable();
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

fn layout_contains_terminal(node: &ApiLayoutNode, target: &str) -> bool {
    match node {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id),
            ..
        } => id == target,
        ApiLayoutNode::Terminal { .. } => false,
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
            children.iter().any(|c| layout_contains_terminal(c, target))
        }
    }
}

/// Resolve the layout path (chain of child indices from the root layout node to
/// the matching `Terminal` node) for a terminal id.
///
/// This mirrors `okena_layout::LayoutNode::find_terminal_path` exactly: a DFS
/// that pushes the child index at every Split/Tabs level and returns the
/// accumulated path at the matching terminal. The same `Vec<usize>` semantics
/// are what `split_terminal` / `add_tab` consume on the server.
pub fn resolve_terminal_path(project: &ApiProject, terminal_id: &str) -> Option<Vec<usize>> {
    let layout = project.layout.as_ref()?;
    find_path(layout, terminal_id, Vec::new())
}

fn find_path(node: &ApiLayoutNode, target: &str, current: Vec<usize>) -> Option<Vec<usize>> {
    match node {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id),
            ..
        } if id == target => Some(current),
        ApiLayoutNode::Terminal { .. } => None,
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
            for (i, child) in children.iter().enumerate() {
                let mut child_path = current.clone();
                child_path.push(i);
                if let Some(found) = find_path(child, target, child_path) {
                    return Some(found);
                }
            }
            None
        }
    }
}

/// Resolve a window filter to a concrete window id string ("main" or a full
/// UUID).
///
/// Accepts the exact id `"main"`, a full id, or a UNIQUE id prefix. Errors
/// clearly on an unknown or ambiguous prefix, listing the available windows.
pub fn resolve_window(state: &StateResponse, filter: &str) -> Result<String, String> {
    // Exact id match wins outright (covers "main" and full UUIDs).
    if let Some(w) = state.windows.iter().find(|w| w.id == filter) {
        return Ok(w.id.clone());
    }

    let matches: Vec<&str> = state
        .windows
        .iter()
        .filter(|w| w.id.starts_with(filter))
        .map(|w| w.id.as_str())
        .collect();

    match matches.as_slice() {
        [only] => Ok((*only).to_string()),
        [] => Err(format!(
            "Window not found: {filter}\nWindows: {}",
            window_ids_list(state)
        )),
        many => Err(format!(
            "Ambiguous window prefix '{filter}' — matches: {}",
            many.join(", ")
        )),
    }
}

fn window_ids_list(state: &StateResponse) -> String {
    if state.windows.is_empty() {
        return "(none)".to_string();
    }
    state
        .windows
        .iter()
        .map(|w| w.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::api::{ApiProject, ApiWindow, StateResponse};
    use okena_core::theme::FolderColor;
    use okena_core::types::SplitDirection;
    use std::collections::HashMap;

    fn term(id: &str) -> ApiLayoutNode {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id.into()),
            minimized: false,
            detached: false,
            cols: None,
            rows: None,
        }
    }

    fn project(id: &str, name: &str, path: &str, layout: Option<ApiLayoutNode>) -> ApiProject {
        let mut terminal_names = HashMap::new();
        terminal_names.insert("t1".to_string(), "shell".to_string());
        terminal_names.insert("t2".to_string(), "logs".to_string());
        terminal_names.insert("t3".to_string(), "editor".to_string());
        ApiProject {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            show_in_overview: true,
            layout,
            terminal_names,
            git_status: None,
            folder_color: FolderColor::Default,
            services: vec![],
            worktree_info: None,
            worktree_ids: vec![],
        }
    }

    /// Layout:
    ///   Split(h)
    ///     ├─ t1                 path [0]
    ///     └─ Tabs[active=1]
    ///          ├─ t2            path [1, 0]
    ///          └─ Split(v)
    ///               ├─ t3       path [1, 1, 0]
    ///               └─ t4       path [1, 1, 1]
    fn nested_layout() -> ApiLayoutNode {
        ApiLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![
                term("t1"),
                ApiLayoutNode::Tabs {
                    active_tab: 1,
                    children: vec![
                        term("t2"),
                        ApiLayoutNode::Split {
                            direction: SplitDirection::Vertical,
                            sizes: vec![50.0, 50.0],
                            children: vec![term("t3"), term("t4")],
                        },
                    ],
                },
            ],
        }
    }

    fn state_with(projects: Vec<ApiProject>, windows: Vec<ApiWindow>) -> StateResponse {
        StateResponse {
            state_version: 1,
            projects,
            focused_project_id: None,
            fullscreen_terminal: None,
            project_order: vec![],
            folders: vec![],
            windows,
        }
    }

    fn window(id: &str, active: bool) -> ApiWindow {
        ApiWindow {
            id: id.into(),
            kind: if id == "main" { "main".into() } else { "extra".into() },
            active,
            focused_project_id: None,
            focused_terminal_id: None,
            fullscreen: None,
            visible_project_ids: vec![],
            folder_filter: None,
            bounds: None,
            sidebar_open: None,
        }
    }

    #[test]
    fn resolve_terminal_path_nested_split_and_tabs() {
        let p = project("p1", "Proj", "/tmp/p1", Some(nested_layout()));
        assert_eq!(resolve_terminal_path(&p, "t1"), Some(vec![0]));
        assert_eq!(resolve_terminal_path(&p, "t2"), Some(vec![1, 0]));
        assert_eq!(resolve_terminal_path(&p, "t3"), Some(vec![1, 1, 0]));
        assert_eq!(resolve_terminal_path(&p, "t4"), Some(vec![1, 1, 1]));
        assert_eq!(resolve_terminal_path(&p, "missing"), None);
    }

    #[test]
    fn resolve_terminal_path_root_terminal() {
        // A bare terminal at the root has an empty path.
        let p = project("p1", "Proj", "/tmp/p1", Some(term("only")));
        assert_eq!(resolve_terminal_path(&p, "only"), Some(vec![]));
    }

    #[test]
    fn resolve_terminal_by_bare_id() {
        let p = project("p1", "Proj", "/tmp/p1", Some(nested_layout()));
        let state = state_with(vec![p], vec![]);
        let (pid, tid) = resolve_terminal(&state, "t3").unwrap();
        assert_eq!(pid, "p1");
        assert_eq!(tid, "t3");

        assert!(resolve_terminal(&state, "nope").is_err());
    }

    #[test]
    fn resolve_terminal_by_project_slash_name() {
        let p = project("p1", "Proj", "/tmp/p1", Some(nested_layout()));
        let state = state_with(vec![p], vec![]);
        // terminal_names: t1->shell, t2->logs, t3->editor
        let (pid, tid) = resolve_terminal(&state, "Proj/logs").unwrap();
        assert_eq!(pid, "p1");
        assert_eq!(tid, "t2");

        // Case-insensitive name match.
        let (_, tid2) = resolve_terminal(&state, "p1/SHELL").unwrap();
        assert_eq!(tid2, "t1");

        assert!(resolve_terminal(&state, "Proj/nonexistent").is_err());
    }

    #[test]
    fn resolve_terminal_by_project_slash_id_for_unnamed() {
        // t4 is in the layout but has no `terminal_names` entry (unnamed). The
        // `<project>/<name>` form must still resolve it by its id, matching what
        // `term ls` prints in the name column for unnamed terminals.
        let p = project("p1", "Proj", "/tmp/p1", Some(nested_layout()));
        let state = state_with(vec![p], vec![]);
        let (pid, tid) = resolve_terminal(&state, "Proj/t4").unwrap();
        assert_eq!(pid, "p1");
        assert_eq!(tid, "t4");
    }

    #[test]
    fn resolve_terminal_by_project_colon_index() {
        let p = project("p1", "Proj", "/tmp/p1", Some(nested_layout()));
        let state = state_with(vec![p], vec![]);
        // DFS order: t1, t2, t3, t4
        assert_eq!(resolve_terminal(&state, "Proj:0").unwrap().1, "t1");
        assert_eq!(resolve_terminal(&state, "Proj:1").unwrap().1, "t2");
        assert_eq!(resolve_terminal(&state, "Proj:2").unwrap().1, "t3");
        assert_eq!(resolve_terminal(&state, "Proj:3").unwrap().1, "t4");
        assert!(resolve_terminal(&state, "Proj:4").is_err());
    }

    #[test]
    fn resolve_project_by_id_name_path() {
        let p1 = project("p1id", "Alpha", "/tmp/alpha", Some(term("t1")));
        let p2 = project("p2id", "Beta", "/tmp/beta", Some(term("t9")));
        let state = state_with(vec![p1, p2], vec![]);

        assert_eq!(resolve_project(&state, "p2id").unwrap().id, "p2id");
        assert_eq!(resolve_project(&state, "alpha").unwrap().id, "p1id");
        assert_eq!(resolve_project(&state, "ALPHA").unwrap().id, "p1id");
        // Exact path string match (no canonicalization needed for a non-existent dir).
        assert_eq!(resolve_project(&state, "/tmp/beta").unwrap().id, "p2id");
        assert!(resolve_project(&state, "gamma").is_err());
    }

    #[test]
    fn resolve_window_prefix_and_ambiguity() {
        let state = state_with(
            vec![],
            vec![
                window("main", true),
                window("abc12345-aaaa", false),
                window("abc99999-bbbb", false),
                window("def00000-cccc", false),
            ],
        );

        // Exact "main".
        assert_eq!(resolve_window(&state, "main").unwrap(), "main");
        // Unique prefix.
        assert_eq!(resolve_window(&state, "def").unwrap(), "def00000-cccc");
        // Full id.
        assert_eq!(resolve_window(&state, "abc12345-aaaa").unwrap(), "abc12345-aaaa");
        // Ambiguous prefix.
        assert!(resolve_window(&state, "abc").is_err());
        // Unknown.
        assert!(resolve_window(&state, "zzz").is_err());
    }

    #[test]
    fn project_terminals_dfs_order_and_first() {
        let p = project("p1", "Proj", "/tmp/p1", Some(nested_layout()));
        let terms = project_terminals(&p);
        let ids: Vec<&str> = terms.iter().map(|t| t.terminal_id.as_str()).collect();
        assert_eq!(ids, vec!["t1", "t2", "t3", "t4"]);
        assert_eq!(first_terminal_id(&p).as_deref(), Some("t1"));

        // Empty layout → no terminals, no first.
        let empty = project("p2", "Empty", "/tmp/p2", None);
        assert!(project_terminals(&empty).is_empty());
        assert_eq!(first_terminal_id(&empty), None);
    }
}
