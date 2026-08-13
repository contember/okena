//! Filesystem action handlers — listing, reading, and mutating project files.

use super::{
    ActionResult, Workspace, resolve_new_project_file, resolve_project_file, validate_leaf_name,
};
use okena_core::api::{PathBreadcrumb, ResolvedPath, ResolvedPathKind};
use okena_terminal::TerminalsRegistry;
use std::path::{Path, PathBuf};

pub struct PreparedContentSearch {
    project_path: std::path::PathBuf,
    query: String,
    config: okena_files::content_search::ContentSearchConfig,
}

pub(super) fn list_files(ws: &Workspace, project_id: String, show_ignored: bool) -> ActionResult {
    match ws.project(&project_id) {
        Some(p) => {
            let path = match std::path::Path::new(&p.path).canonicalize() {
                Ok(c) => c,
                Err(e) => return ActionResult::Err(format!("Cannot resolve project path: {}", e)),
            };
            let files = okena_files::file_scan::scan_files(&path, show_ignored);
            ActionResult::Ok(Some(
                serde_json::to_value(files).expect("BUG: FileEntry must serialize"),
            ))
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

pub(super) fn list_directory(
    ws: &Workspace,
    project_id: String,
    relative_path: String,
    show_ignored: bool,
) -> ActionResult {
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

/// Server-side ceiling on bytes returned from ReadFileBytes. Mirrors the
/// client's MAX_IMAGE_FILE_SIZE so a misbehaving or older client can't trick
/// the server into reading and base64-encoding arbitrarily large files
/// (each request transiently holds raw + base64 + JSON copies, so the
/// resident multiple is roughly 3-4× the file size).
const MAX_READ_FILE_BYTES: u64 = 20 * 1024 * 1024;

fn modified_at_millis(metadata: &std::fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

fn path_breadcrumbs(path: &Path) -> Vec<PathBreadcrumb> {
    let mut ancestors: Vec<&Path> = path.ancestors().collect();
    ancestors.reverse();
    ancestors
        .into_iter()
        .map(|ancestor| PathBreadcrumb {
            canonical_path: ancestor.to_string_lossy().into_owned(),
            label: ancestor
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| ancestor.to_string_lossy().into_owned()),
        })
        .collect()
}

fn resolved_path(ws: &Workspace, canonical_path: PathBuf) -> Result<ResolvedPath, String> {
    let metadata =
        std::fs::metadata(&canonical_path).map_err(|error| format!("Cannot read path: {error}"))?;
    let kind = if metadata.is_file() {
        ResolvedPathKind::File
    } else if metadata.is_dir() {
        ResolvedPathKind::Directory
    } else {
        return Err("Path is neither a regular file nor a directory".to_string());
    };

    let mut containing_project = None;
    for project in &ws.data().projects {
        let Ok(project_root) = Path::new(&project.path).canonicalize() else {
            continue;
        };
        let Ok(relative_path) = canonical_path.strip_prefix(&project_root) else {
            continue;
        };
        let depth = project_root.components().count();
        if containing_project
            .as_ref()
            .is_none_or(|(current_depth, _, _)| depth > *current_depth)
        {
            containing_project = Some((
                depth,
                project.id.clone(),
                relative_path.to_string_lossy().replace('\\', "/"),
            ));
        }
    }

    let name = canonical_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| canonical_path.to_string_lossy().into_owned());
    let (_, project_id, relative_path) = containing_project
        .map(|value| (value.0, Some(value.1), Some(value.2)))
        .unwrap_or((0, None, None));
    let breadcrumbs = path_breadcrumbs(&canonical_path);
    Ok(ResolvedPath {
        canonical_path: canonical_path.to_string_lossy().into_owned(),
        name,
        kind,
        size: metadata.len(),
        modified_at_millis: modified_at_millis(&metadata),
        project_id,
        relative_path,
        breadcrumbs,
    })
}

fn expand_terminal_path(path: &str, cwd: &str) -> Result<PathBuf, String> {
    if path.starts_with("file://") {
        let url = url::Url::parse(path).map_err(|error| format!("Invalid file URL: {error}"))?;
        if let Ok(path) = url.to_file_path() {
            return Ok(path);
        }
        #[cfg(unix)]
        if url.host_str().is_some() {
            let decoded = percent_encoding::percent_decode_str(url.path())
                .decode_utf8()
                .map_err(|error| format!("File URL path is not valid UTF-8: {error}"))?;
            return Ok(PathBuf::from(decoded.as_ref()));
        }
        return Err("File URL does not contain a local path".to_string());
    }
    if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or_else(|| "Cannot resolve the daemon home directory".to_string())?;
        return Ok(
            PathBuf::from(home).join(path.trim_start_matches('~').trim_start_matches(['/', '\\']))
        );
    }
    let path = Path::new(path);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(cwd).join(path)
    })
}

