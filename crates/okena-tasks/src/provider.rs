//! The provider abstraction.
//!
//! Adding Jira or Azure DevOps later means implementing [`TaskProvider`] and
//! registering it — no changes in the daemon or the harness UI.

use okena_core::tasks::{Task, TaskId, TaskState};
use std::fmt;

/// How a provider's credentials are supplied.
///
/// Both variants end up as a bearer token on the wire; they differ in where the
/// token came from and whether it can expire. Keeping them distinct lets the UI
/// say "your login expired, re-authorize" rather than "401".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Credential {
    /// A long-lived key the user pasted in. Never expires on its own.
    ApiKey(String),
    /// An OAuth access token. May expire; `refresh_token` re-mints it.
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        /// Expiry as a Unix timestamp in seconds, when the provider states one.
        expires_at: Option<u64>,
    },
}

impl Credential {
    /// The bearer value to send.
    pub fn bearer(&self) -> &str {
        match self {
            Credential::ApiKey(k) => k,
            Credential::OAuth { access_token, .. } => access_token,
        }
    }

    /// Whether an OAuth token is past its stated expiry, with `skew_secs` of
    /// slack so a token that dies mid-flight is refreshed first.
    pub fn is_expired(&self, now_unix: u64, skew_secs: u64) -> bool {
        match self {
            Credential::ApiKey(_) => false,
            Credential::OAuth { expires_at, .. } => {
                expires_at.is_some_and(|e| now_unix.saturating_add(skew_secs) >= e)
            }
        }
    }
}

// A credential is a secret: keep it out of logs even when a surrounding struct
// is derived-Debug'd.
impl fmt::Debug for CredentialRedacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Credential::ApiKey(_) => f.write_str("ApiKey(<redacted>)"),
            Credential::OAuth { expires_at, .. } => {
                write!(f, "OAuth(<redacted>, expires_at={expires_at:?})")
            }
        }
    }
}

/// Wrapper that renders a [`Credential`] with its secret elided.
pub struct CredentialRedacted<'a>(pub &'a Credential);

/// Whether a provider is ready to make calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthStatus {
    /// No credential stored — the user has not connected this provider.
    Disconnected,
    /// Credential present and usable.
    Connected {
        /// Display name of the authenticated user, when known.
        account: Option<String>,
    },
    /// Credential present but rejected or expired; the user must re-authorize.
    Expired,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("not authenticated with {provider}")]
    NotAuthenticated { provider: &'static str },
    /// The provider rejected our credential (401/403). Distinct from a
    /// transport failure so the UI can prompt for re-auth instead of retrying.
    #[error("{provider} rejected the stored credential")]
    Unauthorized { provider: &'static str },
    #[error("{provider} request failed: {message}")]
    Transport {
        provider: &'static str,
        message: String,
    },
    /// The response parsed as JSON but didn't match the expected shape, or the
    /// provider returned an API-level error inside a 200.
    #[error("{provider} returned an unexpected response: {message}")]
    Protocol {
        provider: &'static str,
        message: String,
    },
}

/// A task source. Implementations are expected to be cheap to construct and
/// are called from daemon worker threads, so methods are blocking.
pub trait TaskProvider: Send + Sync {
    /// Stable identifier, e.g. `"linear"`. Must match [`TaskId::provider`].
    fn id(&self) -> &'static str;

    /// Human-facing name for the UI, e.g. `"Linear"`.
    fn display_name(&self) -> &'static str;

    fn auth_status(&self) -> AuthStatus;

    /// Tasks assigned to the authenticated user, newest activity first.
    ///
    /// Closed tasks are excluded — the harness is a work queue, not an archive.
    fn list_assigned(&self) -> Result<Vec<Task>, TaskError>;

    /// Move a task to a new state. Providers map the normalized category back
    /// onto one of their own workflow states; when a team has several states in
    /// the same category the provider picks its canonical one.
    fn set_state(&self, id: &TaskId, state: TaskState) -> Result<(), TaskError>;

    /// Branch name to use when starting a worktree for `task`.
    ///
    /// Default derives a slug from the task key and title; providers that
    /// supply a branch name natively (Linear does) should return theirs so the
    /// provider's own automation — branch-to-issue linking — keeps working.
    fn branch_name(&self, task: &Task) -> String {
        if !task.branch_name.is_empty() {
            return task.branch_name.clone();
        }
        slugify_branch(&task.display_key, &task.title)
    }
}

/// Build a git-safe branch name from a task key and title.
///
/// Git refuses refs containing a space, `~^:?*[\`, a `..`, a trailing `.` or
/// `.lock`, and leading/trailing `/` — so everything outside a conservative
/// allowlist collapses to `-`.
pub fn slugify_branch(key: &str, title: &str) -> String {
    let mut out = String::with_capacity(key.len() + title.len() + 1);
    let mut last_dash = false;
    for ch in format!("{key}-{title}").chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    // A trailing separator would leave `feature-` or a bare `.`-adjacent ref.
    while out.ends_with('-') {
        out.pop();
    }
    // Keep it comfortably under filesystem path limits: worktree directories
    // are derived from the branch name.
    out.truncate(60);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_git_safe() {
        assert_eq!(
            slugify_branch("LIN-12", "Fix: the thing/that broke?"),
            "lin-12-fix-the-thing-that-broke"
        );
    }

    #[test]
    fn slug_collapses_runs_and_trims() {
        assert_eq!(slugify_branch("AB-1", "  a   b  "), "ab-1-a-b");
    }

    #[test]
    fn slug_truncates_without_trailing_separator() {
        let s = slugify_branch("LIN-1", &"word ".repeat(40));
        assert!(s.len() <= 60, "got {} chars", s.len());
        assert!(!s.ends_with('-'));
    }

    #[test]
    fn api_key_never_expires() {
        let c = Credential::ApiKey("k".into());
        assert!(!c.is_expired(u64::MAX, 0));
    }

    #[test]
    fn oauth_expiry_respects_skew() {
        let c = Credential::OAuth {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: Some(1_000),
        };
        assert!(!c.is_expired(900, 30));
        assert!(c.is_expired(980, 30));
    }

    #[test]
    fn credential_debug_redacts_secret() {
        let c = Credential::ApiKey("super-secret".into());
        let rendered = format!("{:?}", CredentialRedacted(&c));
        assert!(!rendered.contains("super-secret"), "leaked: {rendered}");
    }
}
