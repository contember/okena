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
pub fn prepare_content_search_for_path(
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

/// Resolve an existing entry and describe it. `resolve_new_project_file` canonicalizes and contains
/// the parent while the leaf stays lexical, and `symlink_metadata` then describes the link itself —
/// so a mutation acts on the entry the caller named, never on what it points at.
fn resolve_entry(root: &str, relative_path: &str) -> Result<(PathBuf, std::fs::Metadata), String> {
    let path = resolve_new_project_file(root, relative_path)?;
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|error| format!("Cannot read path: {error}"))?;
    Ok((path, metadata))
}

/// Existence of the entry itself; unlike `Path::exists`, a dangling symlink counts.
fn entry_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

#[cfg(windows)]
fn remove_symlink(target: &Path) -> std::io::Result<()> {
    // Windows unlinks a directory symlink with `remove_dir`, a file symlink with `remove_file`.
    std::fs::remove_file(target)
        .or_else(|file_error| std::fs::remove_dir(target).map_err(|_| file_error))
}

#[cfg(not(windows))]
fn remove_symlink(target: &Path) -> std::io::Result<()> {
    std::fs::remove_file(target)
}

/// Remove the entry at `target` itself — a symlink is unlinked, never followed into its referent.
fn remove_entry(target: &Path, metadata: &std::fs::Metadata) -> std::io::Result<()> {
    if metadata.file_type().is_symlink() {
        remove_symlink(target)
    } else if metadata.is_dir() {
        std::fs::remove_dir_all(target)
    } else {
        std::fs::remove_file(target)
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
    let old_path = match resolve_entry(&project_path, &relative_path) {
        Ok((path, _)) => path,
        Err(e) => return ActionResult::Err(e),
    };
    let parent = match old_path.parent() {
        Some(p) => p,
        None => return ActionResult::Err("cannot rename project root".to_string()),
    };
    let new_path = parent.join(&new_name);
    if entry_exists(&new_path) {
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
    let project_root = match std::path::Path::new(&project_path).canonicalize() {
        Ok(r) => r,
        Err(e) => return ActionResult::Err(format!("Cannot resolve project path: {}", e)),
    };
    // The diff viewer is the caller, and its paths share the git surface's worktree-root base.
    let (git_root, _) = okena_git::resolve_git_root_and_subdir(&project_root);
    let Some(root) = git_root.to_str() else {
        return ActionResult::Err("Repository root path is not valid UTF-8".to_string());
    };
    let (target, metadata) = match resolve_entry(root, &relative_path) {
        Ok(entry) => entry,
        Err(e) => return ActionResult::Err(e),
    };
    // Compare on disk: the leaf is lexical, so a case-folding filesystem would slip a byte-unequal
    // spelling of the project root past equality. A symlink is exempt — unlinking it is not that.
    if !metadata.file_type().is_symlink()
        && target
            .canonicalize()
            .is_ok_and(|resolved| resolved == project_root)
    {
        return ActionResult::Err("cannot delete project root".to_string());
    }
    // The diff viewer, the only producer, never sends a folder; keep `remove_dir_all` off this base.
    if metadata.is_dir() {
        return ActionResult::Err("cannot delete a directory".to_string());
    }
    match remove_entry(&target, &metadata) {
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
    let old_path = match resolve_entry(&root, &relative_path) {
        Ok((path, _)) => path,
        Err(error) => return ActionResult::Err(error),
    };
    if old_path == root_path {
        return ActionResult::Err("cannot rename browser root".to_string());
    }
    let Some(parent) = old_path.parent() else {
        return ActionResult::Err("path has no parent".to_string());
    };
    let new_path = parent.join(&new_name);
    if entry_exists(&new_path) {
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
    let (target, metadata) = match resolve_entry(&root, &relative_path) {
        Ok(entry) => entry,
        Err(error) => return ActionResult::Err(error),
    };
    if target == root_path {
        return ActionResult::Err("cannot delete browser root".to_string());
    }
    match remove_entry(&target, &metadata) {
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

#[cfg(test)]
mod entry_mutation_tests {
    use super::{ActionResult, delete_file, rename_file};
    #[cfg(unix)]
    use super::{delete_path, rename_path};
    use crate::workspace::state::{ProjectData, WindowState, Workspace, WorkspaceData};
    use okena_core::theme::FolderColor;
    use okena_workspace::settings::HooksConfig;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A temp directory removed even when an assertion panics mid-test.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let base = std::env::temp_dir().join(format!(
                "okena-files-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&base).expect("create fixture root");
            Self(base.canonicalize().expect("canonicalize fixture root"))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    fn expect_ok(result: ActionResult) {
        if let ActionResult::Err(error) = result {
            panic!("action failed: {error}");
        }
    }

    fn expect_err(result: ActionResult) -> String {
        match result {
            ActionResult::Err(error) => error,
            ActionResult::Ok(_) => panic!("action succeeded but should have been refused"),
        }
    }

    fn workspace_with_project(path: &Path) -> Workspace {
        let project = ProjectData {
            task_ref: None,
            spec_change: None,
            custom_session: None,
            agent: None,
            id: "p1".to_string(),
            name: "Project".to_string(),
            path: path.to_string_lossy().into_owned(),
            layout: None,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
            creating_progress: None,
        };
        Workspace::new(WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["p1".to_string()],
            folders: Vec::new(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        })
    }

    /// A repository at `<fixture>/repo` whose project lives in the `packages/app` subdirectory.
    fn subdirectory_project(fixture: &Fixture) -> PathBuf {
        let repo = fixture.path().join("repo");
        let project = repo.join("packages").join("app");
        fs::create_dir_all(&project).expect("create project dir");
        let init = Command::new("git")
            .current_dir(&repo)
            .args(["init", "--quiet"])
            .output()
            .expect("run git init");
        assert!(
            init.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        );
        project
    }

    #[cfg(unix)]
    #[test]
    fn delete_path_unlinks_a_directory_symlink_and_keeps_its_referent() {
        let fixture = Fixture::new();
        let root = fixture.path();
        let referent = root.join("real");
        fs::create_dir(&referent).expect("create referent dir");
        fs::write(referent.join("keep.txt"), "keep").expect("write referent file");
        std::os::unix::fs::symlink(&referent, root.join("link")).expect("create dir symlink");

        expect_ok(delete_path(
            root.to_string_lossy().into_owned(),
            "link".to_string(),
        ));

        assert!(
            referent.join("keep.txt").exists(),
            "the referent tree must survive"
        );
        assert!(
            fs::symlink_metadata(root.join("link")).is_err(),
            "the link itself must be gone"
        );
    }

    #[cfg(unix)]
    #[test]
    fn delete_path_unlinks_a_file_symlink_and_keeps_its_referent() {
        let fixture = Fixture::new();
        let root = fixture.path();
        let referent = root.join("real.txt");
        fs::write(&referent, "keep").expect("write referent file");
        std::os::unix::fs::symlink(&referent, root.join("link.txt")).expect("create file symlink");

        expect_ok(delete_path(
            root.to_string_lossy().into_owned(),
            "link.txt".to_string(),
        ));

        assert_eq!(
            fs::read_to_string(&referent).expect("referent must survive"),
            "keep"
        );
        assert!(
            fs::symlink_metadata(root.join("link.txt")).is_err(),
            "the link itself must be gone"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rename_path_moves_the_link_and_leaves_its_referent_in_place() {
        let fixture = Fixture::new();
        let root = fixture.path();
        let referent = root.join("real.txt");
        fs::write(&referent, "keep").expect("write referent file");
        std::os::unix::fs::symlink(&referent, root.join("link.txt")).expect("create file symlink");

        expect_ok(rename_path(
            root.to_string_lossy().into_owned(),
            "link.txt".to_string(),
            "moved.txt".to_string(),
        ));

        assert!(
            fs::symlink_metadata(&referent)
                .expect("referent must survive")
                .is_file(),
            "the referent must stay a regular file at its own path"
        );
        assert!(
            fs::symlink_metadata(root.join("moved.txt"))
                .expect("renamed entry must exist")
                .is_symlink(),
            "the renamed entry must still be the link"
        );
        assert!(fs::symlink_metadata(root.join("link.txt")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn delete_file_unlinks_a_symlink_instead_of_its_referent() {
        let fixture = Fixture::new();
        let root = fixture.path();
        let referent = root.join("real.txt");
        fs::write(&referent, "keep").expect("write referent file");
        std::os::unix::fs::symlink(&referent, root.join("link.txt")).expect("create file symlink");
        let ws = workspace_with_project(root);

        expect_ok(delete_file(&ws, "p1".to_string(), "link.txt".to_string()));

        assert_eq!(
            fs::read_to_string(&referent).expect("referent must survive"),
            "keep"
        );
        assert!(
            fs::symlink_metadata(root.join("link.txt")).is_err(),
            "the link itself must be gone"
        );
    }

    #[test]
    fn delete_file_resolves_paths_against_the_git_worktree_root() {
        let fixture = Fixture::new();
        let project = subdirectory_project(&fixture);

        let intended = project.join("fresh.txt");
        fs::write(&intended, "intended").expect("write intended file");
        let decoy_dir = project.join("packages").join("app");
        fs::create_dir_all(&decoy_dir).expect("create decoy dir");
        let decoy = decoy_dir.join("fresh.txt");
        fs::write(&decoy, "decoy").expect("write decoy file");

        let ws = workspace_with_project(&project);
        expect_ok(delete_file(
            &ws,
            "p1".to_string(),
            "packages/app/fresh.txt".to_string(),
        ));

        assert!(
            !intended.exists(),
            "a diff-viewer path must delete the worktree-root file"
        );
        assert!(
            decoy.exists(),
            "the project-relative twin must be left untouched"
        );
    }

    #[test]
    fn delete_file_refuses_a_directory_reached_through_the_worktree_root() {
        let fixture = Fixture::new();
        let project = subdirectory_project(&fixture);
        let repo = project
            .parent()
            .and_then(Path::parent)
            .expect("repo root")
            .to_path_buf();
        let ws = workspace_with_project(&project);

        for (relative_path, directory) in [
            (".git", repo.join(".git")),
            ("packages", repo.join("packages")),
        ] {
            let error = expect_err(delete_file(
                &ws,
                "p1".to_string(),
                relative_path.to_string(),
            ));
            assert!(
                error.contains("cannot delete"),
                "{relative_path} gave: {error}"
            );
            assert!(
                directory.is_dir(),
                "{relative_path} must still exist after the refusal"
            );
        }
    }

    #[test]
    fn delete_file_refuses_the_project_root_reached_through_the_worktree_root() {
        let fixture = Fixture::new();
        let project = subdirectory_project(&fixture);
        let ws = workspace_with_project(&project);

        let error = expect_err(delete_file(
            &ws,
            "p1".to_string(),
            "packages/app".to_string(),
        ));

        assert_eq!(error, "cannot delete project root");
        assert!(project.is_dir(), "the project directory must survive");
    }

    #[test]
    fn rename_file_refuses_an_empty_or_dot_leaf() {
        let fixture = Fixture::new();
        let root = fixture.path().join("project");
        fs::create_dir(&root).expect("create project dir");
        let ws = workspace_with_project(&root);

        for relative_path in ["", "."] {
            let error = expect_err(rename_file(
                &ws,
                "p1".to_string(),
                relative_path.to_string(),
                "renamed".to_string(),
            ));
            assert!(!error.is_empty(), "{relative_path:?} must be refused");
            assert!(
                root.is_dir(),
                "the project directory must keep its name after {relative_path:?}"
            );
        }
    }
}
