//! Open pull requests offered as worktree sources, searchable by text or number.
//!
//! Runs over the shared GitHub HTTP bus ([`super::github`]). An empty query
//! lists the newest open PRs straight off the repository. Text goes through
//! GitHub search (title, body); a query that is a PR number (`123`, `#123`) is
//! also looked up directly, because search does not reliably match on the
//! number and that lookup is what reaches PRs older than the first page.
//!
//! The remote may still name a repository that was since renamed or
//! transferred. `repository(...)` follows the redirect but a `repo:` search
//! qualifier does not, so search runs against the canonical `nameWithOwner`.

use std::path::Path;

use okena_core::api::WorktreePullRequest;
use serde_json::{Value, json};

use super::github::{ApiError, GithubClient, GithubRepo, resolve_base_repo};

const REPOSITORY_QUERY: &str = r#"
query WorktreePullRequests($owner: String!, $repo: String!, $first: Int!, $newest: Boolean!) {
  repository(owner: $owner, name: $repo) {
    nameWithOwner
    pullRequests(
      states: [OPEN],
      first: $first,
      orderBy: {field: CREATED_AT, direction: DESC}
    ) @include(if: $newest) {
      nodes { number title headRefName state }
    }
  }
}"#;

const SEARCH_QUERY: &str = r#"
query WorktreePullRequestSearch($search: String!, $first: Int!) {
  search(type: ISSUE, query: $search, first: $first) {
    nodes { ... on PullRequest {
      number title headRefName state
      repository { nameWithOwner }
    } }
  }
}"#;

const NUMBER_QUERY: &str = r#"
query WorktreePullRequestByNumber($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) { number title headRefName state }
  }
}"#;

/// A `PullRequest` node from a repository query.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrNode {
    number: u32,
    title: String,
    head_ref_name: String,
    state: String,
}