pub(super) fn resolve_project_path_action(
    ws: &Workspace,
    project_id: String,
    relative_path: String,
) -> ActionResult {
    let project = match ws.project(&project_id) {
        Some(project) => project,
        None => return ActionResult::Err(format!("project not found: {project_id}")),
    };
    match resolve_project_file(&project.path, &relative_path)
        .and_then(|path| resolved_path(ws, path))
    {
        Ok(path) => ActionResult::Ok(Some(
            serde_json::to_value(path).expect("BUG: ResolvedPath must serialize"),
        )),
        Err(error) => ActionResult::Err(error),
    }
}

fn resolve_terminal_path(
    ws: &Workspace,
    terminals: &TerminalsRegistry,
    terminal_id: &str,
    path: &str,
) -> Result<ResolvedPath, String> {
    let terminal = terminals
        .lock()
        .get(terminal_id)
        .cloned()
        .ok_or_else(|| format!("terminal not found: {terminal_id}"))?;
    let expanded = expand_terminal_path(path, &terminal.current_cwd())?;
    let canonical = expanded
        .canonicalize()
        .map_err(|error| format!("Cannot resolve path: {error}"))?;
    resolved_path(ws, canonical)
}

fn require_file(path: ResolvedPath) -> Result<ResolvedPath, String> {
    if path.kind == ResolvedPathKind::File {
        Ok(path)
    } else {
        Err("Path is not a regular file".to_string())
    }
}

pub(super) fn resolve_terminal_path_action(
    ws: &Workspace,
    terminals: &TerminalsRegistry,
    terminal_id: String,
    path: String,
) -> ActionResult {
    match resolve_terminal_path(ws, terminals, &terminal_id, &path) {
        Ok(path) => ActionResult::Ok(Some(
            serde_json::to_value(path).expect("BUG: ResolvedPath must serialize"),
        )),
        Err(error) => ActionResult::Err(error),
    }
}

pub(super) fn read_terminal_file(
    ws: &Workspace,
    terminals: &TerminalsRegistry,
    terminal_id: String,
    path: String,
) -> ActionResult {
    let file =
        match resolve_terminal_path(ws, terminals, &terminal_id, &path).and_then(require_file) {
            Ok(file) => file,
            Err(error) => return ActionResult::Err(error),
        };
    match std::fs::read_to_string(&file.canonical_path) {
        Ok(content) => ActionResult::Ok(Some(serde_json::json!({ "content": content }))),
        Err(error) => ActionResult::Err(format!("Cannot read file: {error}")),
    }
}

pub(super) fn read_terminal_file_bytes(
    ws: &Workspace,
    terminals: &TerminalsRegistry,
    terminal_id: String,
    path: String,
) -> ActionResult {
    use base64::Engine as _;
    let file =
        match resolve_terminal_path(ws, terminals, &terminal_id, &path).and_then(require_file) {
            Ok(file) => file,
            Err(error) => return ActionResult::Err(error),
        };
    if file.size > MAX_READ_FILE_BYTES {
        return ActionResult::Err(format!(
            "File too large ({:.1} MB). Maximum is {} MB.",
            file.size as f64 / 1024.0 / 1024.0,
            MAX_READ_FILE_BYTES / 1024 / 1024
        ));
    }
    match std::fs::read(&file.canonical_path) {
        Ok(bytes) if bytes.len() as u64 <= MAX_READ_FILE_BYTES => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            ActionResult::Ok(Some(serde_json::json!({ "content_b64": encoded })))
        }
        Ok(bytes) => ActionResult::Err(format!(
            "File too large ({:.1} MB). Maximum is {} MB.",
            bytes.len() as f64 / 1024.0 / 1024.0,
            MAX_READ_FILE_BYTES / 1024 / 1024
        )),
        Err(error) => ActionResult::Err(format!("Cannot read file: {error}")),
    }
}

pub(super) fn terminal_file_size(
    ws: &Workspace,
    terminals: &TerminalsRegistry,
    terminal_id: String,
    path: String,
) -> ActionResult {
    match resolve_terminal_path(ws, terminals, &terminal_id, &path).and_then(require_file) {
        Ok(file) => ActionResult::Ok(Some(serde_json::json!({
            "size": file.size,
            "modified_at_millis": file.modified_at_millis,
        }))),
        Err(error) => ActionResult::Err(error),
    }
}

