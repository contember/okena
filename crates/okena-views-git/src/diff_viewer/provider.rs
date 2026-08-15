//! GitProvider trait and the remote-server (HTTP) implementation.

use futures::future::BoxFuture;
use okena_core::api::ActionRequest;
use okena_core::review::{ReviewComparisonId, ReviewDiffRequest, ReviewInventory};
use okena_git::ExactReviewDiffResponse;
use okena_git::{BranchList, CommitLogEntry, DiffMode, DiffResult, FileDiffSummary};
use okena_review::ReviewStructure;
use serde::de::DeserializeOwned;
use std::hash::{Hash, Hasher};

/// Provides git data from either local git commands or a remote server.
pub trait GitProvider: Send + Sync + 'static {
    fn is_git_repo(&self) -> bool;
    /// True for providers that perform mutations on the local filesystem.
    /// Used by UI to gate destructive actions (e.g. branch switching is
    /// disabled when this is false).
    fn supports_mutations(&self) -> bool {
        true
    }
    fn get_diff(&self, mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String>;
    fn get_review_inventory(
        &self,
        _mode: DiffMode,
    ) -> BoxFuture<'static, Result<ReviewInventory, String>> {
        Box::pin(async { Err("Review inventory is not supported by this provider".to_string()) })
    }
    fn get_review_diff(
        &self,
        _request: ReviewDiffRequest,
    ) -> BoxFuture<'static, Result<ExactReviewDiffResponse, String>> {
        Box::pin(async { Err("Exact review diff is not supported by this provider".to_string()) })
    }
    fn get_review_structure(
        &self,
        _request: ReviewDiffRequest,
    ) -> BoxFuture<'static, Result<ReviewStructure, String>> {
        Box::pin(async { Err("Structured review is not supported by this provider".to_string()) })
    }
    fn get_file_contents(
        &self,
        file_path: &str,
        mode: DiffMode,
    ) -> Result<(Option<String>, Option<String>), String>;
    fn get_diff_file_summary(&self) -> Result<Vec<FileDiffSummary>, String>;
    fn get_commit_graph(
        &self,
        count: usize,
        branch: Option<&str>,
    ) -> Result<Vec<CommitLogEntry>, String>;
    fn list_branches(&self) -> Result<Vec<String>, String>;
    /// List branches split into local/remote with the current branch name.
    /// Default implementation falls back to [`list_branches`] and classifies
    /// remote refs as anything containing a `/`.
    fn list_branches_classified(&self) -> Result<BranchList, String> {
        let all = self.list_branches()?;
        let (remote, local): (Vec<String>, Vec<String>) =
            all.into_iter().partition(|n| n.contains('/'));
        Ok(BranchList {
            local,
            remote,
            current: None,
        })
    }

    // ── Mutations (Phase 1: per-file) ──────────────────────────────────────
    fn stage_file(&self, file_path: &str) -> Result<(), String>;
    fn unstage_file(&self, file_path: &str) -> Result<(), String>;
    fn discard_file(&self, file_path: &str) -> Result<(), String>;
    fn delete_file(&self, file_path: &str) -> Result<(), String>;

    // ── Mutations (Phase 2: branches) ──────────────────────────────────────
    fn checkout_local_branch(&self, _branch: &str) -> Result<(), String> {
        Err("Branch checkout is not supported by this provider".to_string())
    }
    fn checkout_remote_branch(&self, _remote_branch: &str) -> Result<(), String> {
        Err("Branch checkout is not supported by this provider".to_string())
    }
    fn create_and_checkout_branch(
        &self,
        _new_name: &str,
        _start_point: Option<&str>,
    ) -> Result<(), String> {
        Err("Branch creation is not supported by this provider".to_string())
    }

    /// Absolute path of a file in the working tree, used for copy-absolute-path.
    /// Returns None when the provider can't resolve it (e.g. remote without
    /// a sensible local absolute path).
    fn absolute_file_path(&self, file_path: &str) -> Option<String>;
}

/// Remote git provider — fetches git data via HTTP from a remote server.
pub struct RemoteGitProvider {
    client: okena_transport::remote_action::RemoteActionClient,
    project_id: String,
    root: String,
}

