//! The planning tree inside one root.
//!
//! Discovery rules follow the CLI's `item-discovery.ts` / `spec-discovery.ts`:
//!
//! - a capability is any `spec.md` under `openspec/specs/` at depth ≥ 1, so
//!   both `specs/<id>/spec.md` and nested `specs/<area>/<id>/spec.md` count,
//!   and its id is the directory path (`platform/session`);
//! - dot-entries are skipped and symlinked directories are not followed; a
//!   symlinked `spec.md` counts only when it resolves inside the specs root or
//!   its own capability directory;
//! - a change is any non-dot directory under `openspec/changes/` other than
//!   `archive` — even one holding only `.openspec.yaml`, which is exactly what
//!   `openspec new change` scaffolds.

use crate::files::read_change_metadata;
use crate::paths::canonical;
use crate::root::OPENSPEC_DIR;
use okena_core::specs::{CHANGE_ARTIFACTS, SpecChange, SpecDoc, SpecTree};
use std::path::{Path, PathBuf};

/// Read the planning tree of `root`. The caller fills in the root key and
/// store id, which this module has no way to know.
pub fn read_tree(root: &Path) -> SpecTree {
    let openspec = root.join(OPENSPEC_DIR);
    // An uninitialized root is a normal state, not an error: it is what a new
    // spec folder looks like before its first change.
    let mut tree = SpecTree {
        root: crate::paths::display(root),
        initialized: openspec.is_dir(),
        ..Default::default()
    };
    if tree.initialized {
        tree.specs = discover_specs(root, &openspec.join("specs"));
        let changes = openspec.join("changes");
        tree.changes = read_active_changes(root, &changes);
        tree.archived = read_archived_changes(root, &changes.join("archive"));
    }
    tree
}

/// Path relative to `root`, in the forward-slash form the wire types use.
pub fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every capability `spec.md` under `specs_dir`, sorted by id.
pub fn discover_specs(root: &Path, specs_dir: &Path) -> Vec<SpecDoc> {
    let mut out = Vec::new();
    let specs_root = canonical(specs_dir);
    walk_specs(root, &specs_root, specs_dir, &mut Vec::new(), &mut out);
    // Code-point order, like the CLI, so ordering never depends on locale.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn walk_specs(
    root: &Path,
    specs_root: &Path,
    dir: &Path,
    segments: &mut Vec<String>,
    out: &mut Vec<SpecDoc>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        // `file_type` does not follow symlinks, which is what keeps a linked
        // directory from being walked.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            segments.push(name);
            walk_specs(root, specs_root, &path, segments, out);
            segments.pop();
        } else if name == "spec.md" && !segments.is_empty() {
            let counts = kind.is_file()
                || (kind.is_symlink()
                    && path.canonicalize().is_ok_and(|real| {
                        real.is_file()
                            && (real.starts_with(specs_root) || real.starts_with(canonical(dir)))
                    }));
            if counts {
                out.push(SpecDoc {
                    path: rel(root, &path),
                    name: segments.join("/"),
                });
            }
        }
    }
}

fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// Active changes, most recently touched first — the change you want is nearly
/// always the one you just made. `archive` is read separately.
fn read_active_changes(root: &Path, dir: &Path) -> Vec<SpecChange> {
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = child_dirs(dir)
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n != "archive"))
        .map(|p| {
            let modified = p
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (modified, p)
        })
        .collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    dirs.into_iter()
        .map(|(_, p)| read_change(root, &p, false))
        .collect()
}

/// Archived changes, newest first. `openspec archive` prefixes the directory
/// with the date, so reverse name order is chronological.
fn read_archived_changes(root: &Path, dir: &Path) -> Vec<SpecChange> {
    let mut dirs = child_dirs(dir);
    dirs.sort_by(|a, b| b.cmp(a));
    dirs.into_iter()
        .map(|p| read_change(root, &p, true))
        .collect()
}

/// Read one change directory.
pub fn read_change(root: &Path, dir: &Path, archived: bool) -> SpecChange {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let doc = |p: &Path| SpecDoc {
        path: rel(root, p),
        name: p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    // Conventional artifacts in reading order (why → how → work), then any
    // other Markdown a custom schema produced, by name.
    let mut artifacts: Vec<SpecDoc> = CHANGE_ARTIFACTS
        .iter()
        .map(|f| dir.join(f))
        .filter(|p| p.is_file())
        .map(|p| doc(&p))
        .collect();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut extra: Vec<SpecDoc> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .filter(|p| {
                p.file_name().is_some_and(|n| {
                    let n = n.to_string_lossy();
                    !n.starts_with('.') && !CHANGE_ARTIFACTS.contains(&n.as_ref())
                })
            })
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md")))
            .map(|p| doc(&p))
            .collect();
        extra.sort_by(|a, b| a.name.cmp(&b.name));
        artifacts.extend(extra);
    }
    let meta = read_change_metadata(dir);
    SpecChange {
        name,
        path: rel(root, dir),
        artifacts,
        specs: discover_specs(root, &dir.join("specs")),
        archived,
        schema: meta.schema,
        created: meta.created,
    }
}