pub(super) fn resolve_path_action(ws: &Workspace, path: String) -> ActionResult {
    let canonical = match Path::new(&path).canonicalize() {
        Ok(path) => path,
        Err(error) => return ActionResult::Err(format!("Cannot resolve path: {error}")),
    };
    match resolved_path(ws, canonical) {
        Ok(path) => ActionResult::Ok(Some(
            serde_json::to_value(path).expect("BUG: ResolvedPath must serialize"),
        )),
        Err(error) => ActionResult::Err(error),
    }
}

pub(super) fn resolve_path_in_scope_action(
    ws: &Workspace,
    root: String,
    relative_path: String,
) -> ActionResult {
    let canonical = match resolve_project_file(&root, &relative_path) {
        Ok(path) => path,
        Err(error) => return ActionResult::Err(error),
    };
    match resolved_path(ws, canonical) {
        Ok(path) => ActionResult::Ok(Some(
            serde_json::to_value(path).expect("BUG: ResolvedPath must serialize"),
        )),
        Err(error) => ActionResult::Err(error),
    }
}

fn canonical_scope_root(root: &str) -> Result<PathBuf, String> {
    let root = Path::new(root)
        .canonicalize()
        .map_err(|error| format!("Cannot resolve browser root: {error}"))?;
    if !root.is_dir() {
        return Err("Browser root is not a directory".to_string());
    }
    Ok(root)
}

fn resolve_path_file(root: &str, relative_path: &str) -> Result<PathBuf, String> {
    let path = resolve_project_file(root, relative_path)?;
    if !path.is_file() {
        return Err("Path is not a regular file".to_string());
    }
    Ok(path)
}

pub(super) fn list_path_files(root: String, show_ignored: bool) -> ActionResult {
    let root = match canonical_scope_root(&root) {
        Ok(root) => root,
        Err(error) => return ActionResult::Err(error),
    };
    let files = okena_files::file_scan::scan_files(&root, show_ignored);
    ActionResult::Ok(Some(
        serde_json::to_value(files).expect("BUG: FileEntry must serialize"),
    ))
}

pub(super) fn list_path_directory(
    root: String,
    relative_path: String,
    show_ignored: bool,
) -> ActionResult {
    let root = match canonical_scope_root(&root) {
        Ok(root) => root,
        Err(error) => return ActionResult::Err(error),
    };
    match okena_files::list_directory::list_directory(&root, &relative_path, show_ignored) {
        Ok(entries) => ActionResult::Ok(Some(
            serde_json::to_value(entries).expect("BUG: DirEntry must serialize"),
        )),
        Err(error) => ActionResult::Err(error),
    }
}

pub(super) fn read_path_file(root: String, relative_path: String) -> ActionResult {
    let path = match resolve_path_file(&root, &relative_path) {
        Ok(path) => path,
        Err(error) => return ActionResult::Err(error),
    };
    match std::fs::read_to_string(path) {
        Ok(content) => ActionResult::Ok(Some(serde_json::json!({ "content": content }))),
        Err(error) => ActionResult::Err(format!("Cannot read file: {error}")),
    }
}

pub(super) fn read_path_file_bytes(root: String, relative_path: String) -> ActionResult {
    use base64::Engine as _;
    let path = match resolve_path_file(&root, &relative_path) {
        Ok(path) => path,
        Err(error) => return ActionResult::Err(error),
    };
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => return ActionResult::Err(format!("Cannot read file: {error}")),
    };
    if metadata.len() > MAX_READ_FILE_BYTES {
        return ActionResult::Err(format!(
            "File too large ({:.1} MB). Maximum is {} MB.",
            metadata.len() as f64 / 1024.0 / 1024.0,
            MAX_READ_FILE_BYTES / 1024 / 1024
        ));
    }
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() as u64 <= MAX_READ_FILE_BYTES => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            ActionResult::Ok(Some(serde_json::json!({ "content_b64": encoded })))
        }
        Ok(bytes) => ActionResult::Err(format!(
            "File too large ({:.1} MB). Maximum is {} MB.",
            bytes.len() as f64 / 1024.0 / 1024.0,
            MAX_READ_FILE_BYTES / 1024 / 1024
        )),
        Err(error) => ActionResult::Err(format!("Cannot read file: {error}")),
    }
}