#[derive(serde::Deserialize)]
struct SearchPrNode {
    #[serde(flatten)]
    pr: PrNode,
    repository: SearchRepository,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRepository {
    name_with_owner: String,
}

impl PrNode {
    fn into_open(self) -> Option<WorktreePullRequest> {
        (self.state == "OPEN").then_some(WorktreePullRequest {
            number: self.number,
            title: self.title,
            branch: self.head_ref_name,
        })
    }
}

/// List open pull requests matching `query`; an empty query lists the newest.
pub fn list_pull_requests(
    path: &Path,
    limit: usize,
    query: &str,
) -> Result<Vec<WorktreePullRequest>, String> {
    let repo = resolve_base_repo(path)
        .ok_or_else(|| "No GitHub remote found for this repository".to_string())?;
    let mut client = GithubClient::for_host(&repo.host)
        .ok_or_else(|| "Not authenticated with GitHub — run `gh auth login`".to_string())?;
    let query = query.trim();
    let first = limit.clamp(1, 100);

    let repository = client
        .graphql(
            REPOSITORY_QUERY,
            json!({
                "owner": repo.owner,
                "repo": repo.name,
                "first": first,
                "newest": query.is_empty(),
            }),
        )
        .map_err(describe_error)?;
    if query.is_empty() {
        return Ok(parse_nodes(&repository, "/repository/pullRequests/nodes"));
    }
    let name_with_owner = repository
        .pointer("/repository/nameWithOwner")
        .and_then(Value::as_str)
        .ok_or_else(|| describe_error(ApiError::Failed))?;

    let search = client
        .graphql(
            SEARCH_QUERY,
            json!({ "search": search_string(name_with_owner, query), "first": first }),
        )
        .map_err(describe_error)
        .map(|data| parse_search_nodes(&data, name_with_owner))?;

    let exact = match parse_pr_number(query) {
        Some(number) => lookup_number(&mut client, &repo, number)?,
        None => None,
    };

    Ok(merge_results(exact, search))
}

/// A failed number lookup (typically "no such PR") is not an error — the
/// search results still stand. A rate limit is, since the user can't fix it by
/// typing.
fn lookup_number(
    client: &mut GithubClient,
    repo: &GithubRepo,
    number: u32,
) -> Result<Option<WorktreePullRequest>, String> {
    let variables = json!({ "owner": repo.owner, "repo": repo.name, "number": number });
    match client.graphql(NUMBER_QUERY, variables) {
        Ok(data) => Ok(parse_number_lookup(&data)),
        Err(ApiError::RateLimited) => Err(describe_error(ApiError::RateLimited)),
        Err(ApiError::Failed) => Ok(None),
    }
}

fn describe_error(error: ApiError) -> String {
    match error {
        ApiError::RateLimited => "GitHub API rate limit reached — try again later".to_string(),
        ApiError::Failed => "GitHub request for pull requests failed".to_string(),
    }
}

/// GitHub search string: open PRs of `name_with_owner`, newest first,
/// narrowed by the user's (non-empty) text. The text is passed through, so
/// search qualifiers like `author:someone` work too.
fn search_string(name_with_owner: &str, query: &str) -> String {
    format!("repo:{name_with_owner} is:pr is:open sort:created-desc {query}")
}

/// `123` or `#123` (surrounding whitespace ignored) → `123`.
fn parse_pr_number(query: &str) -> Option<u32> {
    let digits = query.trim();
    let digits = digits.strip_prefix('#').unwrap_or(digits);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The open PRs among the nodes at `pointer`.
fn parse_nodes(data: &Value, pointer: &str) -> Vec<WorktreePullRequest> {
    data.pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|node| serde_json::from_value::<PrNode>(node.clone()).ok())
        .filter_map(PrNode::into_open)
        .collect()
}

fn parse_search_nodes(data: &Value, name_with_owner: &str) -> Vec<WorktreePullRequest> {
    data.pointer("/search/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|node| serde_json::from_value::<SearchPrNode>(node.clone()).ok())
        // Additional repo: qualifiers broaden GitHub search instead of narrowing it.
        .filter(|node| {
            node.repository
                .name_with_owner
                .eq_ignore_ascii_case(name_with_owner)
        })
        .filter_map(|node| node.pr.into_open())
        .collect()
}

fn parse_number_lookup(data: &Value) -> Option<WorktreePullRequest> {
    let node = data.pointer("/repository/pullRequest")?.clone();
    serde_json::from_value::<PrNode>(node)
        .ok()
        .and_then(PrNode::into_open)
}

/// The exact number hit leads, and is not repeated if search found it too.
fn merge_results(
    exact: Option<WorktreePullRequest>,
    search: Vec<WorktreePullRequest>,
) -> Vec<WorktreePullRequest> {
    let Some(exact) = exact else {
        return search;
    };
    let number = exact.number;
    std::iter::once(exact)
        .chain(search.into_iter().filter(|pr| pr.number != number))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: u32) -> WorktreePullRequest {
        WorktreePullRequest {
            number,
            title: format!("PR {number}"),
            branch: format!("branch-{number}"),
        }
    }

    #[test]
    fn pr_number_accepts_plain_and_hash_forms() {
        assert_eq!(parse_pr_number("123"), Some(123));
        assert_eq!(parse_pr_number("#123"), Some(123));
        assert_eq!(parse_pr_number("  #7 "), Some(7));
    }

    #[test]
    fn pr_number_rejects_text_and_empty() {
        assert_eq!(parse_pr_number(""), None);
        assert_eq!(parse_pr_number("#"), None);
        assert_eq!(parse_pr_number("12a"), None);
        assert_eq!(parse_pr_number("-12"), None);
        assert_eq!(parse_pr_number("fix login"), None);
        assert_eq!(parse_pr_number("99999999999"), None);
    }

    #[test]
    fn search_string_scopes_to_open_prs_of_the_repo() {
        assert_eq!(
            search_string("acme/widgets", "login bug"),
            "repo:acme/widgets is:pr is:open sort:created-desc login bug"
        );
    }

    #[test]
    fn newest_prs_parse_from_the_repository_connection() {
        let data = json!({ "repository": { "nameWithOwner": "acme/widgets", "pullRequests": { "nodes": [
            { "number": 3, "title": "T", "headRefName": "b", "state": "OPEN" },
        ] } } });
        let prs = parse_nodes(&data, "/repository/pullRequests/nodes");
        assert_eq!(prs.iter().map(|pr| pr.number).collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn search_parsing_skips_issues_and_closed_prs() {
        let data = json!({ "search": { "nodes": [
            { "number": 12, "title": "Remote worktree", "headRefName": "feature/remote", "state": "OPEN", "repository": { "nameWithOwner": "acme/widgets" } },
            {},
            { "number": 13, "title": "Old", "headRefName": "old", "state": "MERGED", "repository": { "nameWithOwner": "acme/widgets" } },
        ] } });
        assert_eq!(
            parse_search_nodes(&data, "acme/widgets"),
            vec![WorktreePullRequest {
                number: 12,
                title: "Remote worktree".into(),
                branch: "feature/remote".into(),
            }]
        );
    }

    #[test]
    fn search_parsing_tolerates_missing_nodes() {
        assert!(parse_search_nodes(&json!({}), "acme/widgets").is_empty());
    }

    #[test]
    fn search_results_must_belong_to_the_current_repository() {
        let data = json!({ "search": { "nodes": [
            { "number": 12, "title": "Foreign", "headRefName": "main", "state": "OPEN", "repository": { "nameWithOwner": "other/widgets" } },
            { "number": 12, "title": "Local", "headRefName": "feature/local", "state": "OPEN", "repository": { "nameWithOwner": "Acme/Widgets" } },
            { "number": 13, "title": "Unknown", "headRefName": "unknown", "state": "OPEN" },
        ] } });
        assert_eq!(
            parse_search_nodes(&data, "acme/widgets"),
            vec![WorktreePullRequest {
                number: 12,
                title: "Local".into(),
                branch: "feature/local".into(),
            }]
        );
    }

    #[test]
    fn number_lookup_keeps_only_open_prs() {
        let open = json!({ "repository": { "pullRequest":
            { "number": 5, "title": "T", "headRefName": "b", "state": "OPEN" } } });
        assert_eq!(parse_number_lookup(&open).map(|pr| pr.number), Some(5));

        let closed = json!({ "repository": { "pullRequest":
            { "number": 5, "title": "T", "headRefName": "b", "state": "CLOSED" } } });
        assert_eq!(parse_number_lookup(&closed), None);

        let missing = json!({ "repository": { "pullRequest": null } });
        assert_eq!(parse_number_lookup(&missing), None);
    }

    #[test]
    fn merge_puts_exact_hit_first_without_duplicating_it() {
        let merged = merge_results(Some(pr(2)), vec![pr(9), pr(2), pr(1)]);
        let numbers: Vec<u32> = merged.iter().map(|pr| pr.number).collect();
        assert_eq!(numbers, vec![2, 9, 1]);
    }

    #[test]
    fn merge_without_exact_hit_is_the_search() {
        let merged = merge_results(None, vec![pr(9), pr(1)]);
        let numbers: Vec<u32> = merged.iter().map(|pr| pr.number).collect();
        assert_eq!(numbers, vec![9, 1]);
    }
}
