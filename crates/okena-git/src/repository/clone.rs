//! Clone a remote repository into a fresh directory.

use std::path::Path;

use okena_core::process::{CommandBus, CommandHandle, CommandSpec, Lane};

use super::{network_command, path_str, require_success};
use crate::error::{GitError, GitResult};

/// Validate that a clone URL cannot be read by git as an option.
///
/// `git clone` is invoked with a `--` separator, so a leading `-` is already
/// harmless there; this rejects it anyway so the bad input surfaces as a clear
/// error instead of a confusing git failure. Empty URLs are rejected too.
pub fn validate_clone_url(url: &str) -> GitResult<&str> {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return Err(GitError::InvalidUrl(url.to_string()));
    }
    Ok(trimmed)
}

/// Derive the directory name `git clone <url>` would create.
///
/// Mirrors git's own rule: take the last non-empty path segment and drop a
/// trailing `.git`. Handles both URL forms (`https://host/a/b.git`) and the
/// scp-like form (`git@host:a/b.git`). Returns `None` when nothing usable is
/// left, so callers can ask the user for a name instead of guessing.
pub fn clone_dir_name(url: &str) -> Option<String> {
    let url = url.trim();
    // Strip a fragment/query before splitting — `?ref=x` is not part of the name.
    let url = url.split(['?', '#']).next().unwrap_or(url);
    // For a `scheme://host/path` URL the name comes from the path, so drop the
    // scheme and host first — otherwise a hostless `https://` would yield the
    // scheme itself. Everything else (scp-like `git@host:a/b`, a local path) is
    // already just a path.
    let path = match url.split_once("://") {
        Some((_scheme, rest)) => rest.split_once('/')?.1,
        None => url,
    };
    // Both `/` and `:` separate the repo from its host in the forms git accepts.
    let name = path
        .trim_end_matches('/')
        .rsplit(['/', ':', '\\'])
        .find(|segment| !segment.is_empty())?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(name.to_string())
}

/// Reject a clone target that already holds something.
///
/// `git clone` refuses a non-empty directory itself, but checking up-front
/// keeps the failure fast and the message ours.
fn require_absent_clone_target(target_path: &Path) -> GitResult<()> {
    let occupied = match std::fs::read_dir(target_path) {
        Ok(mut entries) => entries.next().is_some(),
        // Not a directory (a file sits there) still counts as occupied.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => target_path.exists(),
    };
    if occupied {
        return Err(GitError::CloneTargetExists {
            path: target_path.to_path_buf(),
        });
    }
    Ok(())
}

/// How far along a `git clone` is, as reported by git itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneProgress {
    /// The phase git names, e.g. `Receiving objects`.
    pub phase: String,
    /// Percent complete within that phase, 0-100.
    pub percent: u8,
}

impl CloneProgress {
    /// One short line for a UI: `Receiving objects: 42%`.
    pub fn summary(&self) -> String {
        format!("{}: {}%", self.phase, self.percent)
    }
}

/// Read one line of `git clone --progress` output.
///
/// The lines that carry progress look like `Receiving objects:  42% (52/123)`,
/// sometimes behind a `remote: ` prefix when the phase runs on the server.
/// Everything else git writes — the opening `Cloning into '...'`, warnings,
/// the final `done.` summaries — carries no percentage and is skipped, so
/// callers can feed it every line without filtering first.
pub fn parse_clone_progress(line: &str) -> Option<CloneProgress> {
    let line = line.trim().strip_prefix("remote: ").unwrap_or(line.trim());
    let (phase, rest) = line.split_once(':')?;
    let phase = phase.trim();
    // Guard against picking up a URL or a path with a colon in it.
    if phase.is_empty() || !phase.chars().all(|c| c.is_ascii_alphabetic() || c == ' ') {
        return None;
    }
    let rest = rest.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !rest[digits.len()..].starts_with('%') {
        return None;
    }
    let percent: u8 = digits.parse().ok().filter(|p| *p <= 100)?;
    Some(CloneProgress {
        phase: phase.to_string(),
        percent,
    })
}

