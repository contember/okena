//! A checkout against its upstream: what the checked-out branch tracks and how
//! far apart they are, fast-forwarding to it, and where `origin` points.

use std::path::Path;

use okena_core::process::{command, safe_output};

use super::branch::{UpstreamState, parse_upstream_track};
use super::{head_branch_short, path_str, require_success};
use crate::error::GitResult;

/// The checked-out branch and how it sits against its upstream.
///
/// `None` when HEAD is detached or `path` is not in a repository. One
/// `for-each-ref` on the branch alone, with `LC_ALL=C` because the track text
/// is parsed back out. Ahead/behind are only as fresh as the last fetch.
pub fn current_upstream(path: &Path) -> Option<(String, UpstreamState)> {
    let repo = crate::gix_helpers::open(path)?;
    let branch = head_branch_short(&repo)?;
    let p = path_str(path).ok()?;
    let output = safe_output(command("git").env("LC_ALL", "C").args([
        "-C",
        p,
        "for-each-ref",
        "--format",
        "%(upstream:short)\t%(upstream:track)",
        &format!("refs/heads/{branch}"),
    ]))
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // An unborn branch has no ref yet, so no line: untracked.
    let line = stdout.lines().next().unwrap_or("");
    let (upstream, track) = line.split_once('\t').unwrap_or((line, ""));
    Some((branch, parse_upstream_track(upstream, track)))
}

/// Fast-forward the checked-out branch to its upstream.
///
/// `merge --ff-only` never rewrites local work: a diverged branch, or
/// uncommitted changes to files the upstream touches, make git refuse, and its
/// refusal is the error.
pub fn fast_forward_to_upstream(path: &Path) -> GitResult<()> {
    let p = path_str(path)?;
    require_success(safe_output(command("git").args([
        "-C",
        p,
        "merge",
        "--ff-only",
        "@{upstream}",
    ]))?)
}

/// Where `origin` fetches from, when there is an `origin`.
pub fn origin_url(path: &Path) -> Option<String> {
    let p = path_str(path).ok()?;
    let output = safe_output(command("git").args(["-C", p, "remote", "get-url", "origin"])).ok()?;
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !url.is_empty()).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::{git_in, init_temp_repo};
    use std::path::PathBuf;

    /// A seed repo, a bare remote cloned from it, and a clone of the remote.
    struct Tracked {
        _seed_dir: tempfile::TempDir,
        dir: tempfile::TempDir,
        seed: PathBuf,
    }

    impl Tracked {
        fn new() -> Self {
            let (seed_dir, seed) = init_temp_repo();
            let dir = tempfile::tempdir().expect("tempdir");
            let seed_str = seed.to_str().expect("utf8");
            git_in(dir.path(), &["clone", "--bare", seed_str, "remote.git"]);
            git_in(dir.path(), &["clone", "remote.git", "clone"]);
            Self {
                _seed_dir: seed_dir,
                dir,
                seed,
            }
        }
        fn remote(&self) -> PathBuf {
            self.dir.path().join("remote.git")
        }
        fn clone(&self) -> PathBuf {
            self.dir.path().join("clone")
        }
        /// A new commit on the remote's `main`, made from the seed.
        fn advance_remote(&self) {
            git_in(
                &self.seed,
                &[
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "upstream",
                ],
            );
            let remote = self.remote();
            git_in(
                &self.seed,
                &["push", remote.to_str().expect("utf8"), "main"],
            );
        }
    }

    fn tracked(name: &str, ahead: usize, behind: usize) -> UpstreamState {
        UpstreamState::Tracked {
            name: name.into(),
            ahead,
            behind,
        }
    }

    #[test]
    fn a_local_only_branch_is_untracked_and_has_no_origin() {
        let (_tmp, repo) = init_temp_repo();
        assert_eq!(
            current_upstream(&repo),
            Some(("main".into(), UpstreamState::Untracked))
        );
        assert_eq!(origin_url(&repo), None);
    }

    #[test]
    fn a_clone_tracks_origin_and_fast_forwards_once_it_has_fetched() {
        let t = Tracked::new();
        let clone = t.clone();
        assert_eq!(
            current_upstream(&clone),
            Some(("main".into(), tracked("origin/main", 0, 0)))
        );
        assert!(origin_url(&clone).is_some_and(|u| u.ends_with("remote.git")));

        t.advance_remote();
        // Nothing moves until a fetch: the state is local.
        assert_eq!(
            current_upstream(&clone),
            Some(("main".into(), tracked("origin/main", 0, 0)))
        );
        git_in(&clone, &["fetch"]);
        assert_eq!(
            current_upstream(&clone),
            Some(("main".into(), tracked("origin/main", 0, 1)))
        );

        fast_forward_to_upstream(&clone).expect("fast-forward");
        assert_eq!(
            current_upstream(&clone),
            Some(("main".into(), tracked("origin/main", 0, 0)))
        );
    }

    #[test]
    fn a_diverged_branch_is_not_fast_forwarded() {
        let t = Tracked::new();
        let clone = t.clone();
        git_in(
            &clone,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "local",
            ],
        );
        t.advance_remote();
        git_in(&clone, &["fetch"]);
        assert_eq!(
            current_upstream(&clone),
            Some(("main".into(), tracked("origin/main", 1, 1)))
        );
        assert!(fast_forward_to_upstream(&clone).is_err());
        assert_eq!(
            current_upstream(&clone),
            Some(("main".into(), tracked("origin/main", 1, 1)))
        );
    }

    #[test]
    fn a_detached_head_has_no_upstream() {
        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["checkout", "--detach"]);
        assert_eq!(current_upstream(&repo), None);
    }
}
