//! What makes a directory an OpenSpec root, and whether a store is usable.
//!
//! Mirrors `openspec-root.ts` (health), `project-config.ts`
//! (`classifyOpenSpecDir`) and `root-selection.ts` (the qualifying walk and
//! registered-store inspection), with the same diagnostic codes.

use crate::OpenSpecError;
use crate::files::{self, ProjectConfig, StoreMetadata};
use crate::paths::{canonical, display};
use okena_core::specs::SpecDiagnostic;
use std::path::{Path, PathBuf};

pub const OPENSPEC_DIR: &str = "openspec";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Missing,
    Dir,
    File,
    Other,
}

fn kind(p: &Path) -> Kind {
    match std::fs::metadata(p) {
        Ok(m) if m.is_dir() => Kind::Dir,
        Ok(m) if m.is_file() => Kind::File,
        Ok(_) => Kind::Other,
        Err(_) => Kind::Missing,
    }
}

/// OpenSpec's `inspectOpenSpecRoot`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootInspection {
    pub openspec_present: bool,
    pub config: Option<PathBuf>,
    /// `openspec/` and a config file present, and nothing malformed.
    pub healthy: bool,
    pub diagnostics: Vec<SpecDiagnostic>,
}

impl RootInspection {
    pub fn problems(&self) -> String {
        if self.diagnostics.is_empty() {
            "OpenSpec root is missing or incomplete.".into()
        } else {
            self.diagnostics
                .iter()
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

pub fn inspect(root: &Path) -> RootInspection {
    let mut out = RootInspection {
        openspec_present: false,
        config: None,
        healthy: false,
        diagnostics: Vec::new(),
    };
    match kind(root) {
        Kind::Dir => {}
        Kind::Missing => {
            out.diagnostics.push(SpecDiagnostic::error(
                "openspec_store_root_missing",
                "Store root does not exist.",
            ));
            return out;
        }
        _ => {
            out.diagnostics.push(SpecDiagnostic::error(
                "openspec_store_root_not_directory",
                "Store root is not a directory.",
            ));
            return out;
        }
    }

    let openspec = root.join(OPENSPEC_DIR);
    match kind(&openspec) {
        Kind::Dir => out.openspec_present = true,
        Kind::Missing => {
            out.diagnostics.push(SpecDiagnostic::error(
                "openspec_root_missing",
                "Missing openspec/ directory.",
            ));
            return out;
        }
        _ => {
            out.diagnostics.push(SpecDiagnostic::error(
                "openspec_root_not_directory",
                "openspec/ exists but is not a directory.",
            ));
            return out;
        }
    }

    let yaml = openspec.join("config.yaml");
    let yml = openspec.join("config.yml");
    match (kind(&yaml), kind(&yml)) {
        (Kind::File, _) => out.config = Some(yaml),
        (_, Kind::File) => out.config = Some(yml),
        (Kind::Missing, Kind::Missing) => out.diagnostics.push(SpecDiagnostic::error(
            "openspec_config_missing",
            "Missing openspec/config.yaml or openspec/config.yml.",
        )),
        _ => out.diagnostics.push(SpecDiagnostic::error(
            "openspec_config_not_file",
            "OpenSpec config path exists but is not a file.",
        )),
    }

    for (rel, code) in [
        ("specs", "openspec_specs_not_directory"),
        ("changes", "openspec_changes_not_directory"),
    ] {
        if !matches!(kind(&openspec.join(rel)), Kind::Dir | Kind::Missing) {
            out.diagnostics.push(SpecDiagnostic::error(
                code,
                format!("openspec/{rel}/ exists but is not a directory."),
            ));
        }
    }
    if kind(&openspec.join("changes")) == Kind::Dir
        && !matches!(
            kind(&openspec.join("changes").join("archive")),
            Kind::Dir | Kind::Missing
        )
    {
        out.diagnostics.push(SpecDiagnostic::error(
            "openspec_archive_not_directory",
            "openspec/changes/archive/ exists but is not a directory.",
        ));
    }

    out.healthy = out.openspec_present && out.config.is_some() && out.diagnostics.is_empty();
    out
}

/// OpenSpec's `classifyOpenSpecDir`: a real planning tree, or a config-only
/// directory (which may point at a store).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classification {
    /// `openspec/specs/` or `openspec/changes/` exists.
    pub has_planning_shape: bool,
    pub config: Option<ProjectConfig>,
}

pub fn classify(root: &Path) -> Classification {
    let openspec = root.join(OPENSPEC_DIR);
    Classification {
        has_planning_shape: openspec.join("specs").is_dir() || openspec.join("changes").is_dir(),
        config: files::read_project_config(root),
    }
}

/// The nearest ancestor of `start` (inclusive) that OpenSpec treats as a root.
///
/// An `openspec/` directory alone does not qualify — it must hold a planning
/// shape or a config file. Without that rule the recommended `~/openspec/<id>`
/// store layout would make `$HOME` a phantom root for everything under it.
pub fn find_qualifying_root(start: &Path) -> Option<PathBuf> {
    let start = canonical(start);
    let mut dir: &Path = if start.is_file() {
        start.parent()?
    } else {
        &start
    };
    loop {
        if dir.join(OPENSPEC_DIR).is_dir() {
            let c = classify(dir);
            if c.has_planning_shape || c.config.is_some() {
                return Some(dir.to_path_buf());
            }
        }
        dir = dir.parent()?;
    }
}

/// Outcome of checking a registered store's checkout, identity first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreInspection {
    Ok {
        root: PathBuf,
        metadata: StoreMetadata,
    },
    MetadataError(OpenSpecError),
    MetadataMissing(PathBuf),
    IdMismatch(String),
    Unhealthy(String),
}

/// OpenSpec's `inspectRegisteredStore`.
pub fn inspect_registered_store(id: &str, root: &Path) -> StoreInspection {
    let metadata = match StoreMetadata::read(root) {
        Ok(Some(m)) => m,
        Ok(None) => return StoreInspection::MetadataMissing(StoreMetadata::path(root)),
        Err(e) => return StoreInspection::MetadataError(e),
    };
    if metadata.id != id {
        return StoreInspection::IdMismatch(metadata.id);
    }
    let health = inspect(root);
    if !health.healthy {
        return StoreInspection::Unhealthy(health.problems());
    }
    StoreInspection::Ok {
        root: canonical(root),
        metadata,
    }
}

impl StoreInspection {
    pub fn is_ok(&self) -> bool {
        matches!(self, StoreInspection::Ok { .. })
    }

