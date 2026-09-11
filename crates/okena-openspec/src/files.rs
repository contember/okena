//! OpenSpec's file formats.
//!
//! Each reader accepts what the CLI accepts and rejects what it rejects — the
//! registry and store identity are strict (OpenSpec validates them with
//! `.strict()` zod schemas), project config is tolerant — and each writer
//! produces output the CLI reads back unchanged.

use crate::paths::display;
use crate::{KEBAB_ID_DESCRIPTION, OpenSpecError};
use okena_core::specs::is_kebab_id;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_yaml_ng::Value as Yaml;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const STORE_METADATA_DIR: &str = ".openspec-store";
pub const STORE_METADATA_FILE: &str = "store.yaml";
pub const CHANGE_METADATA_FILE: &str = ".openspec.yaml";
pub const DEFAULT_SCHEMA: &str = "spec-driven";

// ─── Store registry: <data>/stores/registry.yaml ─────────────────────────────

/// The machine store registry.
///
/// ```yaml
/// version: 1
/// stores:
///   team-plans:
///     backend:
///       type: git
///       local_path: /Users/you/openspec/team-plans
///       remote: git@github.com:acme/team-plans.git   # observed git origin
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub version: u64,
    /// Keyed by store id. A `BTreeMap` so writes come out sorted by id, as the
    /// CLI writes them.
    pub stores: BTreeMap<String, RegistryEntry>,
    /// Legacy code-checkout map: tolerated on read and dropped on the next
    /// write, exactly as OpenSpec does.
    #[serde(default, skip_serializing)]
    #[allow(dead_code)] // Held only so a registry carrying it still parses.
    repos: Option<Yaml>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryEntry {
    pub backend: Backend,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backend {
    /// Always `git` today.
    #[serde(rename = "type")]
    pub kind: String,
    /// Canonical path of the checkout.
    pub local_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl Registry {
    pub fn empty() -> Self {
        Self {
            version: 1,
            stores: BTreeMap::new(),
            repos: None,
        }
    }

    pub fn parse(content: &str, path: &Path) -> Result<Self, OpenSpecError> {
        let invalid = |detail: String| {
            OpenSpecError::new(
                "invalid_store_registry",
                format!("Invalid store registry state: {detail}"),
            )
            .with_fix(format!("Repair or remove {}.", display(path)))
        };
        let registry: Registry =
            serde_yaml_ng::from_str(content).map_err(|e| invalid(e.to_string()))?;
        if registry.version != 1 {
            return Err(invalid(format!(
                "version must be 1, found {}",
                registry.version
            )));
        }
        for (id, entry) in &registry.stores {
            if !is_kebab_id(id) {
                return Err(invalid(format!("'{id}': {KEBAB_ID_DESCRIPTION}")));
            }
            let b = &entry.backend;
            if b.kind != "git" {
                return Err(invalid(format!(
                    "store '{id}' has unsupported backend '{}'",
                    b.kind
                )));
            }
            if b.local_path.is_empty() {
                return Err(invalid(format!("store '{id}' has an empty local_path")));
            }
            if b.remote.as_deref() == Some("") || b.branch.as_deref() == Some("") {
                return Err(invalid(format!(
                    "store '{id}' has an empty remote or branch"
                )));
            }
        }
        Ok(registry)
    }

    /// Read the registry. `Ok(None)` when it does not exist yet — the normal
    /// state before the first store.
    pub fn read(path: &Path) -> Result<Option<Self>, OpenSpecError> {
        if !path.is_file() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(path).map_err(|e| {
            OpenSpecError::new(
                "store_registry_unreadable",
                format!("Could not read {}: {e}", display(path)),
            )
        })?;
        Self::parse(&content, path).map(Some)
    }

    pub fn to_yaml(&self) -> String {
        serde_yaml_ng::to_string(self).unwrap_or_else(|_| "version: 1\nstores: {}\n".into())
    }
}

// ─── Store identity: <store>/.openspec-store/store.yaml ─────────────────────

/// A store's committed identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreMetadata {
    pub version: u64,
    pub id: String,
    /// Canonical clone source, team-authored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

impl StoreMetadata {
    pub fn new(id: &str, remote: Option<&str>) -> Self {
        Self {
            version: 1,
            id: id.to_string(),
            remote: remote.map(str::to_string),
        }
    }

    pub fn path(root: &Path) -> PathBuf {
        root.join(STORE_METADATA_DIR).join(STORE_METADATA_FILE)
    }

    pub fn parse(content: &str) -> Result<Self, OpenSpecError> {
        let invalid = |detail: String| {
            OpenSpecError::new(
                "invalid_store_metadata",
                format!("Invalid store metadata state: {detail}"),
            )
            .with_fix("Repair .openspec-store/store.yaml.")
        };
        let meta: StoreMetadata =
            serde_yaml_ng::from_str(content).map_err(|e| invalid(e.to_string()))?;
        if meta.version != 1 {
            return Err(invalid(format!(
                "version must be 1, found {}",
                meta.version
            )));
        }
        if meta.remote.as_deref() == Some("") {
            return Err(invalid("remote must not be empty".into()));
        }
        crate::validate_store_id(&meta.id)?;
        Ok(meta)
    }

    /// `Ok(None)` when the root has no identity file.
    pub fn read(root: &Path) -> Result<Option<Self>, OpenSpecError> {
        match std::fs::read_to_string(Self::path(root)) {
            Ok(content) => Self::parse(&content).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(OpenSpecError::new(
                "invalid_store_metadata",
                format!("Could not read {}: {e}", display(&Self::path(root))),
            )),
        }
    }

    pub fn to_yaml(&self) -> String {
        serde_yaml_ng::to_string(self).unwrap_or_default()
    }

    pub fn write(&self, root: &Path) -> Result<(), OpenSpecError> {
        let path = Self::path(root);
        let failed = |e: std::io::Error| {
            OpenSpecError::new(
                "store_metadata_write_failed",
                format!("Could not write {}: {e}", display(&path)),
            )
        };
        std::fs::create_dir_all(root.join(STORE_METADATA_DIR)).map_err(failed)?;
        std::fs::write(&path, self.to_yaml()).map_err(failed)
    }
}

// ─── Project config: <root>/openspec/config.yaml ─────────────────────────────

/// Why a `store:` line could not be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerProblem {
    Unparseable,
    NonString,
}

impl PointerProblem {
    /// OpenSpec's `storePointerProblem` wording.
    pub fn describe(self) -> &'static str {
        match self {
            PointerProblem::Unparseable => "the config file could not be read as YAML",
            PointerProblem::NonString => "the store key must be a single store id string",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorePointer {
    Absent,
    Value(String),
    Malformed(PointerProblem),
}

/// One `references:` entry, normalized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    pub id: String,
    pub remote: Option<String>,
}

/// The parts of `openspec/config.yaml` okena uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectConfig {
    /// Which file was read: `config.yaml`, else `config.yml`.
    pub path: PathBuf,
    pub schema: Option<String>,
    pub store: StorePointer,
    pub references: Vec<Declaration>,
}

/// The config file OpenSpec would read, if any. Probed by existence, not
/// kind, as the CLI does.
pub fn config_file(root: &Path) -> Option<PathBuf> {
    let yaml = root.join("openspec").join("config.yaml");
    if yaml.exists() {
        return Some(yaml);
    }
    let yml = root.join("openspec").join("config.yml");
    yml.exists().then_some(yml)
}

pub fn read_project_config(root: &Path) -> Option<ProjectConfig> {
    let path = config_file(root)?;
    Some(match std::fs::read_to_string(&path) {
        Ok(content) => parse_project_config(&content, path),
        Err(_) => ProjectConfig {
            path,
            schema: None,
            store: StorePointer::Malformed(PointerProblem::Unparseable),
            references: Vec::new(),
        },
    })
}

pub fn parse_project_config(content: &str, path: PathBuf) -> ProjectConfig {
    let mut config = ProjectConfig {
        path,
        schema: None,
        store: StorePointer::Absent,
        references: Vec::new(),
    };
    let value: Yaml = match serde_yaml_ng::from_str(content) {
        Ok(v) => v,
        // An empty or comments-only file carries nothing, and is not broken.
        Err(_) if is_blank_yaml(content) => return config,
        Err(_) => {
            config.store = StorePointer::Malformed(PointerProblem::Unparseable);
            return config;
        }
    };
    if value.as_mapping().is_none() {
        return config;
    }
    config.schema = value
        .get("schema")
        .and_then(Yaml::as_str)
        .map(str::to_string);
    config.store = match value.get("store") {
        None => StorePointer::Absent,
        Some(Yaml::String(s)) => StorePointer::Value(s.clone()),
        Some(_) => StorePointer::Malformed(PointerProblem::NonString),
    };
    config.references = parse_references(value.get("references"));
    config
}

fn is_blank_yaml(content: &str) -> bool {
    content
        .lines()
        .map(str::trim)
        .all(|l| l.is_empty() || l.starts_with('#') || l == "---")
}

/// OpenSpec's `parseDeclarationList`: strings or `{id, remote}` maps, deduped
/// by id keeping the first position; a later duplicate may fill in a missing
/// remote but never overrides one.
fn parse_references(raw: Option<&Yaml>) -> Vec<Declaration> {
    let Some(items) = raw.and_then(Yaml::as_sequence) else {
        return Vec::new();
    };
    let mut out: Vec<Declaration> = Vec::new();
    for item in items {
        let declaration = match item {
            Yaml::String(id) => Some(Declaration {
                id: id.clone(),
                remote: None,
            }),
            Yaml::Mapping(_) => item.get("id").and_then(Yaml::as_str).map(|id| Declaration {
                id: id.to_string(),
                remote: item
                    .get("remote")
                    .and_then(Yaml::as_str)
                    .filter(|r| !r.is_empty())
                    .map(str::to_string),
            }),
            _ => None,
        };
        let Some(declaration) = declaration else {
            continue;
        };
        match out.iter_mut().find(|d| d.id == declaration.id) {
            Some(existing) => {
                if existing.remote.is_none() {
                    existing.remote = declaration.remote;
                }
            }
            None => out.push(declaration),
        }
    }
    out
}

/// The config a new root starts with.
pub fn default_project_config() -> String {
    format!("schema: {DEFAULT_SCHEMA}\n")
}

// ─── Change metadata: <change>/.openspec.yaml ────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeMetadata {
    pub schema: Option<String>,
    pub created: Option<String>,
}

