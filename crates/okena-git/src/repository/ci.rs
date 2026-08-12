//! CI / PR integration: GitHub PR info and CI check parsing.
//!
//! Self-contained and pure where possible — the `parse_*` functions take JSON
//! strings and are unit-tested directly. The `get_*` functions shell out to
//! `gh` (PR view / pr checks / api).

use std::path::Path;
use std::time::Duration;

use okena_core::process::{command, safe_output_with_timeout};

use super::status::get_pushed_sha;

/// Hard cap on every `gh` invocation. `gh` can hang indefinitely — auth
/// prompts, a stalled network, or `--paginate` chasing Link headers — and the
/// poller runs these for every project. The bus kills the process when this
/// elapses so one stuck repo can never wedge the git-status loop.
const GH_TIMEOUT: Duration = Duration::from_secs(15);

/// Outcome of a PR lookup. `RateLimited` is kept distinct from "no PR" so the
/// poller can park its whole `gh` fan-out instead of hammering a closed door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrFetch {
    Fetched(Option<crate::PrInfo>),
    RateLimited,
}

/// Outcome of a CI lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiFetch {
    /// The upstream commit still matches the caller's cached result, so nothing
    /// was requested and the cached summary stays valid.
    Unchanged,
    /// A request ran. `sha` is the upstream commit the summary describes, used
    /// to skip the next fetch while it holds.
    Fetched {
        sha: Option<String>,
        summary: Option<crate::CiCheckSummary>,
    },
    RateLimited,
}

/// Whether a failed `gh` invocation failed because GitHub is rate-limiting us.
///
/// `gh` surfaces both the primary hourly limit ("API rate limit exceeded") and
/// the secondary abuse limit ("exceeded a secondary rate limit") on stderr; a
/// plain substring match covers the REST and GraphQL wordings alike.
fn is_rate_limited(output: &std::process::Output) -> bool {
    if output.status.success() {
        return false;
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    stderr.contains("rate limit")
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u32,
    title: String,
    head_ref_name: String,
}

/// List open pull requests that can be checked out as worktrees.
pub fn list_pull_requests(
    path: &Path,
    limit: usize,
) -> Result<Vec<okena_core::api::WorktreePullRequest>, String> {
    let limit = limit.clamp(1, 100).to_string();
    let output = safe_output_with_timeout(
        command("gh")
            .args([
                "pr",
                "list",
                "--json",
                "number,title,headRefName",
                "--limit",
                &limit,
            ])
            .current_dir(path),
        GH_TIMEOUT,
    )
    .map_err(|error| format!("Failed to run GitHub CLI: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "GitHub CLI failed to list pull requests".to_string()
        } else {
            stderr
        });
    }

    parse_pull_request_list(&String::from_utf8_lossy(&output.stdout))
}

fn parse_pull_request_list(
    json: &str,
) -> Result<Vec<okena_core::api::WorktreePullRequest>, String> {
    let pull_requests: Vec<GhPullRequest> = serde_json::from_str(json)
        .map_err(|error| format!("Failed to parse pull requests: {error}"))?;
    Ok(pull_requests
        .into_iter()
        .map(|pull_request| okena_core::api::WorktreePullRequest {
            number: pull_request.number,
            title: pull_request.title,
            branch: pull_request.head_ref_name,
        })
        .collect())
}

/// Whether the repo has any remote pointing at github.com (https or ssh).
///
/// Used to gate `gh` PR/CI polling: repos with no GitHub remote (local-only,
/// GitLab, Bitbucket, …) can never have GitHub PRs/checks, so every `gh` call
/// for them just fails after a round-trip. Skipping them is the bulk of the
/// poller's `gh` fan-out on a machine with many non-GitHub projects.
pub fn has_github_remote(path: &Path) -> bool {
    let Some(repo) = crate::gix_helpers::open(path) else {
        return false;
    };
    let names = repo.remote_names();
    for name in names.iter() {
        let Ok(remote) = repo.find_remote(name.as_ref()) else {
            continue;
        };
        for dir in [gix::remote::Direction::Fetch, gix::remote::Direction::Push] {
            if let Some(host) = remote.url(dir).and_then(|u| u.host())
                && (host == "github.com" || host.ends_with(".github.com"))
            {
                return true;
            }
        }
    }
    false
}