/// Submit `git clone <url> <target_path>` to the process bus.
///
/// Runs on [`Lane::Long`]: a clone is network-bound and unbounded in duration,
/// so it must never occupy an interactive or poller slot.
///
/// `on_progress` is called from the bus reader thread for each progress update
/// git reports, so it must be cheap and must not block.
pub fn start_clone_repository(
    url: &str,
    target_path: &Path,
    on_progress: impl Fn(CloneProgress) + Send + Sync + 'static,
) -> GitResult<CommandHandle> {
    let url = validate_clone_url(url)?;
    require_absent_clone_target(target_path)?;

    let target_str = path_str(target_path)?;
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent).map_err(GitError::CommandFailed)?;
    }

    let mut cmd = network_command();
    // `--progress` is required, not cosmetic: git reports progress only when
    // stderr is a terminal, and the bus always pipes it.
    cmd.args(["clone", "--progress", "--", url, target_str]);
    Ok(CommandBus::global().submit(
        CommandSpec::from_command(&cmd)
            .lane(Lane::Long)
            .label("git clone")
            .on_stderr_line(move |line| {
                if let Some(progress) = parse_clone_progress(line) {
                    on_progress(progress);
                }
            }),
    ))
}

/// Wait for a clone submitted by [`start_clone_repository`].
pub fn finish_clone_repository(handle: CommandHandle) -> GitResult<()> {
    require_success(handle.wait()?)
}

/// Whether a clone into `path` ran all the way to a checked-out commit.
///
/// `git clone` is not atomic: killed mid-fetch it leaves the target directory
/// behind holding a `.git` whose HEAD points at a branch that does not exist
/// yet. That wreckage is indistinguishable from a finished clone by existence
/// alone, so startup recovery asks this instead — otherwise it promotes a
/// repo with no files in it to a normal-looking project.
///
/// Resolving HEAD is the line between the two: git writes it only once the
/// fetch has landed the branch it is about to check out.
pub fn is_complete_checkout(path: &Path) -> bool {
    gix::open(path).is_ok_and(|repo| repo.head_id().is_ok())
}