/// Read a change's metadata, tolerating anything malformed — a broken
/// `.openspec.yaml` is for `openspec validate` to report, not a reason to hide
/// the change.
pub fn read_change_metadata(change_dir: &Path) -> ChangeMetadata {
    let Ok(content) = std::fs::read_to_string(change_dir.join(CHANGE_METADATA_FILE)) else {
        return ChangeMetadata::default();
    };
    let Ok(value) = serde_yaml_ng::from_str::<Yaml>(&content) else {
        return ChangeMetadata::default();
    };
    let text = |key: &str| value.get(key).and_then(Yaml::as_str).map(str::to_string);
    ChangeMetadata {
        schema: text("schema"),
        created: text("created"),
    }
}

/// The `.openspec.yaml` that `openspec new change` writes.
pub fn change_metadata_yaml(schema: &str, created: &str) -> String {
    #[derive(Serialize)]
    struct Out<'a> {
        schema: &'a str,
        created: &'a str,
    }
    serde_yaml_ng::to_string(&Out { schema, created })
        .unwrap_or_else(|_| format!("schema: {schema}\ncreated: {created}\n"))
}

// ─── Global config: <config>/config.json ─────────────────────────────────────

/// OpenSpec's machine-wide `defaultStore`, if set.
pub fn read_default_store(path: &Path) -> Result<Option<String>, OpenSpecError> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable_config(path, &e.to_string())),
    };
    let value: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| invalid_config(path, &e.to_string()))?;
    Ok(value
        .get("defaultStore")
        .and_then(|d| d.as_str())
        .filter(|d| !d.is_empty())
        .map(str::to_string))
}

