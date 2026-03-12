use crate::process::command;

/// Returns the HEAD commit hash of the given project.
pub fn get_snapshot(project_path: &str) -> Option<String> {
    let output = command("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Returns `git diff --stat` between two commits.
pub fn get_diff_stat(project_path: &str, before: &str, after: &str) -> Option<String> {
    let range = format!("{before}..{after}");
    let output = command("git")
        .args(["diff", "--stat", &range])
        .current_dir(project_path)
        .output()
        .ok()?;

    if output.status.success() {
        let stat = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stat.is_empty() { None } else { Some(stat) }
    } else {
        None
    }
}

/// Returns `git diff --stat HEAD` for uncommitted working-tree changes.
#[allow(dead_code)]
pub fn get_working_tree_diff_stat(project_path: &str) -> Option<String> {
    let output = command("git")
        .args(["diff", "--stat", "HEAD"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if output.status.success() {
        let stat = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stat.is_empty() { None } else { Some(stat) }
    } else {
        None
    }
}