/// Submit and synchronously wait for `git clone <url> <target_path>`,
/// discarding progress.
pub fn clone_repository(url: &str, target_path: &Path) -> GitResult<()> {
    finish_clone_repository(start_clone_repository(url, target_path, |_| {})?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_the_directory_name_git_would_use() {
        let cases = [
            ("https://github.com/user/okena.git", "okena"),
            ("https://github.com/user/okena", "okena"),
            ("https://github.com/user/okena/", "okena"),
            ("git@github.com:user/okena.git", "okena"),
            ("ssh://git@host:2222/user/okena.git", "okena"),
            ("/srv/repos/okena.git", "okena"),
            ("https://host/user/okena.git?ref=main", "okena"),
            ("  https://host/user/okena.git  ", "okena"),
        ];
        for (url, expected) in cases {
            assert_eq!(clone_dir_name(url).as_deref(), Some(expected), "url: {url}");
        }
    }

    #[test]
    fn rejects_urls_with_no_usable_name() {
        for url in ["", "   ", "/", "https://", "https://host/", "../"] {
            assert_eq!(clone_dir_name(url), None, "url: {url}");
        }
    }

    #[test]
    fn rejects_option_like_and_empty_urls() {
        assert!(validate_clone_url("--upload-pack=evil").is_err());
        assert!(validate_clone_url("-x").is_err());
        assert!(validate_clone_url("   ").is_err());
        assert_eq!(
            validate_clone_url("  https://host/a.git ").ok(),
            Some("https://host/a.git")
        );
    }

    #[test]
    fn reads_the_percentage_out_of_gits_progress_lines() {
        let cases = [
            ("Receiving objects:  42% (52/123)", "Receiving objects", 42),
            (
                "Resolving deltas: 100% (30/30), done.",
                "Resolving deltas",
                100,
            ),
            (
                "remote: Enumerating objects:   7% (1/14)",
                "Enumerating objects",
                7,
            ),
            ("Updating files:   0% (1/900)", "Updating files", 0),
        ];
        for (line, phase, percent) in cases {
            assert_eq!(
                parse_clone_progress(line),
                Some(CloneProgress {
                    phase: phase.to_string(),
                    percent
                }),
                "line: {line}"
            );
        }
    }

    #[test]
    fn lines_without_a_percentage_are_not_progress() {
        for line in [
            "Cloning into '/tmp/repo'...",
            "fatal: could not read Username for 'https://github.com'",
            "warning: redirecting to https://example.com/repo.git/",
            "remote: Total 14 (delta 0), reused 0 (delta 0)",
            "",
            "https://example.com: unreachable",
        ] {
            assert_eq!(parse_clone_progress(line), None, "line: {line}");
        }
    }

    #[test]
    fn a_progress_update_renders_as_one_short_line() {
        let progress = CloneProgress {
            phase: "Receiving objects".to_string(),
            percent: 42,
        };
        assert_eq!(progress.summary(), "Receiving objects: 42%");
    }

    /// End-to-end over a real `git clone`: the parser above is only useful if
    /// git actually emits these lines under the bus's piped stderr, which it
    /// does only because of `--progress`. Uses a `file://` URL so the clone
    /// goes through the real transport (a plain path would hardlink and skip
    /// the transfer) without touching the network.
    #[test]
    fn a_real_clone_reports_progress() {
        use crate::repository::test_support::{git_in, init_temp_repo};
        use std::sync::{Arc, Mutex};

        let (_tmp, source) = init_temp_repo();
        for i in 0..20 {
            std::fs::write(source.join(format!("file{i}.txt")), format!("{i}\n")).unwrap();
        }
        git_in(&source, &["add", "."]);
        git_in(
            &source,
            &["-c", "commit.gpgsign=false", "commit", "-m", "seed"],
        );

        let target =
            std::env::temp_dir().join(format!("okena-clone-progress-{}", uuid::Uuid::new_v4()));
        let seen: Arc<Mutex<Vec<CloneProgress>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();

        let url = format!("file://{}", source.display());
        let handle = start_clone_repository(&url, &target, move |progress| {
            sink.lock().expect("lock").push(progress);
        })
        .expect("start clone");
        finish_clone_repository(handle).expect("clone succeeds");

        let seen = seen.lock().expect("lock");
        assert!(
            !seen.is_empty(),
            "git reported no progress; --progress or the parser regressed"
        );
        assert!(
            seen.iter().all(|p| p.percent <= 100 && !p.phase.is_empty()),
            "malformed progress: {seen:?}"
        );
        assert!(is_complete_checkout(&target), "clone should be complete");

        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn an_interrupted_clone_is_not_mistaken_for_a_finished_one() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        // A finished checkout: HEAD resolves.
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("file.txt"), "a\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "seed"],
        );
        assert!(is_complete_checkout(&repo));

        // What a clone killed mid-fetch leaves: a `.git` with an unborn HEAD.
        let dir = std::env::temp_dir().join(format!("okena-partial-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        git_in(&dir, &["init"]);
        assert!(
            !is_complete_checkout(&dir),
            "a repo with no commit is not a finished clone"
        );

        // Not a repo at all.
        let plain = dir.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(!is_complete_checkout(&plain));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_empty_target_is_rejected_before_git_runs() {
        let dir = std::env::temp_dir().join(format!("okena-clone-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // Missing target is fine.
        assert!(require_absent_clone_target(&dir).is_ok());

        // Existing-but-empty is fine (git accepts it too).
        std::fs::create_dir_all(&dir).unwrap();
        assert!(require_absent_clone_target(&dir).is_ok());

        // Anything inside makes it occupied.
        std::fs::write(dir.join("file"), b"x").unwrap();
        assert!(matches!(
            require_absent_clone_target(&dir),
            Err(GitError::CloneTargetExists { .. })
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