/// Set or clear `defaultStore`, the way `openspec config set|unset` does.
///
/// Every other key is kept, in its original order. A file that is not valid
/// JSON is refused rather than replaced: it holds the user's other CLI
/// settings, and the CLI itself would fall back to defaults and then
/// overwrite them.
pub fn write_default_store(path: &Path, id: Option<&str>) -> Result<(), OpenSpecError> {
    let mut entries = match std::fs::read_to_string(path) {
        Ok(content) => {
            serde_json::from_str::<OrderedObject>(&content)
                .map_err(|e| invalid_config(path, &e.to_string()))?
                .0
        }
        // The CLI saves its defaults alongside the first key it sets.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![
            ("featureFlags".to_string(), serde_json::json!({})),
            ("profile".to_string(), serde_json::json!("core")),
            ("delivery".to_string(), serde_json::json!("both")),
        ],
        Err(e) => return Err(unreadable_config(path, &e.to_string())),
    };
    let position = entries.iter().position(|(k, _)| k == "defaultStore");
    entries.retain(|(k, _)| k != "defaultStore");
    if let Some(id) = id {
        let entry = ("defaultStore".to_string(), serde_json::json!(id));
        match position {
            Some(i) => entries.insert(i.min(entries.len()), entry),
            None => entries.push(entry),
        }
    }
    let mut json = serde_json::to_string_pretty(&OrderedObject(entries))
        .map_err(|e| invalid_config(path, &e.to_string()))?;
    json.push('\n');
    write_atomically(path, &json, false).map_err(|e| {
        OpenSpecError::new(
            "global_config_write_failed",
            format!("Could not write {}: {e}", display(path)),
        )
    })
}