/// Resolve a client-supplied relative path inside `root`.
///
/// Canonicalizes both sides so `..` and symlinks cannot escape: the check has
/// to be on the resolved path, since `openspec/../../.ssh/id_rsa` is a perfectly
/// ordinary-looking string.
pub fn resolve_document(root: &Path, path: &str) -> Result<PathBuf, String> {
    let real = root
        .join(path)
        .canonicalize()
        .map_err(|_| format!("no such document: {path}"))?;
    let real_root = root
        .canonicalize()
        .map_err(|e| format!("spec root is unreadable: {e}"))?;
    if !real.starts_with(&real_root) {
        return Err("path is outside the spec root".into());
    }
    if !real.is_file() {
        return Err(format!("not a file: {path}"));
    }
    Ok(real)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write;

    #[test]
    fn capabilities_are_spec_md_files_at_any_depth_named_by_their_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("openspec/specs/auth/spec.md"), "# Auth");
        write(&root.join("openspec/specs/platform/session/spec.md"), "# S");
        // Not capabilities: a root-level spec.md, stray notes, dot-dirs.
        write(&root.join("openspec/specs/spec.md"), "x");
        write(&root.join("openspec/specs/auth/notes.md"), "x");
        write(&root.join("openspec/specs/.draft/spec.md"), "x");
        write(&root.join("openspec/specs/.gitkeep"), "");

        let specs = discover_specs(root, &root.join("openspec/specs"));
        assert_eq!(
            specs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["auth", "platform/session"]
        );
        assert_eq!(specs[1].path, "openspec/specs/platform/session/spec.md");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directories_are_not_followed_and_escaping_links_do_not_count() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        write(&outside.join("ext/spec.md"), "x");
        write(&outside.join("secret.md"), "x");
        std::fs::create_dir_all(root.join("openspec/specs/linked-file")).unwrap();
        std::os::unix::fs::symlink(outside.join("ext"), root.join("openspec/specs/linked-dir"))
            .unwrap();
        std::os::unix::fs::symlink(
            outside.join("secret.md"),
            root.join("openspec/specs/linked-file/spec.md"),
        )
        .unwrap();
        assert!(discover_specs(&root, &root.join("openspec/specs")).is_empty());
    }

    #[test]
    fn a_change_is_any_directory_even_one_with_only_openspec_yaml() {
        // `openspec new change add-login` writes exactly this and nothing else.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("openspec/changes/add-login/.openspec.yaml"),
            "schema: spec-driven\ncreated: 2026-09-10\n",
        );
        std::fs::create_dir_all(root.join("openspec/changes/.hidden")).unwrap();
        std::fs::create_dir_all(root.join("openspec/changes/archive")).unwrap();

        let tree = read_tree(root);
        assert!(tree.initialized);
        assert_eq!(tree.changes.len(), 1);
        let c = &tree.changes[0];
        assert_eq!(c.name, "add-login");
        assert!(c.artifacts.is_empty());
        assert_eq!(c.schema.as_deref(), Some("spec-driven"));
        assert_eq!(c.created.as_deref(), Some("2026-09-10"));
        assert!(
            tree.archived.is_empty(),
            "archive must not read as a change"
        );
    }

    #[test]
    fn artifacts_list_conventional_files_first_then_custom_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let change = root.join("openspec/changes/c");
        for f in ["tasks.md", "research.md", "proposal.md", "notes.txt"] {
            write(&change.join(f), "x");
        }
        write(&change.join("specs/auth/spec.md"), "x");
        let c = read_change(root, &change, false);
        assert_eq!(
            c.artifacts
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
            ["proposal.md", "tasks.md", "research.md"]
        );
        assert_eq!(c.specs[0].name, "auth");
        assert_eq!(c.specs[0].path, "openspec/changes/c/specs/auth/spec.md");
    }

    #[test]
    fn the_archive_is_newest_first_by_its_date_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["2026-01-02-old", "2026-09-01-new", "2026-03-03-mid"] {
            write(
                &root.join(format!("openspec/changes/archive/{name}/proposal.md")),
                "x",
            );
        }
        let tree = read_tree(root);
        assert_eq!(
            tree.archived
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["2026-09-01-new", "2026-03-03-mid", "2026-01-02-old"]
        );
        assert!(tree.archived.iter().all(|c| c.archived));
    }

    #[test]
    fn a_root_without_openspec_reads_as_uninitialized() {
        let dir = tempfile::tempdir().unwrap();
        let tree = read_tree(dir.path());
        assert!(!tree.initialized);
        assert!(tree.specs.is_empty() && tree.changes.is_empty());
    }

    #[test]
    fn traversal_out_of_the_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        write(&root.join("openspec/specs/auth/spec.md"), "# Auth");
        write(&dir.path().join("secret.md"), "SECRET");

        assert!(resolve_document(&root, "openspec/specs/auth/spec.md").is_ok());
        let err = resolve_document(&root, "openspec/../../secret.md").unwrap_err();
        assert!(err.contains("outside"), "{err}");
        assert!(
            resolve_document(&root, "openspec/specs").is_err(),
            "a directory"
        );
        assert!(resolve_document(&root, "openspec/nope.md").is_err());
    }
}
