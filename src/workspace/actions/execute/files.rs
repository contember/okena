//! Filesystem action handlers — listing, reading, and mutating project files.

use super::{
    ActionResult, Workspace, resolve_new_project_file, resolve_project_file, validate_leaf_name,
};

pub(super) fn list_files(ws: &Workspace, project_id: String, show_ignored: bool) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = match std::path::Path::new(&p.path).canonicalize() {
                Ok(c) => c,
                Err(e) => return ActionResult::Err(format!("Cannot resolve project path: {}", e)),
            };
            let files = okena_files::file_search::FileSearchDialog::scan_files(&path, show_ignored);
            ActionResult::Ok(Some(serde_json::to_value(files).expect("BUG: FileEntry must serialize")))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn list_directory(ws: &Workspace, project_id: String, relative_path: String, show_ignored: bool) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = match std::path::Path::new(&p.path).canonicalize() {
                Ok(c) => c,
                Err(e) => return ActionResult::Err(format!("Cannot resolve project path: {}", e)),
            };
            match okena_files::list_directory::list_directory(&path, &relative_path, show_ignored) {
                Ok(entries) => ActionResult::Ok(Some(
                    serde_json::to_value(entries).expect("BUG: DirEntry must serialize"),
                )),
                Err(e) => ActionResult::Err(e),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn read_file(ws: &Workspace, project_id: String, relative_path: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let canonical = match resolve_project_file(&p.path, &relative_path) {
                Ok(c) => c,
                Err(e) => return ActionResult::Err(e),
            };
            match std::fs::read_to_string(&canonical) {
                Ok(content) => ActionResult::Ok(Some(serde_json::json!({ "content": content }))),
                Err(e) => ActionResult::Err(format!("Cannot read file: {}", e)),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn file_size(ws: &Workspace, project_id: String, relative_path: String) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let canonical = match resolve_project_file(&p.path, &relative_path) {
                Ok(c) => c,
                Err(e) => return ActionResult::Err(e),
            };
            match std::fs::metadata(&canonical) {
                Ok(m) => ActionResult::Ok(Some(serde_json::json!({ "size": m.len() }))),
                Err(e) => ActionResult::Err(format!("Cannot read file: {}", e)),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn search_content(
    ws: &Workspace,
    project_id: String,
    query: String,
    case_sensitive: bool,
    mode: String,
    max_results: usize,
    file_glob: Option<String>,
    context_lines: usize,
) -> ActionResult {
    if let Some(ref glob) = file_glob
        && (glob.contains("..") || glob.starts_with('/')) {
            return ActionResult::Err("file_glob must not contain '..' or start with '/'".to_string());
        }
    match ws.project(&project_id) {
        Some(p) => {
            let path = match std::path::Path::new(&p.path).canonicalize() {
                Ok(c) => c,
                Err(e) => return ActionResult::Err(format!("Cannot resolve project path: {}", e)),
            };
            let search_mode = match mode.as_str() {
                "regex" => okena_files::content_search::SearchMode::Regex,
                "fuzzy" => okena_files::content_search::SearchMode::Fuzzy,
                _ => okena_files::content_search::SearchMode::Literal,
            };
            let config = okena_files::content_search::ContentSearchConfig {
                case_sensitive,
                mode: search_mode,
                max_results,
                file_glob,
                context_lines,
                show_ignored: false,
            };
            let cancelled = std::sync::atomic::AtomicBool::new(false);
            let mut results = Vec::new();
            okena_files::content_search::search_content(
                &path, &query, &config, &cancelled, &mut |result| results.push(result),
            );
            ActionResult::Ok(Some(serde_json::to_value(results).expect("BUG: FileSearchResult must serialize")))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn rename_file(ws: &Workspace, project_id: String, relative_path: String, new_name: String) -> ActionResult {
    if let Err(e) = validate_leaf_name(&new_name) {
        return ActionResult::Err(e);
    }
    let project_path = match ws.project(&project_id) {
        Some(p) => p.path.clone(),
        None => return ActionResult::Err(format!("project not found: {}", project_id)),
    };
    let old_path = match resolve_project_file(&project_path, &relative_path) {
        Ok(c) => c,
        Err(e) => return ActionResult::Err(e),
    };
    let parent = match old_path.parent() {
        Some(p) => p,
        None => return ActionResult::Err("cannot rename project root".to_string()),
    };
    let new_path = parent.join(&new_name);
    if new_path.exists() {
        return ActionResult::Err(format!("target already exists: {}", new_name));
    }
    match std::fs::rename(&old_path, &new_path) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!("Cannot rename: {}", e)),
    }
}

pub(super) fn delete_file(ws: &Workspace, project_id: String, relative_path: String) -> ActionResult {
    let project_path = match ws.project(&project_id) {
        Some(p) => p.path.clone(),
        None => return ActionResult::Err(format!("project not found: {}", project_id)),
    };
    let target = match resolve_project_file(&project_path, &relative_path) {
        Ok(c) => c,
        Err(e) => return ActionResult::Err(e),
    };
    let project_root = match std::path::Path::new(&project_path).canonicalize() {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(format!("Cannot resolve project path: {}", e)),
    };
    if target == project_root {
        return ActionResult::Err("cannot delete project root".to_string());
    }
    let result = if target.is_dir() {
        std::fs::remove_dir_all(&target)
    } else {
        std::fs::remove_file(&target)
    };
    match result {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!("Cannot delete: {}", e)),
    }
}

pub(super) fn create_file(ws: &Workspace, project_id: String, relative_path: String) -> ActionResult {
    let project_path = match ws.project(&project_id) {
        Some(p) => p.path.clone(),
        None => return ActionResult::Err(format!("project not found: {}", project_id)),
    };
    let target = match resolve_new_project_file(&project_path, &relative_path) {
        Ok(c) => c,
        Err(e) => return ActionResult::Err(e),
    };
    if target.exists() {
        return ActionResult::Err("target already exists".to_string());
    }
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&target) {
        Ok(_) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!("Cannot create file: {}", e)),
    }
}

pub(super) fn create_directory(ws: &Workspace, project_id: String, relative_path: String) -> ActionResult {
    let project_path = match ws.project(&project_id) {
        Some(p) => p.path.clone(),
        None => return ActionResult::Err(format!("project not found: {}", project_id)),
    };
    let target = match resolve_new_project_file(&project_path, &relative_path) {
        Ok(c) => c,
        Err(e) => return ActionResult::Err(e),
    };
    if target.exists() {
        return ActionResult::Err("target already exists".to_string());
    }
    match std::fs::create_dir(&target) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!("Cannot create directory: {}", e)),
    }
}
