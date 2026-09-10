//! Everything okena can show as an OpenSpec root, found where OpenSpec looks.
//!
//! Three sources, deduplicated by canonical path:
//!
//! 1. **Stores** in the machine registry (`openspec store list`).
//! 2. **Projects** — okena projects, resolved the way the CLI resolves a
//!    working directory: walk up to the nearest qualifying root; a real
//!    planning tree is a root of its own, a config-only `store:` pointer
//!    resolves to that store.
//! 3. **Folders** listed in okena's settings.
//!
//! Each root's `references:` are then resolved against the registry, and every
//! problem lands as a diagnostic instead of an error, so one broken store never
//! hides the others.

use crate::files::{self, ProjectConfig, StoreMetadata, StorePointer};
use crate::paths::{canonical, display, expand_home};
use crate::registry::{self, RegisteredStore};
use crate::root::{self, StoreInspection};
use crate::{OpenSpecDirs, validate_store_id};
use okena_core::specs::{
    SpecDiagnostic, SpecPointer, SpecReference, SpecRoot, SpecRootKind, SpecSeverity, SpecStores,
    is_kebab_id, path_root_key, store_root_key,
};
use std::path::Path;

/// An okena project to check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSource {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    /// List every registered store. When false, a store is still listed if a
    /// project points at it, since that is where the project's specs live.
    pub registry: bool,
    /// Projects to check. Empty when project discovery is off.
    pub projects: Vec<ProjectSource>,
    pub folders: Vec<String>,
}

pub fn discover(dirs: &OpenSpecDirs, sources: &Sources) -> SpecStores {
    let mut out = SpecStores {
        registry_path: display(&dirs.registry_path()),
        config_path: display(&dirs.config_path()),
        ..Default::default()
    };

    // Read even when stores are not listed: pointers and references resolve
    // through it. `None` means unreadable, which is different from empty.
    let registered: Option<Vec<RegisteredStore>> = match registry::read(dirs) {
        Ok(r) => Some(r.as_ref().map(registry::entries).unwrap_or_default()),
        Err(e) => {
            out.status.push(e.to_diagnostic());
            None
        }
    };

    match files::read_default_store(&dirs.config_path()) {
        Ok(d) => out.default_store = d,
        Err(e) => out.status.push(SpecDiagnostic {
            severity: SpecSeverity::Warning,
            ..e.to_diagnostic()
        }),
    }

    let mut stores: Vec<SpecRoot> = registered
        .iter()
        .flatten()
        .map(|s| store_root(s, out.default_store.as_deref()))
        .collect();

    let mut projects: Vec<SpecRoot> = Vec::new();
    for project in &sources.projects {
        let Some(found) = root::find_qualifying_root(&expand_home(&project.path)) else {
            continue;
        };
        let found = canonical(&found);
        let class = root::classify(&found);
        if !class.has_planning_shape
            && let Some(config) = &class.config
            && config.store != StorePointer::Absent
        {
            out.pointers.push(resolve_pointer(
                &project.name,
                &found,
                config,
                registered.as_deref(),
                &mut stores,
            ));
            continue;
        }
        let key = display(&found);
        if let Some(existing) = stores
            .iter_mut()
            .chain(projects.iter_mut())
            .find(|r| r.path == key)
        {
            add_unique(&mut existing.used_by, &project.name);
            continue;
        }
        let mut r = local_root(
            SpecRootKind::Project,
            project.name.clone(),
            &found,
            registered.as_deref(),
        );
        if let Some(config) = &class.config
            && let StorePointer::Value(id) = &config.store
        {
            r.status.push(
                SpecDiagnostic::warning(
                    "store_pointer_ignored",
                    format!(
                        "{} declares store '{id}', but this directory is a real OpenSpec root; the declaration is ignored.",
                        display(&config.path)
                    ),
                )
                .with_fix("Remove the store: line, or move this planning tree into the store."),
            );
        }
        projects.push(r);
    }

    let mut folders: Vec<SpecRoot> = Vec::new();
    for folder in &sources.folders {
        let raw = expand_home(folder);
        let name = raw
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder.clone());
        if !raw.is_dir() {
            folders.push(missing_folder(name, &raw));
            continue;
        }
        let found = canonical(&raw);
        let key = display(&found);
        if stores
            .iter()
            .chain(projects.iter())
            .chain(folders.iter())
            .any(|r| r.path == key)
        {
            continue;
        }
        folders.push(local_root(
            SpecRootKind::Folder,
            name,
            &found,
            registered.as_deref(),
        ));
    }

    if !sources.registry {
        stores.retain(|s| !s.used_by.is_empty());
    }

    for r in stores
        .iter_mut()
        .chain(projects.iter_mut())
        .chain(folders.iter_mut())
    {
        r.references = resolve_references(r, registered.as_deref());
    }

    if let (Some(id), Some(list)) = (&out.default_store, &registered)
        && !list.iter().any(|s| &s.id == id)
    {
        out.status.push(
            SpecDiagnostic::warning(
                "unknown_store",
                format!("Global defaultStore '{id}' is not registered on this machine."),
            )
            .with_fix(format!(
                "Register the store (openspec store register <path> --id {id}) or clear the stale global default (openspec config unset defaultStore)."
            )),
        );
    }

    out.roots = stores;
    out.roots.extend(projects);
    out.roots.extend(folders);
    out
}