fn invalid_config(path: &Path, detail: &str) -> OpenSpecError {
    OpenSpecError::new(
        "invalid_global_config",
        format!("Invalid JSON in {}: {detail}", display(path)),
    )
    .with_fix(format!("Repair {}.", display(path)))
}

fn unreadable_config(path: &Path, detail: &str) -> OpenSpecError {
    OpenSpecError::new(
        "global_config_unreadable",
        format!("Could not read {}: {detail}", display(path)),
    )
}

/// A JSON object that keeps its key order through a read/write round trip,
/// independent of whether `serde_json`'s `preserve_order` is enabled.
struct OrderedObject(Vec<(String, serde_json::Value)>);

impl<'de> Deserialize<'de> for OrderedObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = OrderedObject;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<OrderedObject, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, serde_json::Value>()? {
                    entries.push(entry);
                }
                Ok(OrderedObject(entries))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

impl Serialize for OrderedObject {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

// ─── Writing ─────────────────────────────────────────────────────────────────

/// Temp file and rename — the CLI's `writeFileAtomically`. `private` makes the
/// file owner-only, as the CLI does for the registry.
pub(crate) use okena_core::fs::write_atomically;

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-for-byte what `openspec store setup team-plans` (1.13.0) wrote.
    const CLI_REGISTRY: &str = "version: 1\nstores:\n  team-plans:\n    backend:\n      type: git\n      local_path: /Users/you/openspec/team-plans\n";