/// Get PR info for the current branch (if any PR exists).
/// Requires the GitHub CLI to be installed and authenticated.
///
/// Uses `gh pr list --head <branch>` rather than `gh pr view`: the latter
/// derives the PR's head repository from the branch's tracking remote, so on
/// repos where `origin` points at a different fork than where the PR actually
/// lives (a fork/upstream split) it reports "no pull requests found" even when
/// a PR exists. `gh pr list --head` matches by head branch name in the default
/// repo, which resolves correctly in that setup.
pub fn fetch_pr_info(path: &Path) -> PrFetch {
    let Some(path_str) = path.to_str() else {
        return PrFetch::Fetched(None);
    };
    let Some(branch) = super::status::get_current_branch(path) else {
        return PrFetch::Fetched(None);
    };
    let current_sha = super::status::get_head_sha(path);
    let pushed_sha = get_pushed_sha(path);

    let output = safe_output_with_timeout(
        command("gh")
            .args([
                "pr",
                "list",
                "--head",
                &branch,
                "--state",
                "all",
                "--limit",
                "1",
                "--json",
                "url,state,isDraft,number,baseRefName,headRefOid",
                "--jq",
                ".[0] | [.url, .state, .isDraft, .number, .baseRefName, .headRefOid] | @tsv",
            ])
            .current_dir(path_str),
        GH_TIMEOUT,
    );
    let Ok(output) = output else {
        return PrFetch::Fetched(None);
    };

    if is_rate_limited(&output) {
        return PrFetch::RateLimited;
    }
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return PrFetch::Fetched(parse_pr_list_tsv(
            stdout.trim(),
            current_sha.as_deref(),
            pushed_sha.as_deref(),
        ));
    }

    PrFetch::Fetched(None)
}

/// Parse the single TSV line our `gh pr list --jq ... | @tsv` query emits into a
/// [`PrInfo`]. Fields, in order: url, state, isDraft, number, baseRefName,
/// headRefOid. Returns `None` for an empty line, no PR, or a closed PR whose
/// head is no longer the current branch head.
fn parse_pr_list_tsv(
    line: &str,
    current_sha: Option<&str>,
    pushed_sha: Option<&str>,
) -> Option<crate::PrInfo> {
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 4 || !parts[0].starts_with("http") {
        return None;
    }
    let url = parts[0].to_string();
    let raw_state = parts[1];
    let is_draft = parts[2] == "true";
    let number = parts[3].parse::<u32>().unwrap_or(0);
    let head_oid = parts.get(5).map(|s| s.trim()).filter(|s| !s.is_empty());
    let is_closed_pr = !is_draft && matches!(raw_state, "MERGED" | "CLOSED");
    if is_closed_pr && !closed_pr_head_matches(head_oid, current_sha, pushed_sha) {
        return None;
    }
    let state = if is_draft {
        crate::PrState::Draft
    } else {
        match raw_state {
            "OPEN" => crate::PrState::Open,
            "MERGED" => crate::PrState::Merged,
            "CLOSED" => crate::PrState::Closed,
            other => {
                log::warn!("Unknown PR state '{}', defaulting to Open", other);
                crate::PrState::Open
            }
        }
    };
    // baseRefName: the PR's target branch. Absent/empty -> None.
    let base = parts
        .get(4)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some(crate::PrInfo {
        url,
        state,
        number,
        base,
    })
}

fn closed_pr_head_matches(
    head_oid: Option<&str>,
    current_sha: Option<&str>,
    pushed_sha: Option<&str>,
) -> bool {
    let Some(head_oid) = head_oid else {
        return false;
    };
    current_sha == Some(head_oid) || pushed_sha == Some(head_oid)
}

/// Compute elapsed milliseconds between two ISO-8601 timestamps (those
/// returned by `gh pr checks --json startedAt,completedAt`). Returns 0
/// when either timestamp is missing or unparseable — interpreted as
/// "still running" / "unknown" by the UI.
fn compute_elapsed_ms(started: Option<&str>, completed: Option<&str>) -> u64 {
    let (Some(s), Some(c)) = (started, completed) else {
        return 0;
    };
    let started_s = gix::date::parse(s, None).ok().map(|t| t.seconds);
    let completed_s = gix::date::parse(c, None).ok().map(|t| t.seconds);
    match (started_s, completed_s) {
        (Some(a), Some(b)) if b >= a => ((b - a) * 1000) as u64,
        _ => 0,
    }
}