pub(super) fn path_file_size(root: String, relative_path: String) -> ActionResult {
    let path = match resolve_path_file(&root, &relative_path) {
        Ok(path) => path,
        Err(error) => return ActionResult::Err(error),
    };
    match std::fs::metadata(path) {
        Ok(metadata) => ActionResult::Ok(Some(serde_json::json!({
            "size": metadata.len(),
            "modified_at_millis": modified_at_millis(&metadata),
        }))),
        Err(error) => ActionResult::Err(format!("Cannot read file: {error}")),
    }
}

pub(super) fn read_file_bytes(
    ws: &Workspace,
    project_id: String,
    relative_path: String,
) -> ActionResult {
    use base64::Engine as _;
    match ws.project(&project_id) {
        Some(p) => {
            let canonical = match resolve_project_file(&p.path, &relative_path) {
                Ok(c) => c,
                Err(e) => return ActionResult::Err(e),
            };
            // Enforce the cap from metadata before allocating; std::fs::read
            // alone would happily pull a multi-GB file into memory.
            match std::fs::metadata(&canonical) {
                Ok(m) if m.len() > MAX_READ_FILE_BYTES => {
                    return ActionResult::Err(format!(
                        "File too large ({:.1} MB). Maximum is {} MB.",
                        m.len() as f64 / 1024.0 / 1024.0,
                        MAX_READ_FILE_BYTES / 1024 / 1024
                    ));
                }
                Ok(_) => {}
                Err(e) => return ActionResult::Err(format!("Cannot read file: {}", e)),
            }
            match std::fs::read(&canonical) {
                Ok(bytes) => {
                    if bytes.len() as u64 > MAX_READ_FILE_BYTES {
                        // TOCTOU: file grew between stat and read.
                        return ActionResult::Err(format!(
                            "File too large ({:.1} MB). Maximum is {} MB.",
                            bytes.len() as f64 / 1024.0 / 1024.0,
                            MAX_READ_FILE_BYTES / 1024 / 1024
                        ));
                    }
                    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    ActionResult::Ok(Some(serde_json::json!({ "content_b64": encoded })))
                }
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
                Ok(m) => {
                    let modified_at_millis = modified_at_millis(&m);
                    ActionResult::Ok(Some(serde_json::json!({
                        "size": m.len(),
                        "modified_at_millis": modified_at_millis,
                    })))
                }
                Err(e) => ActionResult::Err(format!("Cannot read file: {}", e)),
            }
        }
        None => ActionResult::Err(format!("project not found: {}", project_id)),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_content_search(
    ws: &Workspace,
    project_id: String,
    query: String,
    case_sensitive: bool,
    mode: String,
    max_results: usize,
    file_glob: Option<String>,
    context_lines: usize,
    show_ignored: bool,
) -> Result<PreparedContentSearch, String> {
    let project_path = match ws.project(&project_id) {
        Some(project) => project.path.clone(),
        None => return Err(format!("project not found: {project_id}")),
    };
    prepare_content_search_for_path(
        &project_path,
        query,
        case_sensitive,
        mode,
        max_results,
        file_glob,
        context_lines,
        show_ignored,
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_content_search_for_path(
    root: &str,
    query: String,
    case_sensitive: bool,
    mode: String,
    max_results: usize,
    file_glob: Option<String>,
    context_lines: usize,
    show_ignored: bool,
) -> Result<PreparedContentSearch, String> {
    if let Some(ref glob) = file_glob
        && (glob.contains("..") || glob.starts_with('/'))
    {
        return Err("file_glob must not contain '..' or start with '/'".to_string());
    }
    let project_path = canonical_scope_root(root)?;
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
        show_ignored,
    };
    Ok(PreparedContentSearch {
        project_path,
        query,
        config,
    })
}

pub fn execute_prepared_content_search(search: PreparedContentSearch) -> ActionResult {
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    execute_prepared_content_search_with_cancellation(search, &cancelled)
}

pub fn execute_prepared_content_search_with_cancellation(
    search: PreparedContentSearch,
    cancelled: &std::sync::atomic::AtomicBool,
) -> ActionResult {
    let mut results = Vec::new();
    let search_result = okena_files::content_search::search_content(
        &search.project_path,
        &search.query,
        &search.config,
        cancelled,
        &mut |result| results.push(result),
    );
    match search_result {
        Ok(()) => ActionResult::Ok(Some(
            serde_json::to_value(results).expect("BUG: FileSearchResult must serialize"),
        )),
        Err(error) => ActionResult::Err(error),
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
    show_ignored: bool,
) -> ActionResult {
    match prepare_content_search(
        ws,
        project_id,
        query,
        case_sensitive,
        mode,
        max_results,
        file_glob,
        context_lines,
        show_ignored,
    ) {
        Ok(search) => execute_prepared_content_search(search),
        Err(error) => ActionResult::Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn search_path_content(
    root: String,
    query: String,
    case_sensitive: bool,
    mode: String,
    max_results: usize,
    file_glob: Option<String>,
    context_lines: usize,
    show_ignored: bool,
) -> ActionResult {
    match prepare_content_search_for_path(
        &root,
        query,
        case_sensitive,
        mode,
        max_results,
        file_glob,
        context_lines,
        show_ignored,
    ) {
        Ok(search) => execute_prepared_content_search(search),
        Err(error) => ActionResult::Err(error),
    }
}

pub(super) fn rename_file(
    ws: &Workspace,
    project_id: String,
    relative_path: String,
    new_name: String,
) -> ActionResult {
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

pub(super) fn delete_file(
    ws: &Workspace,
    project_id: String,
    relative_path: String,
) -> ActionResult {
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

pub(super) fn rename_path(root: String, relative_path: String, new_name: String) -> ActionResult {
    if let Err(error) = validate_leaf_name(&new_name) {
        return ActionResult::Err(error);
    }
    let root_path = match canonical_scope_root(&root) {
        Ok(root) => root,
        Err(error) => return ActionResult::Err(error),
    };
    let old_path = match resolve_project_file(&root, &relative_path) {
        Ok(path) => path,
        Err(error) => return ActionResult::Err(error),
    };
    if old_path == root_path {
        return ActionResult::Err("cannot rename browser root".to_string());
    }
    let Some(parent) = old_path.parent() else {
        return ActionResult::Err("path has no parent".to_string());
    };
    let new_path = parent.join(&new_name);
    if new_path.exists() {
        return ActionResult::Err(format!("target already exists: {new_name}"));
    }
    match std::fs::rename(old_path, new_path) {
        Ok(()) => ActionResult::Ok(None),
        Err(error) => ActionResult::Err(format!("Cannot rename: {error}")),
    }
}

pub(super) fn delete_path(root: String, relative_path: String) -> ActionResult {
    let root_path = match canonical_scope_root(&root) {
        Ok(root) => root,
        Err(error) => return ActionResult::Err(error),
    };
    let target = match resolve_project_file(&root, &relative_path) {
        Ok(path) => path,
        Err(error) => return ActionResult::Err(error),
    };
    if target == root_path {
        return ActionResult::Err("cannot delete browser root".to_string());
    }
    let result = if target.is_dir() {
        std::fs::remove_dir_all(target)
    } else {
        std::fs::remove_file(target)
    };
    match result {
        Ok(()) => ActionResult::Ok(None),
        Err(error) => ActionResult::Err(format!("Cannot delete: {error}")),
    }
}

pub(super) fn create_file(
    ws: &Workspace,
    project_id: String,
    relative_path: String,
) -> ActionResult {
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
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
    {
        Ok(_) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!("Cannot create file: {}", e)),
    }
}

pub(super) fn create_directory(
    ws: &Workspace,
    project_id: String,
    relative_path: String,
) -> ActionResult {
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

#[cfg(test)]
mod terminal_path_tests {
    use super::{expand_terminal_path, path_breadcrumbs};
    use std::path::{Path, PathBuf};

    #[test]
    fn relative_path_uses_terminal_cwd() {
        assert_eq!(
            expand_terminal_path("notes/release.md", "/srv/project").unwrap(),
            PathBuf::from("/srv/project/notes/release.md")
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_url_decodes_escaped_path() {
        assert_eq!(
            expand_terminal_path("file:///tmp/release%20notes.md", "/ignored").unwrap(),
            PathBuf::from("/tmp/release notes.md")
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_url_host_is_informational_on_the_daemon() {
        assert_eq!(
            expand_terminal_path("file://build-server/tmp/release.md", "/ignored").unwrap(),
            PathBuf::from("/tmp/release.md")
        );
    }

    #[test]
    fn tilde_path_uses_daemon_home() {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .expect("test process should have a home directory");
        assert_eq!(
            expand_terminal_path("~/notes.md", "/ignored").unwrap(),
            PathBuf::from(home).join("notes.md")
        );
    }

    #[cfg(unix)]
    #[test]
    fn breadcrumbs_include_root_and_each_ancestor() {
        let breadcrumbs = path_breadcrumbs(Path::new("/srv/apps/demo"));
        let labels: Vec<&str> = breadcrumbs
            .iter()
            .map(|breadcrumb| breadcrumb.label.as_str())
            .collect();
        assert_eq!(labels, vec!["/", "srv", "apps", "demo"]);
        assert_eq!(breadcrumbs[2].canonical_path, "/srv/apps");
    }
}
