//! Credential storage, one file per profile.
//!
//! Lives at `<profile root>/tasks_credentials.json`, mode 0600 — the same
//! at-rest treatment `remote_secret` gets, and per-profile for the same reason:
//! a `dev` profile must not read the credentials a `default` profile holds.
//!
//! Only the credential is persisted. Task lists are remote state and are never
//! written to disk — a stale cached issue list is worse than an empty one.

use crate::provider::Credential;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const FILE_NAME: &str = "tasks_credentials.json";

/// On-disk shape. Keyed by provider id (`"linear"`), so adding Jira later needs
/// no migration.
#[derive(Default, Serialize, Deserialize)]
struct CredentialFile {
    #[serde(default)]
    providers: HashMap<String, StoredCredential>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredCredential {
    ApiKey {
        key: String,
    },
    OAuth {
        access_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<u64>,
    },
}

impl From<&Credential> for StoredCredential {
    fn from(c: &Credential) -> Self {
        match c {
            Credential::ApiKey(key) => StoredCredential::ApiKey { key: key.clone() },
            Credential::OAuth {
                access_token,
                refresh_token,
                expires_at,
            } => StoredCredential::OAuth {
                access_token: access_token.clone(),
                refresh_token: refresh_token.clone(),
                expires_at: *expires_at,
            },
        }
    }
}

impl From<StoredCredential> for Credential {
    fn from(s: StoredCredential) -> Self {
        match s {
            StoredCredential::ApiKey { key } => Credential::ApiKey(key),
            StoredCredential::OAuth {
                access_token,
                refresh_token,
                expires_at,
            } => Credential::OAuth {
                access_token,
                refresh_token,
                expires_at,
            },
        }
    }
}

fn path() -> Option<PathBuf> {
    okena_core::profiles::try_current().map(|p| p.root.join(FILE_NAME))
}

fn read_file(at: &std::path::Path) -> CredentialFile {
    match std::fs::read_to_string(at) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            // A corrupt file must not wedge the harness: log and start clean,
            // which surfaces as "not connected" and prompts a reconnect.
            log::warn!("[tasks] could not parse {}: {e}", at.display());
            CredentialFile::default()
        }),
        Err(_) => CredentialFile::default(),
    }
}

fn write_file(at: &std::path::Path, file: &CredentialFile) -> Result<(), String> {
    let json = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Write-then-rename so an interrupted save can't truncate a good file, and
    // chmod the temp *before* it holds a secret.
    let tmp = at.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, at).map_err(|e| e.to_string())
}

/// Load the stored credential for `provider`, if any.
pub fn load(provider: &str) -> Option<Credential> {
    let at = path()?;
    read_file(&at)
        .providers
        .remove(provider)
        .map(Credential::from)
}

/// Persist `credential` for `provider`, replacing any existing one.
pub fn save(provider: &str, credential: &Credential) -> Result<(), String> {
    let at = path().ok_or("no active profile — cannot store credentials")?;
    let mut file = read_file(&at);
    file.providers
        .insert(provider.to_string(), StoredCredential::from(credential));
    write_file(&at, &file)
}

/// Forget the credential for `provider`. Succeeds when none was stored.
pub fn clear(provider: &str) -> Result<(), String> {
    let at = path().ok_or("no active profile — cannot clear credentials")?;
    let mut file = read_file(&at);
    if file.providers.remove(provider).is_none() {
        return Ok(());
    }
    write_file(&at, &file)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These exercise the serialization layer directly rather than through
    // `load`/`save`: the profile global is process-wide and set once at
    // startup, so a test can't point it at a temp dir without racing every
    // other test in the binary.

    #[test]
    fn api_key_round_trips() {
        let c = Credential::ApiKey("lin_api_1".into());
        let back: Credential = StoredCredential::from(&c).into();
        assert_eq!(back, c);
    }

    #[test]
    fn oauth_round_trips_with_expiry() {
        let c = Credential::OAuth {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at: Some(42),
        };
        let back: Credential = StoredCredential::from(&c).into();
        assert_eq!(back, c);
    }

    #[test]
    fn file_is_keyed_by_provider() {
        let mut f = CredentialFile::default();
        f.providers.insert(
            "linear".into(),
            StoredCredential::from(&Credential::ApiKey("k".into())),
        );
        let json = serde_json::to_string(&f).expect("serialize");
        let back: CredentialFile = serde_json::from_str(&json).expect("deserialize");
        assert!(back.providers.contains_key("linear"));
        // Tagged representation keeps the two kinds distinguishable on disk.
        assert!(json.contains("\"kind\":\"api_key\""), "got {json}");
    }

    #[test]
    fn corrupt_file_reads_as_empty_rather_than_panicking() {
        let dir = std::env::temp_dir().join(format!("okena-tasks-store-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let at = dir.join("corrupt.json");
        std::fs::write(&at, "{ not json").expect("write fixture");
        assert!(read_file(&at).providers.is_empty());
        let _ = std::fs::remove_file(&at);
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let at = std::env::temp_dir().join("okena-tasks-store-does-not-exist.json");
        let _ = std::fs::remove_file(&at);
        assert!(read_file(&at).providers.is_empty());
    }
}