    #[test]
    fn a_registry_written_by_the_cli_parses_and_round_trips_unchanged() {
        let r = Registry::parse(CLI_REGISTRY, Path::new("/r")).unwrap();
        let entry = &r.stores["team-plans"].backend;
        assert_eq!(entry.kind, "git");
        assert_eq!(entry.local_path, "/Users/you/openspec/team-plans");
        assert_eq!(r.to_yaml(), CLI_REGISTRY);
    }

    #[test]
    fn registry_writes_are_sorted_by_id_and_carry_optional_fields() {
        let mut r = Registry::empty();
        for (id, remote) in [("zeta", None), ("alpha", Some("git@x:a.git"))] {
            r.stores.insert(
                id.into(),
                RegistryEntry {
                    backend: Backend {
                        kind: "git".into(),
                        local_path: format!("/s/{id}"),
                        remote: remote.map(str::to_string),
                        branch: None,
                    },
                },
            );
        }
        let yaml = r.to_yaml();
        assert!(yaml.find("alpha").unwrap() < yaml.find("zeta").unwrap());
        assert!(yaml.contains("remote: git@x:a.git"));
        assert!(!yaml.contains("branch"));
        assert_eq!(Registry::empty().to_yaml(), "version: 1\nstores: {}\n");
    }

    #[test]
    fn the_legacy_repos_map_is_tolerated_and_dropped_on_write() {
        let yaml = format!("{CLI_REGISTRY}repos:\n  web: /p/web\n");
        let r = Registry::parse(&yaml, Path::new("/r")).unwrap();
        assert!(!r.to_yaml().contains("repos"));
    }

    #[test]
    fn a_registry_the_cli_would_reject_is_rejected() {
        for bad in [
            "version: 2\nstores: {}\n",
            "version: 1\n",
            "version: 1\nstores: {}\nextra: 1\n",
            "version: 1\nstores:\n  Bad_Id:\n    backend: {type: git, local_path: /x}\n",
            "version: 1\nstores:\n  a:\n    backend: {type: svn, local_path: /x}\n",
            "version: 1\nstores:\n  a:\n    backend: {type: git, local_path: /x, remote: ''}\n",
            "version: 1\nstores:\n  a:\n    backend: {type: git, local_path: /x, extra: 1}\n",
            ": not yaml",
        ] {
            let err = Registry::parse(bad, Path::new("/r/registry.yaml")).unwrap_err();
            assert_eq!(err.code, "invalid_store_registry", "{bad:?}");
            assert!(err.fix.unwrap().contains("/r/registry.yaml"));
        }
    }

    #[test]
    fn store_metadata_matches_the_cli_format() {
        // What `openspec store setup team-plans --remote …` wrote.
        let cli = "version: 1\nid: team-plans\nremote: git@github.com:acme/team-plans.git\n";
        let m = StoreMetadata::parse(cli).unwrap();
        assert_eq!(
            m,
            StoreMetadata::new("team-plans", Some("git@github.com:acme/team-plans.git"))
        );
        assert_eq!(m.to_yaml(), cli);
        assert_eq!(
            StoreMetadata::new("a", None).to_yaml(),
            "version: 1\nid: a\n"
        );
    }

    #[test]
    fn store_metadata_is_strict() {
        assert_eq!(
            StoreMetadata::parse("version: 1\nid: Nope\n")
                .unwrap_err()
                .code,
            "invalid_store_id"
        );
        for bad in [
            "version: 1\n",
            "version: 1\nid: a\nextra: 1\n",
            "version: 3\nid: a\n",
        ] {
            assert_eq!(
                StoreMetadata::parse(bad).unwrap_err().code,
                "invalid_store_metadata"
            );
        }
    }

    fn config(content: &str) -> ProjectConfig {
        parse_project_config(content, PathBuf::from("/p/openspec/config.yaml"))
    }

