//! The one review dataset every test in this crate builds on.

use okena_core::review::ReviewInventory;
use okena_git::ExactReviewDiffResponse;
use okena_review::ReviewStructure;
use serde_json::{Value, json};

pub(crate) fn comparison_json() -> Value {
    let base = "1".repeat(40);
    let merge_base = "2".repeat(40);
    let head = "3".repeat(40);
    json!({
        "requested": { "branch_compare": { "base": "main", "head": "feature" } },
        "requested_base_oid": base,
        "requested_head_oid": head,
        "strategy": "merge_base_to_head",
        "base": { "kind": "commit", "oid": merge_base },
        "head": { "kind": "commit", "oid": head },
        "merge_base_oid": merge_base,
        "identity": format!("branch:merge-base:{base}:{head}:{merge_base}")
    })
}

pub(crate) fn coverage_json(total: u64, analyzed: u64, unsupported: u64) -> Value {
    json!({
        "total_items": total,
        "analyzed_items": analyzed,
        "pending_items": 0,
        "skipped_items": 0,
        "unsupported_items": unsupported,
        "failed_items": 0
    })
}

fn totals_json() -> Value {
    json!({
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
    })
}

/// A comparison that resolved but changed nothing.
pub(crate) fn empty_inventory() -> ReviewInventory {
    serde_json::from_value(json!({
        "comparison": comparison_json(),
        "totals": totals_json(),
        "commits": [],
        "files": [],
        "coverage": coverage_json(0, 0, 0)
    }))
    .expect("empty inventory fixture")
}

/// One file per role family: implementation, test, rename, docs, config,
/// lockfile, binary. `src/lib.rs` and `tests/lib.rs` come first, in that order.
pub(crate) fn inventory() -> ReviewInventory {
    let mut totals = totals_json();
    totals["commits"] = json!(2);
    totals["files"] = json!(7);
    totals["files_added"] = json!(2);
    totals["files_modified"] = json!(4);
    totals["files_renamed"] = json!(1);
    totals["binary_files"] = json!(1);
    totals["lines_added"] = json!(141);
    totals["lines_deleted"] = json!(45);
    serde_json::from_value(json!({
        "comparison": comparison_json(),
        "totals": totals,
        "commits": [
            { "oid": "a".repeat(40), "parent_oids": [], "subject": "first",
              "author_name": "Ada", "timestamp": 1, "provenance": { "source": "git" } },
            { "oid": "b".repeat(40), "parent_oids": ["a".repeat(40), "c".repeat(40)],
              "subject": "merge second", "author_name": "Bob", "timestamp": 2,
              "provenance": { "source": "git" } }
        ],
        "files": [
            { "new_path": "src/lib.rs", "status": "added", "lines_added": 10,
              "lines_deleted": 0, "binary": false,
              "classification": { "role": "implementation",
                                  "rule_id": "builtin.path.implementation.v1" },
              "provenance": { "source": "git" } },
            { "old_path": "tests/lib.rs", "new_path": "tests/lib.rs", "status": "modified",
              "lines_added": 1, "lines_deleted": 3, "binary": false,
              "classification": { "role": "test", "rule_id": "builtin.path.test.v1" },
              "provenance": { "source": "git" } },
            { "old_path": "src/old.rs", "new_path": "src/new.rs", "status": "renamed",
              "similarity": 98, "lines_added": 2, "lines_deleted": 1, "binary": false,
              "classification": { "role": "implementation",
                                  "rule_id": "builtin.path.implementation.v1" },
              "provenance": { "source": "git" } },
            { "old_path": "README.md", "new_path": "README.md", "status": "modified",
              "lines_added": 5, "lines_deleted": 1, "binary": false,
              "classification": { "role": "documentation",
                                  "rule_id": "builtin.path.documentation.v1" },
              "provenance": { "source": "git" } },
            { "old_path": "Cargo.toml", "new_path": "Cargo.toml", "status": "modified",
              "lines_added": 3, "lines_deleted": 0, "binary": false,
              "classification": { "role": "configuration",
                                  "rule_id": "builtin.path.configuration.v1" },
              "provenance": { "source": "git" } },
            { "old_path": "pnpm-lock.yaml", "new_path": "pnpm-lock.yaml", "status": "modified",
              "lines_added": 120, "lines_deleted": 40, "binary": false,
              "classification": { "role": "lockfile", "rule_id": "builtin.path.lockfile.v1" },
              "provenance": { "source": "git" } },
            { "new_path": "assets/logo.png", "status": "added", "binary": true,
              "classification": { "role": "unclassified",
                                  "rule_id": "builtin.path.unclassified.v1" },
              "provenance": { "source": "git" } }
        ],
        "coverage": coverage_json(7, 3, 4)
    }))
    .expect("inventory fixture")
}

pub(crate) fn exact_diff() -> ExactReviewDiffResponse {
    serde_json::from_value(json!({
        "comparison": comparison_json(),
        "diff": { "files": [] }
    }))
    .expect("exact diff fixture")
}

/// Structure that reached no file; unit R extends this with symbol changes.
pub(crate) fn structure() -> ReviewStructure {
    serde_json::from_value(json!({
        "comparison": comparison_json(),
        "files": [],
        "coverage": coverage_json(0, 0, 0),
        "language_coverage": [],
        "errors": []
    }))
    .expect("structure fixture")
}