fn add_unique(list: &mut Vec<String>, name: &str) {
    if !list.iter().any(|n| n == name) {
        list.push(name.to_string());
    }
}

fn schema_of(path: &Path) -> Option<String> {
    files::read_project_config(path).and_then(|c| c.schema)
}

fn store_root(store: &RegisteredStore, default_store: Option<&str>) -> SpecRoot {
    let inspection = root::inspect_registered_store(&store.id, &store.root);
    let path = canonical(&store.root);
    let mut r = SpecRoot {
        key: store_root_key(&store.id),
        kind: SpecRootKind::Store,
        name: store.id.clone(),
        path: display(&path),
        store_id: Some(store.id.clone()),
        remote: store.remote.clone(),
        schema: schema_of(&path),
        healthy: inspection.is_ok(),
        is_default: default_store == Some(store.id.as_str()),
        references: Vec::new(),
        used_by: Vec::new(),
        status: Vec::new(),
    };
    match &inspection {
        // The team-authored canonical remote beats the observed origin.
        StoreInspection::Ok { metadata, .. } => {
            if metadata.remote.is_some() {
                r.remote = metadata.remote.clone();
            }
        }
        other => {
            if let Some(e) = other.error(&store.id, &store.root) {
                r.status.push(e.to_diagnostic());
            }
        }
    }
    r
}

fn local_root(
    kind: SpecRootKind,
    name: String,
    path: &Path,
    registered: Option<&[RegisteredStore]>,
) -> SpecRoot {
    let inspection = root::inspect(path);
    let mut healthy = true;
    let mut status = Vec::new();
    for d in inspection.diagnostics {
        match d.code.as_str() {
            // The CLI still works in a planning tree that has no config (it is
            // the nearest root), and a folder without openspec/ is where the
            // first change will go — both are worth saying, neither blocks.
            "openspec_config_missing" => status.push(
                SpecDiagnostic::warning(&d.code, d.message)
                    .with_fix("Run openspec init here, or add openspec/config.yaml containing `schema: spec-driven`."),
            ),
            "openspec_root_missing" => status.push(SpecDiagnostic::warning(
                &d.code,
                "No openspec/ directory yet — drafting a change creates one.",
            )),
            _ => {
                healthy = false;
                status.push(d);
            }
        }
    }

    // A store checkout that is not registered here — offer to register it.
    let store_id = StoreMetadata::read(path).ok().flatten().map(|m| m.id);
    if let Some(id) = &store_id {
        let elsewhere = registered
            .into_iter()
            .flatten()
            .find(|s| &s.id == id)
            .map(|s| display(&s.root));
        status.push(match elsewhere {
            Some(other) => SpecDiagnostic::warning(
                "store_id_conflict",
                format!(
                    "This is a second checkout of store '{id}', which is registered at {other}."
                ),
            )
            .with_fix(format!(
                "Run openspec store unregister {id} first to use this checkout instead."
            )),
            None => SpecDiagnostic::warning(
                "store_unregistered",
                format!("This folder is store '{id}', but it is not registered on this machine."),
            )
            .with_fix(format!("openspec store register '{}'", display(path))),
        });
    }

    SpecRoot {
        key: path_root_key(&display(path)),
        kind,
        name,
        path: display(path),
        store_id,
        remote: None,
        schema: schema_of(path),
        healthy,
        is_default: false,
        references: Vec::new(),
        used_by: Vec::new(),
        status,
    }
}

fn missing_folder(name: String, path: &Path) -> SpecRoot {
    SpecRoot {
        key: path_root_key(&display(path)),
        kind: SpecRootKind::Folder,
        name,
        path: display(path),
        store_id: None,
        remote: None,
        schema: None,
        healthy: false,
        is_default: false,
        references: Vec::new(),
        used_by: Vec::new(),
        status: vec![
            SpecDiagnostic::error(
                "folder_missing",
                format!("Folder not found: {}", display(path)),
            )
            .with_fix("Fix the path in Settings → Specs, or remove it."),
        ],
    }
}

