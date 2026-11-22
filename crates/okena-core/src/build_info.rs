//! Identity of the running binary, independent of updater state.

use serde::Deserialize;
use std::sync::LazyLock;

pub static BUILD_INFO: LazyLock<BuildInfo> = LazyLock::new(|| {
    BuildInfo::new(
        env!("CARGO_PKG_VERSION"),
        option_env!("OKENA_NIGHTLY_MANIFEST").unwrap_or_default(),
    )
});

pub struct BuildInfo {
    pub version: String,
    pub nightly: Option<NightlyBuild>,
}

/// Contents of the nightly release's `nightly.json`, embedded at build time.
#[derive(Deserialize)]
pub struct NightlyBuild {
    pub tag: String,
    pub date: String,
    pub sha: String,
    pub run_number: u64,
    pub base_version: String,
}

impl BuildInfo {
    fn new(base_version: &str, manifest: &str) -> Self {
        if manifest.is_empty() {
            return Self {
                version: base_version.to_owned(),
                nightly: None,
            };
        }

        let nightly = serde_json::from_str::<NightlyBuild>(manifest);
        let nightly = match nightly {
            Ok(build)
                if build.base_version == base_version
                    && build.sha.len() == 40
                    && build.sha.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                build
            }
            // A locally misconfigured build can still start and report the error.
            // The packaging check requires an exact match and rejects this label.
            _ => {
                return Self {
                    version: format!("{base_version} (invalid nightly manifest)"),
                    nightly: None,
                };
            }
        };
        let version = format!("{base_version} Nightly · {}", &nightly.sha[..7]);
        Self {
            version,
            nightly: Some(nightly),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BuildInfo;

    #[test]
    fn versions_identify_the_running_build() {
        let stable = BuildInfo::new("1.2.3", "");
        assert_eq!(stable.version, "1.2.3");
        assert!(stable.nightly.is_none());

        let manifest = r#"{"tag":"nightly-20261122-07a47f6","date":"2026-11-22","sha":"07a47f6fa80ae5a40f24022517911d3742a5bc9a","run_number":19,"base_version":"1.2.3"}"#;
        let nightly = BuildInfo::new("1.2.3", manifest);
        assert_eq!(nightly.version, "1.2.3 Nightly · 07a47f6");
        assert_eq!(nightly.nightly.unwrap().run_number, 19);

        let mismatched = BuildInfo::new("1.2.4", manifest);
        assert_eq!(mismatched.version, "1.2.4 (invalid nightly manifest)");
    }

    #[test]
    fn invalid_metadata_is_visible_without_panicking() {
        for manifest in [
            "invalid JSON",
            r#"{"tag":"nightly-20261122-bad","date":"2026-11-22","sha":"bad","run_number":19,"base_version":"1.2.3"}"#,
        ] {
            let build = BuildInfo::new("1.2.3", manifest);
            assert_eq!(build.version, "1.2.3 (invalid nightly manifest)");
            assert!(build.nightly.is_none());
        }
    }
}