impl RemoteGitProvider {
    pub fn new(
        client: okena_transport::remote_action::RemoteActionClient,
        project_id: String,
        root: String,
    ) -> Self {
        Self {
            client,
            project_id,
            root,
        }
    }

    fn post_action(&self, action: ActionRequest) -> Result<Option<serde_json::Value>, String> {
        self.client.post_action(action)
    }

    fn post_json<T>(&self, action: ActionRequest, label: &str) -> Result<T, String>
    where
        T: DeserializeOwned,
    {
        decode_json_response(self.post_action(action)?, label)
    }

    fn post_json_async<T>(
        &self,
        action: ActionRequest,
        label: &'static str,
    ) -> BoxFuture<'static, Result<T, String>>
    where
        T: DeserializeOwned + Send + 'static,
    {
        let client = self.client.clone();
        Box::pin(smol::unblock(move || {
            decode_json_response(client.post_action(action)?, label)
        }))
    }

    fn post_unit(&self, action: ActionRequest) -> Result<(), String> {
        self.post_action(action).map(|_| ())
    }
}

fn decode_json_response<T>(value: Option<serde_json::Value>, label: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let value = value.ok_or_else(|| format!("Missing {label} response"))?;
    serde_json::from_value(value).map_err(|error| format!("Invalid {label} response: {error}"))
}

fn review_inventory_action(project_id: &str, mode: DiffMode) -> ActionRequest {
    ActionRequest::ReviewInventory {
        project_id: project_id.to_string(),
        mode,
    }
}

fn review_diff_action(project_id: &str, request: ReviewDiffRequest) -> ActionRequest {
    ActionRequest::ReviewDiff {
        project_id: project_id.to_string(),
        request,
    }
}

fn review_structure_action(project_id: &str, request: ReviewDiffRequest) -> ActionRequest {
    ActionRequest::ReviewStructure {
        project_id: project_id.to_string(),
        request,
    }
}

fn require_comparison_identity(
    expected: &ReviewComparisonId,
    received: &ReviewComparisonId,
    label: &str,
) -> Result<(), String> {
    if expected == received {
        return Ok(());
    }
    Err(format!(
        "{label} response comparison mismatch (expected tag {:016x}, received tag {:016x})",
        comparison_identity_tag(expected),
        comparison_identity_tag(received)
    ))
}

fn comparison_identity_tag(identity: &ReviewComparisonId) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    identity.hash(&mut hasher);
    hasher.finish()
}

impl GitProvider for RemoteGitProvider {
    fn is_git_repo(&self) -> bool {
        true
    }

    fn supports_mutations(&self) -> bool {
        true
    }

    fn get_diff(&self, mode: DiffMode, ignore_whitespace: bool) -> Result<DiffResult, String> {
        let action = ActionRequest::GitDiff {
            project_id: self.project_id.clone(),
            mode,
            ignore_whitespace,
        };
        self.post_json(action, "diff")
    }