    #[test]
    fn project_config_reads_schema_pointer_and_references() {
        let c = config(
            "schema: spec-driven\nstore: team-plans\nreferences:\n  - design-system\n  - { id: other, remote: \"git@github.com:acme/other.git\" }\n  - { id: design-system, remote: late }\n  - 42\n",
        );
        assert_eq!(c.schema.as_deref(), Some("spec-driven"));
        assert_eq!(c.store, StorePointer::Value("team-plans".into()));
        assert_eq!(
            c.references,
            [
                // First position kept; the later duplicate fills the missing remote.
                Declaration {
                    id: "design-system".into(),
                    remote: Some("late".into())
                },
                Declaration {
                    id: "other".into(),
                    remote: Some("git@github.com:acme/other.git".into())
                },
            ]
        );
    }

    #[test]
    fn pointer_problems_follow_the_cli() {
        assert_eq!(
            config("store: [a, b]\n").store,
            StorePointer::Malformed(PointerProblem::NonString)
        );
        assert_eq!(
            config("store:\n").store,
            StorePointer::Malformed(PointerProblem::NonString)
        );
        assert_eq!(
            config("store: [unclosed\n").store,
            StorePointer::Malformed(PointerProblem::Unparseable)
        );
        // Imperfect, not malformed.
        assert_eq!(config("").store, StorePointer::Absent);
        assert_eq!(config("# just comments\n").store, StorePointer::Absent);
        assert_eq!(config("- a list\n").store, StorePointer::Absent);
    }

    #[test]
    fn the_cli_config_template_with_its_comment_block_parses() {
        let c = config(
            "schema: spec-driven\n\n# Project context (optional)\n# Example:\n#   context: |\n#     Tech stack\n",
        );
        assert_eq!(c.schema.as_deref(), Some("spec-driven"));
        assert_eq!(c.store, StorePointer::Absent);
    }

    #[test]
    fn change_metadata_matches_what_openspec_new_change_writes() {
        assert_eq!(
            change_metadata_yaml("spec-driven", "2026-09-10"),
            "schema: spec-driven\ncreated: 2026-09-10\n"
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CHANGE_METADATA_FILE),
            "schema: spec-driven\ncreated: 2026-09-10\n",
        )
        .unwrap();
        let m = read_change_metadata(dir.path());
        assert_eq!(m.schema.as_deref(), Some("spec-driven"));
        assert_eq!(m.created.as_deref(), Some("2026-09-10"));
    }

    #[test]
    fn default_store_is_set_and_cleared_keeping_other_keys_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("openspec/config.json");

        // Missing file: the CLI's defaults come along, as `config set` writes.
        write_default_store(&path, Some("team-plans")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\n  \"featureFlags\": {},\n  \"profile\": \"core\",\n  \"delivery\": \"both\",\n  \"defaultStore\": \"team-plans\"\n}\n"
        );
        assert_eq!(
            read_default_store(&path).unwrap().as_deref(),
            Some("team-plans")
        );

        std::fs::write(
            &path,
            "{\"zeta\":1,\"defaultStore\":\"a\",\"alpha\":{\"y\":1}}",
        )
        .unwrap();
        write_default_store(&path, Some("b")).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        let (z, d, a) = (
            written.find("zeta").unwrap(),
            written.find("defaultStore").unwrap(),
            written.find("alpha").unwrap(),
        );
        assert!(z < d && d < a, "key order changed: {written}");

        write_default_store(&path, None).unwrap();
        assert_eq!(read_default_store(&path).unwrap(), None);
        assert!(std::fs::read_to_string(&path).unwrap().contains("zeta"));
    }

    #[test]
    fn an_invalid_global_config_is_refused_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(
            write_default_store(&path, Some("a")).unwrap_err().code,
            "invalid_global_config"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        assert_eq!(
            read_default_store(&path).unwrap_err().code,
            "invalid_global_config"
        );
    }

    #[test]
    fn atomic_writes_leave_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stores/registry.yaml");
        write_atomically(&path, "a", true).unwrap();
        write_atomically(&path, "b", true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "b");
        let names: Vec<_> = std::fs::read_dir(dir.path().join("stores"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }
}