    /// Short label, as `references.ts` renders it in warnings.
    pub fn label(&self) -> &'static str {
        match self {
            StoreInspection::Ok { .. } => "ok",
            StoreInspection::MetadataError(_) => "metadata error",
            StoreInspection::MetadataMissing(_) => "metadata missing",
            StoreInspection::IdMismatch(_) => "metadata id mismatch",
            StoreInspection::Unhealthy(_) => "unhealthy root",
        }
    }

    /// The error `resolveStoreRoot` raises for this outcome.
    pub fn error(&self, id: &str, root: &Path) -> Option<OpenSpecError> {
        let doctor = format!("Run openspec store doctor {id} to inspect it.");
        Some(match self {
            StoreInspection::Ok { .. } => return None,
            StoreInspection::MetadataError(e) => e.clone(),
            StoreInspection::MetadataMissing(path) => OpenSpecError::new(
                "store_identity_mismatch",
                format!(
                    "Store '{id}' is missing identity metadata at {}.",
                    display(path)
                ),
            )
            .with_fix(doctor),
            StoreInspection::IdMismatch(actual) => OpenSpecError::new(
                "store_identity_mismatch",
                format!("Store '{id}' metadata id '{actual}' does not match its registered id."),
            )
            .with_fix(doctor),
            StoreInspection::Unhealthy(problems) => OpenSpecError::new(
                "unhealthy_store_root",
                format!(
                    "Store '{id}' does not have a healthy OpenSpec root at {}: {problems}",
                    display(root)
                ),
            )
            .with_fix(doctor),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{healthy_root, store_root, write};

    fn codes(i: &RootInspection) -> Vec<&str> {
        i.diagnostics.iter().map(|d| d.code.as_str()).collect()
    }

    #[test]
    fn a_setup_shaped_root_is_healthy() {
        let dir = tempfile::tempdir().unwrap();
        healthy_root(dir.path());
        let i = inspect(dir.path());
        assert!(i.healthy, "{:?}", i.diagnostics);
        assert!(i.config.unwrap().ends_with("config.yaml"));
    }

    #[test]
    fn health_problems_use_the_cli_codes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(
            codes(&inspect(&root.join("nope"))),
            ["openspec_store_root_missing"]
        );
        assert_eq!(codes(&inspect(root)), ["openspec_root_missing"]);

        std::fs::create_dir_all(root.join("openspec/specs")).unwrap();
        let i = inspect(root);
        assert!(!i.healthy && i.openspec_present);
        assert_eq!(codes(&i), ["openspec_config_missing"]);

        write(&root.join("openspec/config.yml"), "schema: spec-driven\n");
        write(&root.join("openspec/changes"), "a file, not a directory");
        assert_eq!(codes(&inspect(root)), ["openspec_changes_not_directory"]);
    }

    #[test]
    fn a_bare_openspec_directory_is_not_a_root_so_home_never_captures_stores() {
        // ~/openspec/<id> is the recommended store layout. The bare
        // ~/openspec/ above it must not make ~ a root.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("openspec/team-plans")).unwrap();
        let project = home.join("src/app");
        std::fs::create_dir_all(&project).unwrap();
        assert_eq!(find_qualifying_root(&project), None);
    }

    #[test]
    fn the_nearest_qualifying_ancestor_wins() {
        let dir = tempfile::tempdir().unwrap();
        let mono = dir.path().join("mono");
        healthy_root(&mono);
        let pkg = mono.join("packages/web/src");
        std::fs::create_dir_all(&pkg).unwrap();
        assert_eq!(find_qualifying_root(&pkg), Some(canonical(&mono)));

        // A config-only pointer repo qualifies too.
        let web = dir.path().join("web");
        write(
            &web.join("openspec/config.yaml"),
            "schema: spec-driven\nstore: team-plans\n",
        );
        assert_eq!(find_qualifying_root(&web), Some(canonical(&web)));
        let c = classify(&web);
        assert!(!c.has_planning_shape);
        assert_eq!(
            c.config.unwrap().store,
            files::StorePointer::Value("team-plans".into())
        );
    }

    #[test]
    fn registered_store_inspection_checks_identity_before_health() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("s");
        std::fs::create_dir_all(&root).unwrap();
        assert!(matches!(
            inspect_registered_store("a", &root),
            StoreInspection::MetadataMissing(_)
        ));

        write(
            &root.join(".openspec-store/store.yaml"),
            "version: 1\nid: other\n",
        );
        let i = inspect_registered_store("a", &root);
        assert_eq!(i, StoreInspection::IdMismatch("other".into()));
        assert_eq!(i.error("a", &root).unwrap().code, "store_identity_mismatch");

        write(
            &root.join(".openspec-store/store.yaml"),
            "version: 1\nid: a\n",
        );
        let i = inspect_registered_store("a", &root);
        assert!(matches!(i, StoreInspection::Unhealthy(_)));
        assert_eq!(i.error("a", &root).unwrap().code, "unhealthy_store_root");

        store_root(&root, "a");
        assert!(inspect_registered_store("a", &root).is_ok());
    }
}