    fn get_review_inventory(
        &self,
        mode: DiffMode,
    ) -> BoxFuture<'static, Result<ReviewInventory, String>> {
        self.post_json_async(
            review_inventory_action(&self.project_id, mode),
            "review inventory",
        )
    }

    fn get_review_diff(
        &self,
        request: ReviewDiffRequest,
    ) -> BoxFuture<'static, Result<ExactReviewDiffResponse, String>> {
        let expected = request.comparison.identity().clone();
        let response = self.post_json_async::<ExactReviewDiffResponse>(
            review_diff_action(&self.project_id, request),
            "exact review diff",
        );
        Box::pin(async move {
            let response = response.await?;
            require_comparison_identity(
                &expected,
                response.comparison().identity(),
                "Exact review diff",
            )?;
            Ok(response)
        })
    }

    fn get_review_structure(
        &self,
        request: ReviewDiffRequest,
    ) -> BoxFuture<'static, Result<ReviewStructure, String>> {
        let expected = request.comparison.identity().clone();
        let response = self.post_json_async::<ReviewStructure>(
            review_structure_action(&self.project_id, request),
            "review structure",
        );
        Box::pin(async move {
            let response = response.await?;
            require_comparison_identity(
                &expected,
                response.comparison().identity(),
                "Review structure",
            )?;
            Ok(response)
        })
    }

    fn get_file_contents(
        &self,
        file_path: &str,
        mode: DiffMode,
    ) -> Result<(Option<String>, Option<String>), String> {
        let action = ActionRequest::GitFileContents {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
            mode,
        };
        let value = self
            .post_action(action)?
            .ok_or_else(|| "Missing file contents response".to_string())?;
        let old = value
            .get("old_content")
            .and_then(|v| v.as_str())
            .map(String::from);
        let new = value
            .get("new_content")
            .and_then(|v| v.as_str())
            .map(String::from);
        Ok((old, new))
    }

    fn get_diff_file_summary(&self) -> Result<Vec<FileDiffSummary>, String> {
        let action = ActionRequest::GitDiffSummary {
            project_id: self.project_id.clone(),
        };
        self.post_json(action, "diff summary")
    }

    fn get_commit_graph(
        &self,
        count: usize,
        branch: Option<&str>,
    ) -> Result<Vec<CommitLogEntry>, String> {
        let action = ActionRequest::GitCommitGraph {
            project_id: self.project_id.clone(),
            count,
            branch: branch.map(String::from),
        };
        self.post_json(action, "commit graph")
    }

    fn list_branches(&self) -> Result<Vec<String>, String> {
        let action = ActionRequest::GitListBranches {
            project_id: self.project_id.clone(),
        };
        self.post_json(action, "branch list")
    }

    fn list_branches_classified(&self) -> Result<BranchList, String> {
        let action = ActionRequest::GitListBranchesClassified {
            project_id: self.project_id.clone(),
        };
        self.post_json(action, "classified branch list")
    }

    fn stage_file(&self, file_path: &str) -> Result<(), String> {
        let action = ActionRequest::GitStageFile {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
        };
        self.post_unit(action)
    }

    fn unstage_file(&self, file_path: &str) -> Result<(), String> {
        let action = ActionRequest::GitUnstageFile {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
        };
        self.post_unit(action)
    }

    fn discard_file(&self, file_path: &str) -> Result<(), String> {
        let action = ActionRequest::GitDiscardFile {
            project_id: self.project_id.clone(),
            file_path: file_path.to_string(),
        };
        self.post_unit(action)
    }

    fn delete_file(&self, file_path: &str) -> Result<(), String> {
        let action = ActionRequest::DeleteFile {
            project_id: self.project_id.clone(),
            relative_path: file_path.to_string(),
        };
        self.post_unit(action)
    }

    fn checkout_local_branch(&self, branch: &str) -> Result<(), String> {
        let action = ActionRequest::GitCheckoutLocalBranch {
            project_id: self.project_id.clone(),
            branch: branch.to_string(),
        };
        self.post_unit(action)
    }

    fn checkout_remote_branch(&self, remote_branch: &str) -> Result<(), String> {
        let action = ActionRequest::GitCheckoutRemoteBranch {
            project_id: self.project_id.clone(),
            remote_branch: remote_branch.to_string(),
        };
        self.post_unit(action)
    }

    fn create_and_checkout_branch(
        &self,
        new_name: &str,
        start_point: Option<&str>,
    ) -> Result<(), String> {
        let action = ActionRequest::GitCreateAndCheckoutBranch {
            project_id: self.project_id.clone(),
            new_name: new_name.to_string(),
            start_point: start_point.map(String::from),
        };
        self.post_unit(action)
    }

    fn absolute_file_path(&self, file_path: &str) -> Option<String> {
        if self.root.is_empty() {
            return None;
        }
        let base = self.root.trim_end_matches(['/', '\\']);
        Some(format!("{}/{}", base, file_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn comparison_json() -> Value {
        comparison_json_for('1', '2', '3')
    }

    fn comparison_json_for(base_digit: char, merge_base_digit: char, head_digit: char) -> Value {
        let requested_base = base_digit.to_string().repeat(40);
        let merge_base = merge_base_digit.to_string().repeat(40);
        let head = head_digit.to_string().repeat(40);
        let identity = format!("branch:merge-base:{requested_base}:{head}:{merge_base}");
        json!({
            "requested": {
                "branch_compare": {
                    "base": "origin/main",
                    "head": "feature/review"
                }
            },
            "requested_base_oid": requested_base,
            "requested_head_oid": head,
            "strategy": "merge_base_to_head",
            "base": { "kind": "commit", "oid": merge_base },
            "head": { "kind": "commit", "oid": head },
            "merge_base_oid": merge_base,
            "identity": identity
        })
    }

    fn coverage_json() -> Value {
        json!({
            "total_items": 0,
            "analyzed_items": 0,
            "pending_items": 0,
            "skipped_items": 0,
            "unsupported_items": 0,
            "failed_items": 0
        })
    }

    fn request() -> ReviewDiffRequest {
        serde_json::from_value(json!({
            "comparison": comparison_json(),
            "ignore_whitespace": false
        }))
        .expect("valid review request")
    }

    #[test]
    fn review_actions_keep_project_and_exact_request() {
        let mode = DiffMode::BranchCompare {
            base: "origin/main".to_string(),
            head: "feature/review".to_string(),
        };
        let request = request();

        assert_eq!(
            serde_json::to_value(review_inventory_action("project-1", mode)).unwrap(),
            json!({
                "action": "review_inventory",
                "project_id": "project-1",
                "mode": {
                    "branch_compare": {
                        "base": "origin/main",
                        "head": "feature/review"
                    }
                }
            })
        );
        for (action, name) in [
            (
                review_diff_action("project-1", request.clone()),
                "review_diff",
            ),
            (
                review_structure_action("project-1", request.clone()),
                "review_structure",
            ),
        ] {
            assert_eq!(
                serde_json::to_value(action).unwrap(),
                json!({
                    "action": name,
                    "project_id": "project-1",
                    "request": {
                        "comparison": comparison_json(),
                        "ignore_whitespace": false
                    }
                })
            );
        }
    }

    #[test]
    fn typed_review_responses_decode_without_empty_fallbacks() {
        let inventory: ReviewInventory = decode_json_response(
            Some(json!({
                "comparison": comparison_json(),
                "totals": {
                    "commits": 0,
                    "files": 0,
                    "files_added": 0,
                    "files_deleted": 0,
                    "files_modified": 0,
                    "files_renamed": 0,
                    "files_copied": 0,
                    "files_type_changed": 0,
                    "files_mode_changed": 0,
                    "submodule_changes": 0,
                    "binary_files": 0,
                    "lines_added": 0,
                    "lines_deleted": 0,
                    "provenance": { "source": "git" }
                },
                "commits": [],
                "files": [],
                "coverage": coverage_json()
            })),
            "review inventory",
        )
        .expect("typed inventory");
        assert_eq!(inventory.comparison.identity().0, comparison_identity());

        let diff: ExactReviewDiffResponse = decode_json_response(
            Some(json!({
                "comparison": comparison_json(),
                "diff": { "files": [] }
            })),
            "exact review diff",
        )
        .expect("typed exact diff");
        assert_eq!(diff.comparison().identity().0, comparison_identity());
        assert!(diff.diff().is_empty());

        let structure: ReviewStructure =
            decode_json_response(Some(valid_structure_json()), "review structure")
                .expect("typed review structure");
        assert_eq!(structure.comparison().identity().0, comparison_identity());
        assert!(structure.files().is_empty());
    }

    #[test]
    fn missing_or_malformed_review_responses_are_errors() {
        let missing = decode_json_response::<ReviewInventory>(None, "review inventory")
            .expect_err("missing response must fail");
        assert_eq!(missing, "Missing review inventory response");

        let mut malformed = valid_structure_json();
        malformed["coverage"]["total_items"] = json!(1);
        let error = decode_json_response::<ReviewStructure>(Some(malformed), "review structure")
            .expect_err("invalid coverage must fail");
        assert!(error.starts_with("Invalid review structure response:"));
    }

    #[test]
    fn exact_review_responses_must_match_the_requested_identity() {
        let expected = request().comparison.identity().clone();
        let matching_diff: ExactReviewDiffResponse = decode_json_response(
            Some(json!({
                "comparison": comparison_json(),
                "diff": { "files": [] }
            })),
            "exact review diff",
        )
        .expect("matching diff response");
        require_comparison_identity(
            &expected,
            matching_diff.comparison().identity(),
            "Exact review diff",
        )
        .expect("matching diff identity");

        let mismatched_diff: ExactReviewDiffResponse = decode_json_response(
            Some(json!({
                "comparison": comparison_json_for('4', '5', '6'),
                "diff": { "files": [] }
            })),
            "exact review diff",
        )
        .expect("valid mismatched diff response");
        let diff_error = require_comparison_identity(
            &expected,
            mismatched_diff.comparison().identity(),
            "Exact review diff",
        )
        .expect_err("mismatched diff identity");
        assert_mismatch_is_bounded_and_redacted(&diff_error, &expected);

        let matching_structure: ReviewStructure =
            decode_json_response(Some(valid_structure_json()), "review structure")
                .expect("matching structure response");
        require_comparison_identity(
            &expected,
            matching_structure.comparison().identity(),
            "Review structure",
        )
        .expect("matching structure identity");

        let mismatched_structure: ReviewStructure = decode_json_response(
            Some(structure_json(comparison_json_for('4', '5', '6'))),
            "review structure",
        )
        .expect("valid mismatched structure response");
        let structure_error = require_comparison_identity(
            &expected,
            mismatched_structure.comparison().identity(),
            "Review structure",
        )
        .expect_err("mismatched structure identity");
        assert_mismatch_is_bounded_and_redacted(&structure_error, &expected);
    }

    fn assert_mismatch_is_bounded_and_redacted(error: &str, expected: &ReviewComparisonId) {
        assert!(error.contains("response comparison mismatch"));
        assert!(error.len() < 160, "unbounded mismatch error: {error}");
        assert!(
            !error.contains(&expected.0),
            "mismatch error exposed the raw identity"
        );
    }

    fn valid_structure_json() -> Value {
        structure_json(comparison_json())
    }

    fn structure_json(comparison: Value) -> Value {
        json!({
            "comparison": comparison,
            "files": [],
            "coverage": coverage_json(),
            "language_coverage": [],
            "errors": []
        })
    }

    fn comparison_identity() -> String {
        format!(
            "branch:merge-base:{}:{}:{}",
            "1".repeat(40),
            "3".repeat(40),
            "2".repeat(40)
        )
    }

    struct UnsupportedProvider;

    impl GitProvider for UnsupportedProvider {
        fn is_git_repo(&self) -> bool {
            false
        }

        fn get_diff(
            &self,
            _mode: DiffMode,
            _ignore_whitespace: bool,
        ) -> Result<DiffResult, String> {
            Err("unsupported".to_string())
        }

        fn get_file_contents(
            &self,
            _file_path: &str,
            _mode: DiffMode,
        ) -> Result<(Option<String>, Option<String>), String> {
            Err("unsupported".to_string())
        }

        fn get_diff_file_summary(&self) -> Result<Vec<FileDiffSummary>, String> {
            Err("unsupported".to_string())
        }

        fn get_commit_graph(
            &self,
            _count: usize,
            _branch: Option<&str>,
        ) -> Result<Vec<CommitLogEntry>, String> {
            Err("unsupported".to_string())
        }

        fn list_branches(&self) -> Result<Vec<String>, String> {
            Err("unsupported".to_string())
        }

        fn stage_file(&self, _file_path: &str) -> Result<(), String> {
            Err("unsupported".to_string())
        }

        fn unstage_file(&self, _file_path: &str) -> Result<(), String> {
            Err("unsupported".to_string())
        }

        fn discard_file(&self, _file_path: &str) -> Result<(), String> {
            Err("unsupported".to_string())
        }

        fn delete_file(&self, _file_path: &str) -> Result<(), String> {
            Err("unsupported".to_string())
        }

        fn absolute_file_path(&self, _file_path: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn default_review_methods_fail_explicitly() {
        let provider = UnsupportedProvider;
        assert_eq!(
            smol::block_on(provider.get_review_inventory(DiffMode::WorkingTree))
                .expect_err("unsupported inventory"),
            "Review inventory is not supported by this provider"
        );
        assert_eq!(
            smol::block_on(provider.get_review_diff(request()))
                .expect_err("unsupported exact diff"),
            "Exact review diff is not supported by this provider"
        );
        assert_eq!(
            smol::block_on(provider.get_review_structure(request()))
                .expect_err("unsupported structure"),
            "Structured review is not supported by this provider"
        );
    }
}