/// Parse CI check entries from a JSON array string (extracted for testability).
/// Each entry may carry `bucket`, `name`, `workflow`, `link`, `description`,
/// and `elapsed` (milliseconds). Skipped checks are kept in the per-check
/// list (flagged via `is_skipped`) but do not count toward the rollup totals.
pub(crate) fn parse_ci_checks(json_str: &str) -> Option<crate::CiCheckSummary> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(json_str).ok()?;

    if entries.is_empty() {
        return None;
    }

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut pending = 0usize;
    let mut checks: Vec<crate::CiCheck> = Vec::with_capacity(entries.len());

    for entry in &entries {
        let bucket = entry.get("bucket").and_then(|v| v.as_str()).unwrap_or("");
        let (status, is_skipped) = match bucket {
            "pass" => {
                passed += 1;
                (crate::CiStatus::Success, false)
            }
            "fail" | "cancel" => {
                failed += 1;
                (crate::CiStatus::Failure, false)
            }
            "pending" => {
                pending += 1;
                (crate::CiStatus::Pending, false)
            }
            "skipping" => (crate::CiStatus::Pending, true),
            _ => continue,
        };

        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("(unnamed)")
            .to_string();
        let workflow = entry
            .get("workflow")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let link = entry
            .get("link")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let description = entry
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let elapsed_ms = compute_elapsed_ms(
            entry.get("startedAt").and_then(|v| v.as_str()),
            entry.get("completedAt").and_then(|v| v.as_str()),
        );

        checks.push(crate::CiCheck {
            name,
            workflow,
            status,
            is_skipped,
            link,
            description,
            elapsed_ms,
        });
    }

    let total = passed + failed + pending;
    if total == 0 && checks.is_empty() {
        return None;
    }
    // Rollup uses only non-skipped buckets so a workflow of all-skipped
    // checks doesn't surface as a "passing" status. If everything was
    // skipped we return None — there's nothing actionable to display.
    if total == 0 {
        return None;
    }

    let status = if failed > 0 {
        crate::CiStatus::Failure
    } else if pending > 0 {
        crate::CiStatus::Pending
    } else {
        crate::CiStatus::Success
    };

    Some(crate::CiCheckSummary {
        status,
        passed,
        failed,
        pending,
        total,
        checks,
    })
}

/// Get CI check status for the current branch.
///
/// With a known PR number, uses `gh pr checks <number>` (covers Actions +
/// external status checks aggregated by the PR). Otherwise falls back to
/// fetching `check-runs` + `status` on the current HEAD commit via `gh api`,
/// which works for any pushed branch — including default branches without a PR.
///
/// `unchanged_sha` is the upstream commit a *settled* cached summary describes.
/// Checks on a given commit only move while something is running, so when the
/// branch still points at that commit the whole lookup is skipped — no `gh`, no
/// request. This is what keeps a machine with many projects inside GitHub's
/// hourly budget: a repo parked on `main` costs one cheap local ref read per
/// poll instead of two REST calls.
///
/// Returns `Fetched(None)` when there are no checks, when `gh` isn't installed /
/// authenticated, or when the repo has no GitHub remote.
pub fn fetch_ci_checks(
    path: &Path,
    pr_number: Option<u32>,
    unchanged_sha: Option<&str>,
) -> CiFetch {
    // Read locally (gix, no network) before deciding to spend a request.
    let sha = get_pushed_sha(path);
    if let (Some(sha), Some(cached)) = (sha.as_deref(), unchanged_sha)
        && sha == cached
    {
        return CiFetch::Unchanged;
    }

    match pr_number {
        Some(number) => get_pr_ci_checks(path, number, sha),
        None => get_branch_ci_checks(path, sha),
    }
}

