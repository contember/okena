//! Whether the next start should nudge the user towards What's New.
//!
//! Pure: the host reads `whats_new_seen_version` from its settings, decides
//! here, and records the running version — posting the toast first when an
//! update calls for it, so the toast shows once per update.

use semver::Version;

/// What the host does at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nudge {
    /// Post the "see what's new" toast and record the running version as
    /// seen, so the next start is quiet whether or not the toast was used.
    Toast,
    /// Record the running version as seen, quietly: a fresh install, the
    /// first run of this feature, or a seen marker that cannot be parsed.
    RecordOnly,
    /// Nothing to do: already seen, a revert to an older build, the nudge is
    /// off, or the running version is not a version at all.
    Nothing,
}

pub fn whats_new_nudge(seen: Option<&str>, running: &str, enabled: bool) -> Nudge {
    let Ok(running) = Version::parse(running.strip_prefix('v').unwrap_or(running)) else {
        return Nudge::Nothing;
    };
    let Some(seen) = seen else {
        return Nudge::RecordOnly;
    };
    let Ok(seen) = Version::parse(seen.strip_prefix('v').unwrap_or(seen)) else {
        return Nudge::RecordOnly;
    };
    if seen < running && enabled {
        Nudge::Toast
    } else {
        Nudge::Nothing
    }
}

#[cfg(test)]
mod tests {
    use super::{Nudge, whats_new_nudge};

    #[test]
    fn a_fresh_install_records_without_a_toast() {
        assert_eq!(whats_new_nudge(None, "0.35.0", true), Nudge::RecordOnly);
        assert_eq!(whats_new_nudge(None, "0.35.0", false), Nudge::RecordOnly);
    }

    #[test]
    fn an_update_toasts() {
        assert_eq!(
            whats_new_nudge(Some("0.34.0"), "0.35.0", true),
            Nudge::Toast
        );
    }

    #[test]
    fn the_running_version_seen_is_quiet() {
        assert_eq!(
            whats_new_nudge(Some("0.35.0"), "0.35.0", true),
            Nudge::Nothing
        );
    }

    #[test]
    fn a_revert_to_an_older_build_is_quiet() {
        assert_eq!(
            whats_new_nudge(Some("0.36.0"), "0.35.0", true),
            Nudge::Nothing
        );
    }

    #[test]
    fn the_toggle_silences_the_toast() {
        assert_eq!(
            whats_new_nudge(Some("0.34.0"), "0.35.0", false),
            Nudge::Nothing
        );
    }

    #[test]
    fn an_unparsable_seen_marker_is_rewritten() {
        assert_eq!(
            whats_new_nudge(Some("garbage"), "0.35.0", true),
            Nudge::RecordOnly
        );
    }

    #[test]
    fn an_unparsable_running_version_does_nothing() {
        assert_eq!(whats_new_nudge(Some("0.34.0"), "dev", true), Nudge::Nothing);
    }
}