/// A project whose `openspec/config.yaml` points at a store.
fn resolve_pointer(
    project: &str,
    path: &Path,
    config: &ProjectConfig,
    registered: Option<&[RegisteredStore]>,
    stores: &mut [SpecRoot],
) -> SpecPointer {
    let file = display(&config.path);
    let mut p = SpecPointer {
        project: project.to_string(),
        path: display(path),
        store_id: String::new(),
        root_key: None,
        status: Vec::new(),
    };
    match &config.store {
        StorePointer::Absent => {}
        StorePointer::Malformed(problem) => p.status.push(
            SpecDiagnostic::error(
                "invalid_store_pointer",
                format!(
                    "Invalid store declaration in {file}: {}.",
                    problem.describe()
                ),
            )
            .with_fix(match problem {
                files::PointerProblem::Unparseable => format!("Fix the YAML syntax in {file}."),
                files::PointerProblem::NonString => {
                    format!("Edit {file} so the store key is a registered store id, or remove it.")
                }
            }),
        ),
        StorePointer::Value(id) => {
            p.store_id = id.clone();
            if let Err(e) = validate_store_id(id) {
                p.status.push(e.to_diagnostic());
            } else {
                match registered {
                    None => p.status.push(
                        SpecDiagnostic::error(
                            "invalid_store_registry",
                            format!("Declared in {file}: store '{id}' cannot be resolved because the store registry is unreadable."),
                        )
                        .with_fix("Run openspec store doctor"),
                    ),
                    Some(list) if !list.iter().any(|s| &s.id == id) => {
                        let code = if list.is_empty() { "no_registered_stores" } else { "unknown_store" };
                        p.status.push(
                            SpecDiagnostic::error(code, format!("Declared in {file}: unknown store '{id}'."))
                                .with_fix(format!(
                                    "Register the store (openspec store register <path> --id {id}) or edit {file} to name a registered store."
                                )),
                        );
                    }
                    Some(_) => {
                        if let Some(store) = stores.iter_mut().find(|s| s.store_id.as_deref() == Some(id)) {
                            add_unique(&mut store.used_by, project);
                            if store.healthy {
                                p.root_key = Some(store.key.clone());
                            } else {
                                p.status.push(
                                    SpecDiagnostic::error(
                                        "unhealthy_store_root",
                                        format!("Declared in {file}: store '{id}' is registered but not usable."),
                                    )
                                    .with_fix(format!("Run openspec store doctor {id} to inspect it.")),
                                );
                            }
                        }
                    }
                }
            }
            // The CLI reads the resolved store's config, so references here
            // do nothing — and says so.
            if !config.references.is_empty() {
                p.status.push(
                    SpecDiagnostic::warning(
                        "pointer_declarations_inert",
                        format!(
                            "{file} declares references, but commands read the resolved store's config — these declarations are inert."
                        ),
                    )
                    .with_fix("Move the references declarations into the store's openspec/config.yaml."),
                );
            }
        }
    }
    p
}

