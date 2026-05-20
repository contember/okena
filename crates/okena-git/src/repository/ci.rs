//! CI / PR integration: GitHub PR info and CI check parsing.
//!
//! Self-contained and pure where possible — the `parse_*` functions take JSON
//! strings and are unit-tested directly. The `get_*` functions shell out to
//! `gh` (PR view / pr checks / api).

use std::path::Path;

use okena_core::process::{command, safe_output};

use super::status::get_head_sha;

/// Get PR info for the current branch (if any PR exists).
/// Uses `gh pr view` which requires the GitHub CLI to be installed and authenticated.
pub fn get_pr_info(path: &Path) -> Option<crate::PrInfo> {
    let path_str = path.to_str()?;

    let output = safe_output(
        command("gh")
            .args(["pr", "view", "--json", "url,state,isDraft,number", "--jq", "[.url, .state, .isDraft, .number] | @tsv"])
            .current_dir(path_str),
    )
    .ok()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.trim();
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4 && parts[0].starts_with("http") {
            let url = parts[0].to_string();
            let is_draft = parts[2] == "true";
            let number = parts[3].parse::<u32>().unwrap_or(0);
            let state = if is_draft {
                crate::PrState::Draft
            } else {
                match parts[1] {
                    "OPEN" => crate::PrState::Open,
                    "MERGED" => crate::PrState::Merged,
                    "CLOSED" => crate::PrState::Closed,
                    other => {
                        log::warn!("Unknown PR state '{}', defaulting to Open", other);
                        crate::PrState::Open
                    }
                }
            };
            return Some(crate::PrInfo { url, state, number });
        }
    }

    None
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
/// When `has_pr` is true, uses `gh pr checks` (covers Actions + external
/// status checks aggregated by the PR). Otherwise falls back to fetching
/// `check-runs` + `status` on the current HEAD commit via `gh api`, which
/// works for any pushed branch — including default branches without a PR.
///
/// Returns `None` when there are no checks, when `gh` isn't installed /
/// authenticated, or when the repo has no GitHub remote.
pub fn get_ci_checks(path: &Path, has_pr: bool) -> Option<crate::CiCheckSummary> {
    if has_pr {
        get_pr_ci_checks(path)
    } else {
        get_branch_ci_checks(path)
    }
}

/// Fetch CI checks via `gh pr checks` (PR-scoped — see `get_ci_checks`).
fn get_pr_ci_checks(path: &Path) -> Option<crate::CiCheckSummary> {
    let path_str = path.to_str()?;

    let output = safe_output(
        command("gh")
            .args([
                "pr",
                "checks",
                "--json",
                "bucket,name,workflow,link,description,startedAt,completedAt",
            ])
            .current_dir(path_str),
    )
    .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ci_checks(stdout.trim())
}

/// Fetch CI checks for the current branch's HEAD commit via the REST API.
/// Combines GitHub Actions check-runs with the older commit-status API
/// (which is what services like Vercel, CircleCI deploy bots, etc. still
/// use) into a single `CiCheckSummary`.
fn get_branch_ci_checks(path: &Path) -> Option<crate::CiCheckSummary> {
    let path_str = path.to_str()?;
    let sha = get_head_sha(path)?;

    // `gh api` substitutes `{owner}` and `{repo}` from the current repo
    // context, so we don't need to resolve the remote ourselves.
    let check_runs_endpoint = format!("repos/{{owner}}/{{repo}}/commits/{}/check-runs", sha);
    let status_endpoint = format!("repos/{{owner}}/{{repo}}/commits/{}/status", sha);

    let check_runs_out = safe_output(
        command("gh")
            .args(["api", "--paginate", &check_runs_endpoint])
            .current_dir(path_str),
    )
    .ok()?;

    let statuses_out = safe_output(
        command("gh")
            .args(["api", &status_endpoint])
            .current_dir(path_str),
    )
    .ok()?;

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
        return None;
    }

    parse_branch_ci(check_runs_json.as_deref(), statuses_json.as_deref())
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
                let normalized = if !chunk.starts_with('{') { format!("{{{chunk}") } else { chunk.clone() };
                let normalized = if !normalized.ends_with('}') { format!("{normalized}}}") } else { normalized };
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&normalized)
                    && let Some(arr) = v.get("check_runs").and_then(|x| x.as_array()) {
                        runs.extend(arr.iter().cloned());
                    }
            }
        }

        for run in runs {
            let name = run.get("name").and_then(|v| v.as_str()).unwrap_or("(unnamed)").to_string();
            let status_str = run.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let conclusion = run.get("conclusion").and_then(|v| v.as_str()).unwrap_or("");
            let (status, is_skipped) = match (status_str, conclusion) {
                (_, "success") => { passed += 1; (crate::CiStatus::Success, false) }
                (_, "failure") | (_, "timed_out") | (_, "action_required") | (_, "cancelled") | (_, "stale") | (_, "startup_failure") => {
                    failed += 1;
                    (crate::CiStatus::Failure, false)
                }
                (_, "skipped") | (_, "neutral") => (crate::CiStatus::Pending, true),
                ("queued", _) | ("in_progress", _) | ("waiting", _) | ("pending", _) | ("requested", _) => {
                    pending += 1;
                    (crate::CiStatus::Pending, false)
                }
                _ => continue,
            };
            let link = run.get("html_url").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from);
            let description = run
                .get("output")
                .and_then(|o| o.get("summary"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            let workflow = run
                .get("check_suite")
                .and_then(|s| s.get("workflow_id"))
                .and_then(|_| run.get("app").and_then(|a| a.get("name")).and_then(|v| v.as_str()))
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
            && let Some(arr) = v.get("statuses").and_then(|x| x.as_array()) {
                for st in arr {
                    let name = st.get("context").and_then(|v| v.as_str()).unwrap_or("(unnamed)").to_string();
                    let state = st.get("state").and_then(|v| v.as_str()).unwrap_or("");
                    let (status, is_skipped) = match state {
                        "success" => { passed += 1; (crate::CiStatus::Success, false) }
                        "failure" | "error" => { failed += 1; (crate::CiStatus::Failure, false) }
                        "pending" => { pending += 1; (crate::CiStatus::Pending, false) }
                        _ => continue,
                    };
                    let link = st.get("target_url").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from);
                    let description = st.get("description").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from);
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
        let runs = r#"{"check_runs":[{"name":"Lint","status":"completed","conclusion":"success"}]}"#;
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
}