/// Fetch CI checks via `gh pr checks <number>` (PR-scoped — see `fetch_ci_checks`).
/// The explicit PR number is passed rather than relying on `gh`'s current-branch
/// resolution, which (like `gh pr view`) misfires on fork/upstream-split repos.
fn get_pr_ci_checks(path: &Path, pr_number: u32, sha: Option<String>) -> CiFetch {
    let Some(path_str) = path.to_str() else {
        return CiFetch::Fetched { sha, summary: None };
    };
    let number = pr_number.to_string();

    let output = safe_output_with_timeout(
        command("gh")
            .args([
                "pr",
                "checks",
                &number,
                "--json",
                "bucket,name,workflow,link,description,startedAt,completedAt",
            ])
            .current_dir(path_str),
        GH_TIMEOUT,
    );
    let Ok(output) = output else {
        return CiFetch::Fetched { sha, summary: None };
    };

    if is_rate_limited(&output) {
        return CiFetch::RateLimited;
    }
    if !output.status.success() {
        return CiFetch::Fetched { sha, summary: None };
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    CiFetch::Fetched {
        summary: parse_ci_checks(stdout.trim()),
        sha,
    }
}

/// Fetch CI checks for the branch's last *pushed* commit via the REST API.
/// Combines GitHub Actions check-runs with the older commit-status API
/// (which is what services like Vercel, CircleCI deploy bots, etc. still
/// use) into a single `CiCheckSummary`.
///
/// Uses the upstream (pushed) SHA, not the local HEAD: CI runs on what's on the
/// remote, so an unpushed local commit has no checks. Returns no summary
/// (skipping the API calls) when the branch has no upstream — nothing has been
/// pushed.
fn get_branch_ci_checks(path: &Path, sha: Option<String>) -> CiFetch {
    let (Some(path_str), Some(sha)) = (path.to_str(), sha) else {
        return CiFetch::Fetched {
            sha: None,
            summary: None,
        };
    };

    // `gh api` substitutes `{owner}` and `{repo}` from the current repo
    // context, so we don't need to resolve the remote ourselves.
    let check_runs_endpoint = format!("repos/{{owner}}/{{repo}}/commits/{}/check-runs", sha);
    let status_endpoint = format!("repos/{{owner}}/{{repo}}/commits/{}/status", sha);
    let fetched = |summary| CiFetch::Fetched {
        sha: Some(sha.clone()),
        summary,
    };

    let Ok(check_runs_out) = safe_output_with_timeout(
        command("gh")
            .args(["api", "--paginate", &check_runs_endpoint])
            .current_dir(path_str),
        GH_TIMEOUT,
    ) else {
        return fetched(None);
    };
    if is_rate_limited(&check_runs_out) {
        return CiFetch::RateLimited;
    }

    let Ok(statuses_out) = safe_output_with_timeout(
        command("gh")
            .args(["api", &status_endpoint])
            .current_dir(path_str),
        GH_TIMEOUT,
    ) else {
        return fetched(None);
    };
    if is_rate_limited(&statuses_out) {
        return CiFetch::RateLimited;
    }

    let check_runs_json = if check_runs_out.status.success() {
        Some(String::from_utf8_lossy(&check_runs_out.stdout).into_owned())
    } else {
        None
    };
    let statuses_json = if statuses_out.status.success() {
        Some(String::from_utf8_lossy(&statuses_out.stdout).into_owned())
    } else {
        None
    };

    if check_runs_json.is_none() && statuses_json.is_none() {
        return fetched(None);
    }

    fetched(parse_branch_ci(
        check_runs_json.as_deref(),
        statuses_json.as_deref(),
    ))
}

/// Parse the REST `check-runs` + `status` JSON payloads into a unified
/// `CiCheckSummary`. Either input may be `None` (the other endpoint still
/// supplies usable data); both being empty produces `None`.
///
/// `check-runs` is the modern GitHub Actions API — bucketing matches
/// `gh pr checks` conventions (`pass`/`fail`/`pending`/`skipping`).
/// `statuses` is the legacy commit-status API used by external services
/// (Vercel, CircleCI deploy bots, …) — `state` is `success`/`failure`/
/// `error`/`pending`.
pub(crate) fn parse_branch_ci(
    check_runs_json: Option<&str>,
    statuses_json: Option<&str>,
) -> Option<crate::CiCheckSummary> {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut pending = 0usize;
    let mut checks: Vec<crate::CiCheck> = Vec::new();

    if let Some(json) = check_runs_json {
        // `gh api --paginate` concatenates pages of objects by repeating the
        // top-level envelope. Try parsing as a single object first; on failure
        // fall through to a more permissive multi-object scan.
        let mut runs: Vec<serde_json::Value> = Vec::new();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
            if let Some(arr) = v.get("check_runs").and_then(|x| x.as_array()) {
                runs.extend(arr.iter().cloned());
            }
        } else {
            // Concatenated pages — split on top-level `}{` boundaries.
            for chunk in json.split("}{").map(|s| s.to_string()).collect::<Vec<_>>() {
                let normalized = if !chunk.starts_with('{') {
                    format!("{{{chunk}")
                } else {
                    chunk.clone()
                };
                let normalized = if !normalized.ends_with('}') {
                    format!("{normalized}}}")
                } else {
                    normalized
                };
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&normalized)
                    && let Some(arr) = v.get("check_runs").and_then(|x| x.as_array())
                {
                    runs.extend(arr.iter().cloned());
                }
            }
        }

        for run in runs {
            let name = run
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("(unnamed)")
                .to_string();
            let status_str = run.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let conclusion = run.get("conclusion").and_then(|v| v.as_str()).unwrap_or("");
            let (status, is_skipped) = match (status_str, conclusion) {
                (_, "success") => {
                    passed += 1;
                    (crate::CiStatus::Success, false)
                }
                (_, "failure")
                | (_, "timed_out")
                | (_, "action_required")
                | (_, "cancelled")
                | (_, "stale")
                | (_, "startup_failure") => {
                    failed += 1;
                    (crate::CiStatus::Failure, false)
                }
                (_, "skipped") | (_, "neutral") => (crate::CiStatus::Pending, true),
                ("queued", _)
                | ("in_progress", _)
                | ("waiting", _)
                | ("pending", _)
                | ("requested", _) => {
                    pending += 1;
                    (crate::CiStatus::Pending, false)
                }
                _ => continue,
            };
            let link = run
                .get("html_url")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            let description = run
                .get("output")
                .and_then(|o| o.get("summary"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            let workflow = run
                .get("check_suite")
                .and_then(|s| s.get("workflow_id"))
                .and_then(|_| {
                    run.get("app")
                        .and_then(|a| a.get("name"))
                        .and_then(|v| v.as_str())
                })
                .filter(|s| !s.is_empty())
                .map(String::from);
            let elapsed_ms = compute_elapsed_ms(
                run.get("started_at").and_then(|v| v.as_str()),
                run.get("completed_at").and_then(|v| v.as_str()),
            );

            checks.push(crate::CiCheck {
                name,
                workflow,
                status,
                is_skipped,
                link,
                description,
                elapsed_ms,
            });
        }
    }

    if let Some(json) = statuses_json
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(json)
        && let Some(arr) = v.get("statuses").and_then(|x| x.as_array())
    {
        for st in arr {
            let name = st
                .get("context")
                .and_then(|v| v.as_str())
                .unwrap_or("(unnamed)")
                .to_string();
            let state = st.get("state").and_then(|v| v.as_str()).unwrap_or("");
            let (status, is_skipped) = match state {
                "success" => {
                    passed += 1;
                    (crate::CiStatus::Success, false)
                }
                "failure" | "error" => {
                    failed += 1;
                    (crate::CiStatus::Failure, false)
                }
                "pending" => {
                    pending += 1;
                    (crate::CiStatus::Pending, false)
                }
                _ => continue,
            };
            let link = st
                .get("target_url")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            let description = st
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            let elapsed_ms = compute_elapsed_ms(
                st.get("created_at").and_then(|v| v.as_str()),
                st.get("updated_at").and_then(|v| v.as_str()),
            );
            checks.push(crate::CiCheck {
                name,
                workflow: None,
                status,
                is_skipped,
                link,
                description,
                elapsed_ms,
            });
        }
    }

    let total = passed + failed + pending;
    if total == 0 && checks.is_empty() {
        return None;
    }
    if total == 0 {
        // Everything was skipped — nothing actionable.
        return None;
    }

    let status = if failed > 0 {
        crate::CiStatus::Failure
    } else if pending > 0 {
        crate::CiStatus::Pending
    } else {
        crate::CiStatus::Success
    };

    Some(crate::CiCheckSummary {
        status,
        passed,
        failed,
        pending,
        total,
        checks,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_worktree_pull_requests() {
        let json = r#"[{"number":12,"title":"Remote worktree","headRefName":"feature/remote"}]"#;
        let pull_requests = super::parse_pull_request_list(json).expect("should parse");
        assert_eq!(pull_requests.len(), 1);
        assert_eq!(pull_requests[0].number, 12);
        assert_eq!(pull_requests[0].title, "Remote worktree");
        assert_eq!(pull_requests[0].branch, "feature/remote");
    }

    #[test]
    fn malformed_worktree_pull_requests_are_rejected() {
        assert!(super::parse_pull_request_list("not json").is_err());
    }

    // ─── PR list TSV parsing tests ─────────────────────────────────────

    #[test]
    fn parse_pr_list_tsv_captures_base_branch() {
        let line = "https://github.com/o/r/pull/9\tOPEN\tfalse\t9\tdevelop\tabc123";
        let pr = super::parse_pr_list_tsv(line, Some("different"), None).expect("should parse");
        assert_eq!(pr.url, "https://github.com/o/r/pull/9");
        assert_eq!(pr.state, crate::PrState::Open);
        assert_eq!(pr.number, 9);
        assert_eq!(pr.base.as_deref(), Some("develop"));
    }

    #[test]
    fn parse_pr_list_tsv_base_absent_is_none() {
        // Older `gh` without baseRefName: only four fields.
        let line = "https://github.com/o/r/pull/3\tOPEN\tfalse\t3";
        let pr = super::parse_pr_list_tsv(line, None, None).expect("should parse");
        assert_eq!(pr.state, crate::PrState::Open);
        assert_eq!(pr.number, 3);
        assert_eq!(pr.base, None);
    }

    #[test]
    fn parse_pr_list_tsv_draft_overrides_state() {
        let line = "https://github.com/o/r/pull/5\tOPEN\ttrue\t5\tmain\tabc123";
        let pr = super::parse_pr_list_tsv(line, Some("different"), None).expect("should parse");
        assert_eq!(pr.state, crate::PrState::Draft);
        assert_eq!(pr.base.as_deref(), Some("main"));
    }

    #[test]
    fn parse_pr_list_tsv_keeps_closed_pr_at_current_head() {
        let line = "https://github.com/o/r/pull/3\tMERGED\tfalse\t3\tmain\tabc123";
        let pr = super::parse_pr_list_tsv(line, Some("abc123"), None).expect("should parse");
        assert_eq!(pr.state, crate::PrState::Merged);
        assert_eq!(pr.number, 3);
    }

    #[test]
    fn parse_pr_list_tsv_ignores_stale_closed_pr() {
        let line = "https://github.com/o/r/pull/4\tCLOSED\tfalse\t4\tmain\toldsha";
        assert!(super::parse_pr_list_tsv(line, Some("newsha"), Some("newsha")).is_none());
    }

    #[test]
    fn parse_pr_list_tsv_empty_or_no_pr_is_none() {
        assert!(super::parse_pr_list_tsv("", None, None).is_none());
        // A jq null/empty result — no leading URL.
        assert!(super::parse_pr_list_tsv("\t\t\t", None, None).is_none());
    }

    // ─── CI check parsing tests ────────────────────────────────────────

    #[test]
    fn parse_ci_all_pass() {
        let json = r#"[{"bucket":"pass"},{"bucket":"pass"},{"bucket":"pass"}]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.status, crate::CiStatus::Success);
        assert_eq!(result.passed, 3);
        assert_eq!(result.failed, 0);
        assert_eq!(result.pending, 0);
        assert_eq!(result.total, 3);
    }

    #[test]
    fn parse_ci_with_failure() {
        let json = r#"[{"bucket":"pass"},{"bucket":"fail"},{"bucket":"pass"}]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.status, crate::CiStatus::Failure);
        assert_eq!(result.passed, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.total, 3);
    }

    #[test]
    fn parse_ci_with_pending() {
        let json = r#"[{"bucket":"pass"},{"bucket":"pending"},{"bucket":"pending"}]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.status, crate::CiStatus::Pending);
        assert_eq!(result.passed, 1);
        assert_eq!(result.pending, 2);
        assert_eq!(result.total, 3);
    }

    #[test]
    fn parse_ci_skipping_excluded_from_total() {
        let json = r#"[{"bucket":"pass"},{"bucket":"skipping"},{"bucket":"pass"}]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.status, crate::CiStatus::Success);
        assert_eq!(result.passed, 2);
        assert_eq!(result.total, 2);
    }

    #[test]
    fn parse_ci_cancel_counts_as_failure() {
        let json = r#"[{"bucket":"pass"},{"bucket":"cancel"}]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.status, crate::CiStatus::Failure);
        assert_eq!(result.failed, 1);
    }

    #[test]
    fn parse_ci_empty_array() {
        assert!(super::parse_ci_checks("[]").is_none());
    }

    #[test]
    fn parse_ci_invalid_json() {
        assert!(super::parse_ci_checks("not json").is_none());
    }

    #[test]
    fn parse_ci_only_skipping() {
        let json = r#"[{"bucket":"skipping"},{"bucket":"skipping"}]"#;
        assert!(super::parse_ci_checks(json).is_none());
    }

    #[test]
    fn parse_ci_captures_per_check_details() {
        let json = r#"[
            {"bucket":"pass","name":"Lint","workflow":"CI","link":"https://ex/1","startedAt":"2024-01-01T10:00:00Z","completedAt":"2024-01-01T10:01:12Z","description":"ok"},
            {"bucket":"fail","name":"Test (macos)","workflow":"CI","link":"https://ex/2","startedAt":"2024-01-01T10:00:00Z","completedAt":"2024-01-01T10:02:51Z"},
            {"bucket":"skipping","name":"Deploy","workflow":"CI"}
        ]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.total, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.checks.len(), 3);

        let lint = &result.checks[0];
        assert_eq!(lint.name, "Lint");
        assert_eq!(lint.workflow.as_deref(), Some("CI"));
        assert_eq!(lint.link.as_deref(), Some("https://ex/1"));
        assert_eq!(lint.description.as_deref(), Some("ok"));
        assert_eq!(lint.elapsed_ms, 72_000);
        assert_eq!(lint.elapsed_label(), "1m12s");
        assert!(!lint.is_skipped);

        let deploy = &result.checks[2];
        assert!(deploy.is_skipped);
        assert_eq!(deploy.elapsed_ms, 0);
        assert_eq!(deploy.elapsed_label(), "\u{2014}");
    }

    // ─── branch-level CI parsing tests ─────────────────────────────────

    #[test]
    fn parse_branch_ci_check_runs_only() {
        let json = r#"{
            "total_count": 3,
            "check_runs": [
                {"name":"Lint","status":"completed","conclusion":"success","html_url":"https://x/1","started_at":"2024-01-01T10:00:00Z","completed_at":"2024-01-01T10:00:30Z"},
                {"name":"Test","status":"completed","conclusion":"failure","html_url":"https://x/2","started_at":"2024-01-01T10:00:00Z","completed_at":"2024-01-01T10:01:00Z"},
                {"name":"Deploy","status":"in_progress","conclusion":null}
            ]
        }"#;
        let result = super::parse_branch_ci(Some(json), None).unwrap();
        assert_eq!(result.status, crate::CiStatus::Failure);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.pending, 1);
        assert_eq!(result.total, 3);
        assert_eq!(result.checks.len(), 3);
        assert_eq!(result.checks[0].link.as_deref(), Some("https://x/1"));
        assert_eq!(result.checks[0].elapsed_ms, 30_000);
    }

    #[test]
    fn parse_branch_ci_skipped_and_neutral_excluded_from_total() {
        let json = r#"{
            "check_runs": [
                {"name":"A","status":"completed","conclusion":"success"},
                {"name":"B","status":"completed","conclusion":"skipped"},
                {"name":"C","status":"completed","conclusion":"neutral"}
            ]
        }"#;
        let result = super::parse_branch_ci(Some(json), None).unwrap();
        assert_eq!(result.status, crate::CiStatus::Success);
        assert_eq!(result.passed, 1);
        assert_eq!(result.total, 1);
        // Skipped/neutral still appear in the per-check list, marked as skipped.
        assert_eq!(result.checks.len(), 3);
        assert!(result.checks.iter().filter(|c| c.is_skipped).count() == 2);
    }

    #[test]
    fn parse_branch_ci_statuses_only() {
        let json = r#"{
            "state": "success",
            "statuses": [
                {"context":"vercel/deploy","state":"success","target_url":"https://v/1","description":"ok","created_at":"2024-01-01T10:00:00Z","updated_at":"2024-01-01T10:00:42Z"},
                {"context":"netlify","state":"pending"}
            ]
        }"#;
        let result = super::parse_branch_ci(None, Some(json)).unwrap();
        assert_eq!(result.status, crate::CiStatus::Pending);
        assert_eq!(result.passed, 1);
        assert_eq!(result.pending, 1);
        assert_eq!(result.total, 2);
        assert_eq!(result.checks[0].name, "vercel/deploy");
        assert_eq!(result.checks[0].elapsed_ms, 42_000);
    }

    #[test]
    fn parse_branch_ci_combines_runs_and_statuses() {
        let runs =
            r#"{"check_runs":[{"name":"Lint","status":"completed","conclusion":"success"}]}"#;
        let statuses = r#"{"statuses":[{"context":"vercel/deploy","state":"failure"}]}"#;
        let result = super::parse_branch_ci(Some(runs), Some(statuses)).unwrap();
        assert_eq!(result.status, crate::CiStatus::Failure);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.total, 2);
        assert_eq!(result.checks.len(), 2);
    }

    #[test]
    fn parse_branch_ci_both_empty_returns_none() {
        let runs = r#"{"check_runs":[]}"#;
        let statuses = r#"{"statuses":[]}"#;
        assert!(super::parse_branch_ci(Some(runs), Some(statuses)).is_none());
    }

    #[test]
    fn parse_branch_ci_only_skipped_returns_none() {
        let runs = r#"{"check_runs":[{"name":"A","status":"completed","conclusion":"skipped"}]}"#;
        assert!(super::parse_branch_ci(Some(runs), None).is_none());
    }

    #[test]
    fn parse_branch_ci_invalid_json_returns_none() {
        assert!(super::parse_branch_ci(Some("not json"), Some("also not json")).is_none());
    }

    // Builds an `ExitStatus` from a raw wait status, so it follows its only
    // caller in being Unix-only.
    #[cfg(unix)]
    fn output(code: i32, stderr: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    #[cfg(unix)]
    fn rate_limit_is_told_apart_from_other_failures() {
        assert!(super::is_rate_limited(&output(
            1,
            "gh: API rate limit exceeded for user ID 1276059."
        )));
        assert!(super::is_rate_limited(&output(
            1,
            "You have exceeded a secondary rate limit"
        )));
        assert!(!super::is_rate_limited(&output(
            1,
            "gh: Not Found (HTTP 404)"
        )));
        // A successful call is never a rate-limit hit, whatever it printed.
        assert!(!super::is_rate_limited(&output(0, "rate limit")));
    }

    #[test]
    fn unchanged_upstream_commit_skips_the_lookup_entirely() {
        // A repo whose upstream still points at the cached commit must not
        // shell out to `gh` at all — this is the skip that keeps the poller
        // inside GitHub's hourly budget.
        let (_bare_tmp, bare) = {
            let tmp = tempfile::tempdir().expect("create temp dir");
            let path = tmp.path().join("origin.git");
            std::process::Command::new("git")
                .args(["init", "--bare", "-b", "main"])
                .arg(&path)
                .output()
                .expect("git init --bare");
            (tmp, path)
        };
        let (_tmp, repo) = super::super::test_support::init_temp_repo();
        super::super::test_support::git_in(
            &repo,
            &["remote", "add", "origin", bare.to_str().expect("path")],
        );
        super::super::test_support::git_in(&repo, &["push", "-u", "origin", "main"]);

        let sha = super::get_pushed_sha(&repo).expect("branch has an upstream");
        assert_eq!(
            super::fetch_ci_checks(&repo, None, Some(&sha)),
            super::CiFetch::Unchanged
        );
    }

    #[test]
    fn branch_without_upstream_never_reaches_the_api() {
        // Nothing is pushed, so there is nothing CI could have run on.
        let (_tmp, repo) = super::super::test_support::init_temp_repo();
        assert_eq!(
            super::fetch_ci_checks(&repo, None, None),
            super::CiFetch::Fetched {
                sha: None,
                summary: None
            }
        );
    }
}
