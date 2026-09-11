//! A project repo's `.okena/knowledge.yaml`.
//!
//! ```yaml
//! stores: [acme-eng]      # org stores this repo follows
//! root: .okena/knowledge  # where this repo's own kind folders live (default)
//! ```
//!
//! Both keys are optional, and so is the file: a repo with a
//! `.okena/knowledge/` folder has a project root without saying so.

use crate::{KnowledgeError, canonical, display};
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

pub const PROJECT_CONFIG: &str = ".okena/knowledge.yaml";
pub const DEFAULT_PROJECT_ROOT: &str = ".okena/knowledge";

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ProjectConfig {
    /// Ids of the stores this repo follows.
    #[serde(default)]
    pub stores: Vec<String>,
    /// This repo's own kind folders, relative to the repo root.
    #[serde(default)]
    pub root: Option<String>,
}

/// Read `repo`'s config. `Ok(None)` when it has none.
pub fn read_config(repo: &Path) -> Result<Option<ProjectConfig>, KnowledgeError> {
    let path = repo.join(PROJECT_CONFIG);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(KnowledgeError::new(
                "project_config_unreadable",
                format!("Could not read {}: {e}", display(&path)),
            ));
        }
    };
    // An empty file is a valid "nothing configured".
    if text.trim().is_empty() {
        return Ok(Some(ProjectConfig::default()));
    }
    serde_yaml_ng::from_str(&text).map(Some).map_err(|e| {
        KnowledgeError::new(
            "project_config_invalid",
            format!("{} is not valid: {e}", display(&path)),
        )
        .with_fix("Use `stores: [<store-id>, …]` and optionally `root: <relative path>`.")
    })
}

/// Where `repo`'s own kind folders live, when that directory exists.
///
/// `root:` must stay inside the repo — lexically, and again after symlinks
/// resolve — so a config file cannot point okena at arbitrary directories.
pub fn project_root(
    repo: &Path,
    config: Option<&ProjectConfig>,
) -> Result<Option<PathBuf>, KnowledgeError> {
    let configured = config.and_then(|c| c.root.as_deref()).map(str::trim);
    let rel = configured
        .filter(|r| !r.is_empty())
        .unwrap_or(DEFAULT_PROJECT_ROOT);
    let outside = || {
        KnowledgeError::new(
            "project_root_outside",
            format!(
                "`root: {rel}` in {} points outside the repository.",
                display(&repo.join(PROJECT_CONFIG))
            ),
        )
        .with_fix("Use a path inside the repository, e.g. `root: docs/knowledge`.")
    };
    let escapes = Path::new(rel)
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
    if escapes {
        return Err(outside());
    }
    let dir = repo.join(rel);
    if !dir.is_dir() {
        return Ok(None);
    }
    if !canonical(&dir).starts_with(canonical(repo)) {
        return Err(outside());
    }
    Ok(Some(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write;

    #[test]
    fn config_reads_stores_and_root_and_tolerates_absence_and_emptiness() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_config(dir.path()), Ok(None));

        write(&dir.path().join(PROJECT_CONFIG), "");
        assert_eq!(read_config(dir.path()), Ok(Some(ProjectConfig::default())));

        write(
            &dir.path().join(PROJECT_CONFIG),
            "stores: [acme-eng, platform]\nroot: docs/knowledge\n",
        );
        let cfg = read_config(dir.path()).expect("read").expect("present");
        assert_eq!(cfg.stores, ["acme-eng", "platform"]);
        assert_eq!(cfg.root.as_deref(), Some("docs/knowledge"));

        write(&dir.path().join(PROJECT_CONFIG), "stores: acme-eng\n");
        assert_eq!(
            read_config(dir.path()).expect_err("scalar stores").code,
            "project_config_invalid"
        );
    }

    #[test]
    fn the_default_root_counts_only_when_it_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(project_root(dir.path(), None), Ok(None));
        std::fs::create_dir_all(dir.path().join(DEFAULT_PROJECT_ROOT)).expect("mkdir");
        assert_eq!(
            project_root(dir.path(), None),
            Ok(Some(dir.path().join(DEFAULT_PROJECT_ROOT)))
        );
    }

    #[test]
    fn a_configured_root_is_used_and_may_be_the_repo_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("docs/knowledge")).expect("mkdir");
        let cfg = |root: &str| ProjectConfig {
            stores: Vec::new(),
            root: Some(root.into()),
        };
        assert_eq!(
            project_root(dir.path(), Some(&cfg("docs/knowledge"))),
            Ok(Some(dir.path().join("docs/knowledge")))
        );
        assert_eq!(
            project_root(dir.path(), Some(&cfg("."))),
            Ok(Some(dir.path().join(".")))
        );
    }

    #[test]
    fn a_root_outside_the_repo_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        std::fs::create_dir_all(dir.path().join("elsewhere")).expect("mkdir");
        for rel in ["../elsewhere", "/etc", "docs/../../elsewhere"] {
            let cfg = ProjectConfig {
                stores: Vec::new(),
                root: Some(rel.into()),
            };
            assert_eq!(
                project_root(&repo, Some(&cfg)).map_err(|e| e.code),
                Err("project_root_outside"),
                "{rel}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_escaping_the_repo_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".okena")).expect("mkdir");
        std::fs::create_dir_all(dir.path().join("elsewhere")).expect("mkdir");
        std::os::unix::fs::symlink(
            dir.path().join("elsewhere"),
            repo.join(DEFAULT_PROJECT_ROOT),
        )
        .expect("symlink");
        assert_eq!(
            project_root(&repo, None).map_err(|e| e.code),
            Err("project_root_outside")
        );
    }
}
