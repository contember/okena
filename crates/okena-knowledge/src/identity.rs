//! A store's committed identity, `.okena-knowledge/store.yaml`.
//!
//! The identity travels with the repository, so every clone of a store agrees
//! on its id no matter what the checkout folder is called — which is what lets
//! a project's `.okena/knowledge.yaml` name a store on any machine.

use crate::{KnowledgeError, display};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const STORE_METADATA_DIR: &str = ".okena-knowledge";
pub const STORE_FILE: &str = "store.yaml";
/// The identity format this okena reads and writes.
pub const IDENTITY_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreIdentity {
    pub version: u32,
    pub id: String,
    /// Display name; the id when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Canonical clone source, for onboarding hints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

impl StoreIdentity {
    pub fn new(id: &str) -> Self {
        Self {
            version: IDENTITY_VERSION,
            id: id.to_string(),
            name: None,
            description: None,
            remote: None,
        }
    }

    pub fn path(root: &Path) -> PathBuf {
        root.join(STORE_METADATA_DIR).join(STORE_FILE)
    }

    /// Read `root`'s identity. `Ok(None)` when the root has none — a plain repo
    /// of kind folders, which is allowed.
    ///
    /// Unknown keys are ignored so a store written by a newer okena that only
    /// added fields still reads; a newer `version` is refused, because that is
    /// how a breaking change announces itself.
    pub fn read(root: &Path) -> Result<Option<Self>, KnowledgeError> {
        let path = Self::path(root);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(KnowledgeError::new(
                    "store_identity_unreadable",
                    format!("Could not read {}: {e}", display(&path)),
                ));
            }
        };
        let identity: Self = serde_yaml_ng::from_str(&text).map_err(|e| {
            KnowledgeError::new(
                "store_identity_invalid",
                format!("{} is not valid store identity YAML: {e}", display(&path)),
            )
            .with_fix("It needs at least `version: 1` and `id: <kebab-case-id>`.")
        })?;
        if identity.version > IDENTITY_VERSION {
            return Err(KnowledgeError::new(
                "store_identity_version",
                format!(
                    "{} is identity version {}; this okena reads up to {IDENTITY_VERSION}.",
                    display(&path),
                    identity.version
                ),
            )
            .with_fix("Update okena."));
        }
        validate_store_id(&identity.id)?;
        Ok(Some(identity))
    }

    pub fn to_yaml(&self) -> String {
        // Serializing a plain struct of strings cannot fail.
        serde_yaml_ng::to_string(self).unwrap_or_default()
    }
}

/// Store ids are kebab-case, the same grammar OpenSpec uses for its stores, so
/// an id is safe as a folder name, a key segment and a CLI argument.
pub fn validate_store_id(id: &str) -> Result<&str, KnowledgeError> {
    if okena_core::specs::is_kebab_id(id) {
        Ok(id)
    } else {
        Err(KnowledgeError::new(
            "invalid_store_id",
            format!(
                "Store id `{id}` must be kebab-case: lowercase letters and digits separated by single hyphens."
            ),
        )
        .with_fix("Use an id such as acme-eng."))
    }
}

/// A usable id derived from a folder name, for a checkout without an identity.
pub fn id_from_folder(root: &Path) -> Option<String> {
    let name = root.file_name()?.to_string_lossy();
    let id = okena_core::specs::change_slug(&name);
    okena_core::specs::is_kebab_id(&id).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write;

    fn read_yaml(yaml: &str) -> Result<Option<StoreIdentity>, KnowledgeError> {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&StoreIdentity::path(dir.path()), yaml);
        StoreIdentity::read(dir.path())
    }

    #[test]
    fn reads_a_valid_identity_and_ignores_unknown_keys() {
        let id = read_yaml(
            "version: 1\nid: acme-eng\nname: Acme Engineering\nremote: git@github.com:acme/eng.git\nfuture: yes\n",
        )
        .expect("read")
        .expect("present");
        assert_eq!(id.id, "acme-eng");
        assert_eq!(id.name.as_deref(), Some("Acme Engineering"));
        assert_eq!(id.remote.as_deref(), Some("git@github.com:acme/eng.git"));
    }

    #[test]
    fn a_missing_identity_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(StoreIdentity::read(dir.path()), Ok(None));
    }

    #[test]
    fn bad_ids_yaml_and_newer_versions_are_refused_with_their_codes() {
        let code = |yaml| read_yaml(yaml).expect_err("refused").code;
        assert_eq!(code("version: 1\nid: Acme_Eng\n"), "invalid_store_id");
        assert_eq!(code("version: 1\n"), "store_identity_invalid");
        assert_eq!(code("id: [\n"), "store_identity_invalid");
        assert_eq!(code("version: 2\nid: acme\n"), "store_identity_version");
    }

    #[test]
    fn written_identity_reads_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut id = StoreIdentity::new("team-docs");
        id.description = Some("How we work".into());
        write(&StoreIdentity::path(dir.path()), &id.to_yaml());
        assert_eq!(StoreIdentity::read(dir.path()), Ok(Some(id)));
    }

    #[test]
    fn folder_names_become_kebab_ids_when_they_can() {
        assert_eq!(
            id_from_folder(Path::new("/k/Eng_Knowledge")).as_deref(),
            Some("eng-knowledge")
        );
        assert_eq!(id_from_folder(Path::new("/k/!!!")), None);
    }
}