/// OpenSpec's `assembleReferenceIndex` in health mode: resolution facts only,
/// every failure a warning.
fn resolve_references(
    root: &SpecRoot,
    registered: Option<&[RegisteredStore]>,
) -> Vec<SpecReference> {
    let Some(config) = files::read_project_config(Path::new(&root.path)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for declaration in config.references {
        let id = declaration.id;
        let mut r = SpecReference {
            id: id.clone(),
            remote: declaration.remote.clone(),
            root: None,
            status: Vec::new(),
        };
        let warning = |code: &str, message: String, fix: String| {
            SpecDiagnostic::warning(code, message).with_fix(fix)
        };
        if !is_kebab_id(&id) {
            r.status.push(warning(
                "reference_invalid_id",
                format!("Reference '{id}' is not a valid store id."),
                "Use kebab-case store ids in the references list.".into(),
            ));
            out.push(r);
            continue;
        }
        if root.kind == SpecRootKind::Store && root.store_id.as_deref() == Some(id.as_str()) {
            continue;
        }
        let Some(list) = registered else {
            r.status.push(warning(
                "reference_registry_unreadable",
                format!(
                    "Referenced store '{id}' cannot be checked: the store registry is unreadable."
                ),
                "Run: openspec store doctor".into(),
            ));
            out.push(r);
            continue;
        };
        let Some(store) = list.iter().find(|s| s.id == id) else {
            r.status.push(warning(
                "reference_unresolved",
                format!("Referenced store '{id}' is not registered on this machine."),
                register_fix(&id, declaration.remote.as_deref()),
            ));
            out.push(r);
            continue;
        };
        match root::inspect_registered_store(&id, &store.root) {
            StoreInspection::Ok { root: real, .. } => {
                if display(&real) == root.path {
                    continue;
                }
                r.root = Some(display(&real));
            }
            other => r.status.push(warning(
                "reference_root_unhealthy",
                format!(
                    "Referenced store '{id}' is registered but not usable ({}).",
                    other.label()
                ),
                format!("Run: openspec store doctor {id}"),
            )),
        }
        out.push(r);
    }
    out
}

/// How to get an unregistered referenced store onto this machine — a pasteable
/// command when the declared remote is shell-inert, prose otherwise.
pub fn register_fix(id: &str, remote: Option<&str>) -> String {
    match remote.filter(|r| is_shell_safe_remote(r)) {
        Some(remote) => {
            let checkout = dirs::home_dir()
                .unwrap_or_default()
                .join("openspec")
                .join(id);
            let quoted = if cfg!(windows) {
                format!("\"{}\"", display(&checkout))
            } else {
                format!("'{}'", display(&checkout))
            };
            format!("git clone -- {remote} {quoted} && openspec store register {quoted} --id {id}")
        }
        None => format!(
            "Get a checkout from a teammate and run: openspec store register <path> --id {id}"
        ),
    }
}

/// No whitespace, quotes or metacharacters, and not flag-like: a config-supplied
/// `--upload-pack=…` must never reach a command someone pastes.
fn is_shell_safe_remote(remote: &str) -> bool {
    !remote.is_empty()
        && !remote.starts_with('-')
        && remote
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@:/._~+-".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Sandbox, healthy_root, store_root, write};

    fn project(name: &str, path: &Path) -> ProjectSource {
        ProjectSource {
            name: name.into(),
            path: display(path),
        }
    }

    fn codes(d: &[SpecDiagnostic]) -> Vec<&str> {
        d.iter().map(|d| d.code.as_str()).collect()
    }

    /// The team layout from the stores docs: a registered store, a code repo
    /// pointing at it, a repo with its own planning, and a default store.
    fn team(sb: &Sandbox) {
        let store = sb.path("stores/team-plans");
        store_root(&store, "team-plans");
        write(
            &store.join("openspec/config.yaml"),
            "schema: spec-driven\nreferences:\n  - design-system\n  - { id: ghost, remote: \"git@github.com:acme/ghost.git\" }\n",
        );
        let design = sb.path("stores/design-system");
        store_root(&design, "design-system");
        registry::register(&sb.dirs, &display(&store), None).unwrap();
        registry::register(&sb.dirs, &display(&design), None).unwrap();
        files::write_default_store(&sb.dirs.config_path(), Some("team-plans")).unwrap();

        write(
            &sb.path("repos/web/openspec/config.yaml"),
            "schema: spec-driven\nstore: team-plans\n",
        );
        healthy_root(&sb.path("repos/api"));
    }

    #[test]
    fn stores_projects_and_pointers_are_discovered_like_the_cli_resolves_them() {
        let sb = Sandbox::new();
        team(&sb);
        let s = discover(
            &sb.dirs,
            &Sources {
                registry: true,
                projects: vec![
                    project("web", &sb.path("repos/web/src")),
                    project("api", &sb.path("repos/api")),
                    project("no-specs", &sb.path("repos")),
                ],
                folders: Vec::new(),
            },
        );
        assert!(s.status.is_empty(), "{:?}", s.status);
        assert_eq!(s.default_store.as_deref(), Some("team-plans"));
        assert_eq!(
            s.roots.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            [
                "store:design-system".to_string(),
                "store:team-plans".to_string(),
                path_root_key(&display(&canonical(&sb.path("repos/api")))),
            ]
        );

        let team_plans = s.root("store:team-plans").unwrap();
        assert!(team_plans.healthy && team_plans.is_default);
        assert_eq!(team_plans.used_by, ["web"]);
        assert_eq!(s.default_root().unwrap().key, "store:team-plans");

        // References: one resolves, one needs a clone.
        let refs = &team_plans.references;
        assert_eq!(refs[0].id, "design-system");
        assert!(refs[0].root.is_some() && refs[0].status.is_empty());
        assert_eq!(codes(&refs[1].status), ["reference_unresolved"]);
        assert!(
            refs[1].status[0]
                .fix
                .as_deref()
                .unwrap()
                .starts_with("git clone -- git@github.com:acme/ghost.git")
        );

        assert_eq!(s.pointers.len(), 1);
        assert_eq!(s.pointers[0].project, "web");
        assert_eq!(s.pointers[0].root_key.as_deref(), Some("store:team-plans"));
    }

    #[test]
    fn a_pointer_at_an_unregistered_store_is_reported_not_dropped() {
        let sb = Sandbox::new();
        write(
            &sb.path("web/openspec/config.yaml"),
            "schema: spec-driven\nstore: team-plans\nreferences: [x]\n",
        );
        let s = discover(
            &sb.dirs,
            &Sources {
                registry: true,
                projects: vec![project("web", &sb.path("web"))],
                folders: Vec::new(),
            },
        );
        assert!(s.roots.is_empty());
        assert_eq!(
            codes(&s.pointers[0].status),
            ["no_registered_stores", "pointer_declarations_inert"]
        );
    }

    #[test]
    fn a_planning_tree_ignores_its_store_pointer_with_a_warning() {
        let sb = Sandbox::new();
        let repo = sb.path("repo");
        healthy_root(&repo);
        write(
            &repo.join("openspec/config.yaml"),
            "schema: spec-driven\nstore: team-plans\n",
        );
        let s = discover(
            &sb.dirs,
            &Sources {
                registry: true,
                projects: vec![project("repo", &repo)],
                folders: Vec::new(),
            },
        );
        assert!(s.pointers.is_empty());
        assert_eq!(codes(&s.roots[0].status), ["store_pointer_ignored"]);
    }

    #[test]
    fn folders_dedupe_against_stores_and_report_unregistered_checkouts() {
        let sb = Sandbox::new();
        team(&sb);
        let clone = sb.path("clones/other-store");
        store_root(&clone, "other-store");
        let fresh = sb.path("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        let s = discover(
            &sb.dirs,
            &Sources {
                registry: true,
                projects: Vec::new(),
                folders: vec![
                    display(&sb.path("stores/team-plans")),
                    display(&clone),
                    display(&fresh),
                    display(&sb.path("gone")),
                ],
            },
        );
        let folders: Vec<&SpecRoot> = s
            .roots
            .iter()
            .filter(|r| r.kind == SpecRootKind::Folder)
            .collect();
        assert_eq!(folders.len(), 3, "the registered store is not listed twice");
        assert_eq!(folders[0].store_id.as_deref(), Some("other-store"));
        assert_eq!(codes(&folders[0].status), ["store_unregistered"]);
        assert!(
            folders[1].healthy,
            "an empty folder is where the first change goes"
        );
        assert_eq!(codes(&folders[1].status), ["openspec_root_missing"]);
        assert!(!folders[2].healthy);
        assert_eq!(codes(&folders[2].status), ["folder_missing"]);
    }

    #[test]
    fn with_registry_listing_off_only_stores_in_use_are_shown() {
        let sb = Sandbox::new();
        team(&sb);
        let s = discover(
            &sb.dirs,
            &Sources {
                registry: false,
                projects: vec![project("web", &sb.path("repos/web"))],
                folders: Vec::new(),
            },
        );
        assert_eq!(
            s.roots.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            ["store:team-plans"]
        );
    }

    #[test]
    fn broken_state_degrades_to_diagnostics() {
        let sb = Sandbox::new();
        team(&sb);
        // A registered store whose checkout vanished, and a stale default.
        std::fs::remove_dir_all(sb.path("stores/design-system")).unwrap();
        files::write_default_store(&sb.dirs.config_path(), Some("nope")).unwrap();
        let s = discover(
            &sb.dirs,
            &Sources {
                registry: true,
                ..Default::default()
            },
        );
        let design = s.root("store:design-system").unwrap();
        assert!(!design.healthy);
        assert_eq!(codes(&design.status), ["store_identity_mismatch"]);
        assert_eq!(codes(&s.status), ["unknown_store"]);
        let refs = &s.root("store:team-plans").unwrap().references;
        assert_eq!(codes(&refs[0].status), ["reference_root_unhealthy"]);

        write(&sb.dirs.registry_path(), "nonsense: [");
        let s = discover(
            &sb.dirs,
            &Sources {
                registry: true,
                ..Default::default()
            },
        );
        assert!(s.roots.is_empty());
        assert_eq!(codes(&s.status), ["invalid_store_registry"]);
    }

    #[test]
    fn unsafe_remotes_never_reach_a_pasteable_command() {
        assert!(
            register_fix("a", Some("--upload-pack=touch /tmp/x")).starts_with("Get a checkout")
        );
        assert!(register_fix("a", Some("git@x:a.git; rm -rf ~")).starts_with("Get a checkout"));
        assert!(
            register_fix("a", Some("https://github.com/acme/a.git")).starts_with("git clone -- ")
        );
    }
}
