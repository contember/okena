//! The one review dataset every test in this crate builds on.
//!
//! `inventory()` + `structure()` describe the same comparison, and `model()` is
//! the ranking over both. Keep the builder names stable — other units' tests
//! read them.

use super::model::ReviewModel;
use super::ranking::{ModelInputs, StructureLoad, build_review_model};
use okena_core::review::{ReviewInventory, TruncationReason};
use okena_git::{DiffMode, ExactReviewDiffResponse};
use okena_review::{
    AnalysisError, AnalysisStage, CallChangeKind, CallDiffChange, CallPairingEvidence,
    CallPairingStrategy, ChangedHunk, ChangedLineRange, ComparisonSide, FileAnalysisStatus,
    ImmutableResolvedComparison, LanguageCoverage, OmittedFileGroup, OmittedFileReason,
    ReviewCoverage, ReviewNavigationTarget, ReviewStructure, ReviewTruncation, SignatureChange,
    StructuralHotspot, StructuralMetric, StructuredFile, SymbolChange, SymbolChangeKind,
    SymbolReference,
};
use okena_syntax::{
    CallFact, ControlContext, SourceRange, SymbolFact, SymbolKey, SymbolKind, SymbolVisibility,
    SyntaxLanguage, SyntaxProvenance,
};
use serde_json::{Value, json};
use std::num::NonZeroU32;

/// The only file the fixture analyses; everything else stays a git fact.
const ENGINE: &str = "src/engine.rs";
const DELETED: &str = "src/legacy.rs";
const MOVED_OLD: &str = "src/motion_old.rs";
const MOVED_NEW: &str = "src/motion_new.rs";
const UNSUPPORTED: &str = "src/app.js";
const FAILED: &str = "worker/handler.rs";

const RUN_OLD: &str = "pub fn run(&self, input: &str) -> Result<()>";
const RUN_NEW: &str = "pub fn run(&self, input: &str, retries: u32) -> Result<()>";
const CONFIGURE_OLD: &str = "pub fn configure(&self, options: Options)";
const CONFIGURE_NEW: &str = "pub fn configure(&self, options: Options, strict: bool)";
const DISPATCH: &str = "fn dispatch(&self, event: Event)";
const NORMALIZE: &str = "fn normalize(value: &str) -> String";
const RENDER: &str = "fn render(input: &str) -> String";
const STEPS: &str = "fn steps(count: u32) -> Vec<Step>";
const ORCHESTRATE: &str = "pub fn orchestrate(config: Config, hooks: &Hooks, retries: u32, \
     timeout: u64, tags: &[&str], tracing: bool, budget: Budget, sink: &mut Sink) -> Result<()>";

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

fn implementation() -> Value {
    json!({ "role": "implementation", "rule_id": "builtin.path.implementation.v1" })
}

fn git() -> Value {
    json!({ "source": "git" })
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

/// Thirteen files covering every ranking input: renames on both sides of the
/// residual boundary, a deleted and an added implementation file, a binary, a
/// lockfile, a config file, one implementation directory with test changes next
/// to it and one without, and an unsupported language. `src/lib.rs` and
/// `tests/lib.rs` come first, in that order.
pub(crate) fn inventory() -> ReviewInventory {
    let mut totals = totals_json();
    totals["commits"] = json!(2);
    totals["files"] = json!(13);
    totals["files_added"] = json!(2);
    totals["files_deleted"] = json!(1);
    totals["files_modified"] = json!(8);
    totals["files_renamed"] = json!(2);
    totals["binary_files"] = json!(1);
    totals["lines_added"] = json!(471);
    totals["lines_deleted"] = json!(295);
    serde_json::from_value(json!({
        "comparison": comparison_json(),
        "totals": totals,
        "commits": [
            { "oid": "a".repeat(40), "parent_oids": [], "subject": "first",
              "author_name": "Ada", "timestamp": 1, "provenance": git() },
            { "oid": "b".repeat(40), "parent_oids": ["a".repeat(40), "c".repeat(40)],
              "subject": "merge second", "author_name": "Bob", "timestamp": 2,
              "provenance": git() }
        ],
        "files": [
            { "new_path": "src/lib.rs", "status": "added", "lines_added": 10,
              "lines_deleted": 0, "binary": false,
              "classification": implementation(), "provenance": git() },
            { "old_path": "tests/lib.rs", "new_path": "tests/lib.rs", "status": "modified",
              "lines_added": 1, "lines_deleted": 3, "binary": false,
              "classification": { "role": "test", "rule_id": "builtin.path.test.v1" },
              "provenance": git() },
            { "old_path": "src/old.rs", "new_path": "src/new.rs", "status": "renamed",
              "similarity": 98, "lines_added": 2, "lines_deleted": 1, "binary": false,
              "classification": implementation(), "provenance": git() },
            { "old_path": "README.md", "new_path": "README.md", "status": "modified",
              "lines_added": 5, "lines_deleted": 1, "binary": false,
              "classification": { "role": "documentation",
                                  "rule_id": "builtin.path.documentation.v1" },
              "provenance": git() },
            { "old_path": "Cargo.toml", "new_path": "Cargo.toml", "status": "modified",
              "lines_added": 3, "lines_deleted": 0, "binary": false,
              "classification": { "role": "configuration",
                                  "rule_id": "builtin.path.configuration.v1" },
              "provenance": git() },
            { "old_path": "pnpm-lock.yaml", "new_path": "pnpm-lock.yaml", "status": "modified",
              "lines_added": 120, "lines_deleted": 40, "binary": false,
              "classification": { "role": "lockfile", "rule_id": "builtin.path.lockfile.v1" },
              "provenance": git() },
            { "new_path": "assets/logo.png", "status": "added", "binary": true,
              "classification": { "role": "unclassified",
                                  "rule_id": "builtin.path.unclassified.v1" },
              "provenance": git() },
            { "old_path": ENGINE, "new_path": ENGINE, "status": "modified",
              "lines_added": 60, "lines_deleted": 30, "binary": false,
              "classification": implementation(), "provenance": git() },
            { "old_path": "src/legacy.rs", "status": "deleted", "lines_added": 0,
              "lines_deleted": 120, "binary": false,
              "classification": implementation(), "provenance": git() },
            { "old_path": "src/motion_old.rs", "new_path": "src/motion_new.rs",
              "status": "renamed", "similarity": 91, "lines_added": 50, "lines_deleted": 36,
              "binary": false, "classification": implementation(), "provenance": git() },
            { "old_path": UNSUPPORTED, "new_path": UNSUPPORTED, "status": "modified",
              "lines_added": 12, "lines_deleted": 4, "binary": false,
              "classification": implementation(), "provenance": git() },
            { "old_path": FAILED, "new_path": FAILED, "status": "modified",
              "lines_added": 200, "lines_deleted": 60, "binary": false,
              "classification": implementation(), "provenance": git() },
            { "old_path": "worker/handler_test.rs", "new_path": "worker/handler_test.rs",
              "status": "modified", "lines_added": 8, "lines_deleted": 0, "binary": false,
              "classification": { "role": "test", "rule_id": "builtin.path.test.v1" },
              "provenance": git() }
        ],
        "coverage": coverage_json(13, 13, 0)
    }))
    .expect("inventory fixture")
}

/// Three files, under both small-comparison bounds — spec §12.
pub(crate) fn inventory_small() -> ReviewInventory {
    let mut totals = totals_json();
    totals["files"] = json!(3);
    totals["files_added"] = json!(1);
    totals["files_modified"] = json!(2);
    totals["lines_added"] = json!(16);
    totals["lines_deleted"] = json!(4);
    serde_json::from_value(json!({
        "comparison": comparison_json(),
        "totals": totals,
        "commits": [],
        "files": [
            { "new_path": "src/lib.rs", "status": "added", "lines_added": 10,
              "lines_deleted": 0, "binary": false,
              "classification": implementation(), "provenance": git() },
            { "old_path": "tests/lib.rs", "new_path": "tests/lib.rs", "status": "modified",
              "lines_added": 1, "lines_deleted": 3, "binary": false,
              "classification": { "role": "test", "rule_id": "builtin.path.test.v1" },
              "provenance": git() },
            { "old_path": "README.md", "new_path": "README.md", "status": "modified",
              "lines_added": 5, "lines_deleted": 1, "binary": false,
              "classification": { "role": "documentation",
                                  "rule_id": "builtin.path.documentation.v1" },
              "provenance": git() }
        ],
        "coverage": coverage_json(3, 3, 0)
    }))
    .expect("small inventory fixture")
}

/// Nothing structure analysis can parse — spec §12.
pub(crate) fn inventory_all_unsupported() -> ReviewInventory {
    let mut totals = totals_json();
    totals["files"] = json!(3);
    totals["files_modified"] = json!(3);
    totals["lines_added"] = json!(25);
    totals["lines_deleted"] = json!(6);
    serde_json::from_value(json!({
        "comparison": comparison_json(),
        "totals": totals,
        "commits": [],
        "files": [
            { "old_path": "src/app.js", "new_path": "src/app.js", "status": "modified",
              "lines_added": 12, "lines_deleted": 4, "binary": false,
              "classification": implementation(), "provenance": git() },
            { "old_path": "web/page.astro", "new_path": "web/page.astro", "status": "modified",
              "lines_added": 9, "lines_deleted": 2, "binary": false,
              "classification": implementation(), "provenance": git() },
            { "old_path": "src/util.js", "new_path": "src/util.js", "status": "modified",
              "lines_added": 4, "lines_deleted": 0, "binary": false,
              "classification": implementation(), "provenance": git() }
        ],
        "coverage": coverage_json(3, 0, 3)
    }))
    .expect("unsupported inventory fixture")
}

/// Only binary files, so nothing has a line count — spec §12.
pub(crate) fn inventory_binary_only() -> ReviewInventory {
    let mut totals = totals_json();
    totals["files"] = json!(2);
    totals["files_added"] = json!(2);
    totals["binary_files"] = json!(2);
    serde_json::from_value(json!({
        "comparison": comparison_json(),
        "totals": totals,
        "commits": [],
        "files": [
            { "new_path": "assets/logo.png", "status": "added", "binary": true,
              "classification": { "role": "unclassified",
                                  "rule_id": "builtin.path.unclassified.v1" },
              "provenance": git() },
            { "new_path": "assets/hero.jpg", "status": "added", "binary": true,
              "classification": { "role": "unclassified",
                                  "rule_id": "builtin.path.unclassified.v1" },
              "provenance": git() }
        ],
        "coverage": coverage_json(2, 0, 2)
    }))
    .expect("binary inventory fixture")
}

pub(crate) fn exact_diff() -> ExactReviewDiffResponse {
    serde_json::from_value(json!({
        "comparison": comparison_json(),
        "diff": { "files": [] }
    }))
    .expect("exact diff fixture")
}

// -- structure ---------------------------------------------------------------

fn comparison() -> ImmutableResolvedComparison {
    ImmutableResolvedComparison::try_from(empty_inventory().comparison)
        .expect("the fixture comparison is immutable")
}

fn at(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("fixture line numbers are one-based")
}

/// A line-shaped range; a hundred bytes per line keeps containment obvious.
fn span(start: u32, end: u32) -> SourceRange {
    SourceRange::new(
        u64::from(start) * 100,
        u64::from(end) * 100 + 99,
        at(start),
        at(end),
    )
    .expect("fixture source range")
}

fn rust() -> SyntaxProvenance {
    SyntaxProvenance::tree_sitter(SyntaxLanguage::Rust, "tree-sitter-rust")
        .expect("fixture provenance")
}

fn hunk(old: Option<(u32, u32)>, new: Option<(u32, u32)>) -> ChangedHunk {
    let range = |(start, end): (u32, u32)| {
        ChangedLineRange::new(at(start), at(end)).expect("fixture changed-line range")
    };
    ChangedHunk::new(old.map(range), new.map(range)).expect("fixture hunk")
}

fn nav_at(path: &str, side: ComparisonSide, line: u32) -> ReviewNavigationTarget {
    ReviewNavigationTarget {
        path: path.to_string(),
        side,
        line: at(line),
        byte_offset: None,
        symbol_context: None,
    }
}

fn nav(side: ComparisonSide, line: u32) -> ReviewNavigationTarget {
    nav_at(ENGINE, side, line)
}

fn key(path: &[&str], kind: SymbolKind, name: &str) -> SymbolKey {
    SymbolKey::new(
        path.iter().map(|part| (*part).to_string()).collect(),
        kind,
        name,
    )
    .expect("fixture symbol key")
}

struct FactSpec<'a> {
    key: &'a SymbolKey,
    visibility: SymbolVisibility,
    full: (u32, u32),
    signature_line: u32,
    body: (u32, u32),
    signature: &'a str,
    params: u32,
    depth: u32,
}

fn fact(spec: FactSpec<'_>) -> SymbolFact {
    SymbolFact::new(
        rust(),
        spec.key.clone(),
        spec.visibility,
        span(spec.full.0, spec.full.1),
        span(spec.signature_line, spec.signature_line),
        Some(span(spec.body.0, spec.body.1)),
        spec.signature,
        spec.params,
        spec.depth,
        0,
    )
    .expect("fixture symbol fact")
}

fn call(
    callee: &str,
    arguments: &str,
    line: u32,
    enclosing: &SymbolKey,
    context: Vec<ControlContext>,
) -> CallFact {
    CallFact::new(
        rust(),
        callee,
        arguments,
        span(line, line),
        span(line, line),
        Some(enclosing.clone()),
        context,
    )
    .expect("fixture call fact")
}

/// A `ChangedLines` hotspot on the side the change actually has.
fn changed_lines(
    symbol: &SymbolKey,
    range: (u32, u32),
    old: u32,
    new: u32,
    side: ComparisonSide,
    line: u32,
) -> StructuralHotspot {
    StructuralHotspot::new(
        SymbolReference::new(side, span(range.0, range.1), symbol.clone()),
        StructuralMetric::ChangedLines { old, new },
        rust(),
        nav(side, line),
    )
    .expect("fixture changed-lines hotspot")
}

/// A removed function is measured on the base side, where it still exists.
fn base_changed_lines(
    symbol: &SymbolKey,
    range: (u32, u32),
    old: u32,
    new: u32,
    line: u32,
) -> StructuralHotspot {
    changed_lines(symbol, range, old, new, ComparisonSide::Base, line)
}

fn hotspot(
    symbol: &SymbolKey,
    range: (u32, u32),
    metric: StructuralMetric,
    line: u32,
) -> StructuralHotspot {
    StructuralHotspot::new(
        SymbolReference::new(ComparisonSide::Head, span(range.0, range.1), symbol.clone()),
        metric,
        rust(),
        nav(ComparisonSide::Head, line),
    )
    .expect("fixture hotspot")
}

/// `src/engine.rs`: six changed symbols, one untouched hotspot, four call changes.
fn engine_file() -> StructuredFile {
    let run = key(&["Engine"], SymbolKind::Method, "run");
    let legacy_run = key(&["Engine"], SymbolKind::Method, "legacy_run");
    let configure = key(&["Engine"], SymbolKind::Method, "configure");
    let dispatch = key(&["Engine"], SymbolKind::Method, "dispatch");
    let normalize = key(&[], SymbolKind::Function, "normalize");
    let orchestrate = key(&[], SymbolKind::Function, "orchestrate");
    let helper = key(&["Engine"], SymbolKind::Method, "helper");

    let run_change = SymbolChange::new(
        SymbolChangeKind::Modified,
        Some(fact(FactSpec {
            key: &run,
            visibility: SymbolVisibility::Public,
            full: (10, 40),
            signature_line: 10,
            body: (11, 40),
            signature: RUN_OLD,
            params: 2,
            depth: 3,
        })),
        Some(fact(FactSpec {
            key: &run,
            visibility: SymbolVisibility::Public,
            full: (10, 45),
            signature_line: 10,
            body: (11, 45),
            signature: RUN_NEW,
            params: 3,
            depth: 3,
        })),
        Some(
            SignatureChange::new(RUN_OLD, RUN_NEW, span(10, 10), span(10, 10))
                .expect("fixture signature change"),
        ),
        true,
        vec![
            hunk(Some((10, 10)), Some((10, 10))),
            hunk(Some((20, 24)), Some((20, 26))),
        ],
        nav(ComparisonSide::Head, 10),
    )
    .expect("fixture run change");

    let legacy_change = SymbolChange::new(
        SymbolChangeKind::Removed,
        Some(fact(FactSpec {
            key: &legacy_run,
            visibility: SymbolVisibility::Public,
            full: (50, 70),
            signature_line: 50,
            body: (51, 70),
            signature: "pub fn legacy_run(&self) -> Result<()>",
            params: 0,
            depth: 1,
        })),
        None,
        None,
        false,
        vec![hunk(Some((50, 70)), None)],
        nav(ComparisonSide::Base, 50),
    )
    .expect("fixture legacy change");

    let configure_change = SymbolChange::new(
        SymbolChangeKind::Modified,
        Some(fact(FactSpec {
            key: &configure,
            visibility: SymbolVisibility::Public,
            full: (75, 95),
            signature_line: 75,
            body: (76, 95),
            signature: CONFIGURE_OLD,
            params: 1,
            depth: 1,
        })),
        Some(fact(FactSpec {
            key: &configure,
            visibility: SymbolVisibility::Public,
            full: (75, 95),
            signature_line: 75,
            body: (76, 95),
            signature: CONFIGURE_NEW,
            params: 2,
            depth: 1,
        })),
        Some(
            SignatureChange::new(CONFIGURE_OLD, CONFIGURE_NEW, span(75, 75), span(75, 75))
                .expect("fixture signature change"),
        ),
        false,
        vec![hunk(Some((75, 75)), Some((75, 75)))],
        nav(ComparisonSide::Head, 75),
    )
    .expect("fixture configure change");

    let dispatch_change = SymbolChange::new(
        SymbolChangeKind::Modified,
        Some(fact(FactSpec {
            key: &dispatch,
            visibility: SymbolVisibility::Private,
            full: (200, 240),
            signature_line: 200,
            body: (201, 240),
            signature: DISPATCH,
            params: 1,
            depth: 2,
        })),
        Some(fact(FactSpec {
            key: &dispatch,
            visibility: SymbolVisibility::Private,
            full: (200, 244),
            signature_line: 200,
            body: (201, 244),
            signature: DISPATCH,
            params: 1,
            depth: 2,
        })),
        None,
        true,
        vec![hunk(Some((210, 212)), Some((210, 214)))],
        nav(ComparisonSide::Head, 200),
    )
    .expect("fixture dispatch change");

    let normalize_change = SymbolChange::new(
        SymbolChangeKind::Modified,
        Some(fact(FactSpec {
            key: &normalize,
            visibility: SymbolVisibility::Private,
            full: (400, 430),
            signature_line: 400,
            body: (401, 430),
            signature: NORMALIZE,
            params: 1,
            depth: 2,
        })),
        Some(fact(FactSpec {
            key: &normalize,
            visibility: SymbolVisibility::Private,
            full: (400, 436),
            signature_line: 400,
            body: (401, 436),
            signature: NORMALIZE,
            params: 1,
            depth: 2,
        })),
        None,
        true,
        vec![hunk(Some((410, 415)), Some((410, 421)))],
        nav(ComparisonSide::Head, 400),
    )
    .expect("fixture normalize change");

    let orchestrate_change = SymbolChange::new(
        SymbolChangeKind::Added,
        None,
        Some(fact(FactSpec {
            key: &orchestrate,
            visibility: SymbolVisibility::Public,
            full: (600, 839),
            signature_line: 600,
            body: (601, 839),
            signature: ORCHESTRATE,
            params: 8,
            depth: 6,
        })),
        None,
        false,
        vec![hunk(None, Some((600, 839)))],
        nav(ComparisonSide::Head, 600),
    )
    .expect("fixture orchestrate change");

    let pairing = CallPairingEvidence::new(
        CallPairingStrategy::UniqueOccurrenceWithinEnclosingRange,
        span(30, 30),
        span(32, 32),
        span(10, 40),
        span(10, 45),
        1,
        1,
    )
    .expect("fixture call pairing");

    let call_diff = vec![
        CallDiffChange::new(
            CallChangeKind::Removed,
            Some(call(
                "validate",
                "(input)",
                22,
                &run,
                vec![ControlContext::ErrorBranch],
            )),
            None,
            false,
            false,
            None,
            nav(ComparisonSide::Base, 22),
        )
        .expect("fixture removed call"),
        CallDiffChange::new(
            CallChangeKind::Modified,
            Some(call(
                "retry",
                "(3)",
                30,
                &run,
                vec![ControlContext::Condition],
            )),
            Some(call(
                "retry",
                "(retries)",
                32,
                &run,
                vec![ControlContext::Condition],
            )),
            true,
            false,
            Some(pairing),
            nav(ComparisonSide::Head, 32),
        )
        .expect("fixture modified call"),
        CallDiffChange::new(
            CallChangeKind::Removed,
            Some(call(
                "log_error",
                "(event)",
                215,
                &dispatch,
                vec![ControlContext::MatchArm],
            )),
            None,
            false,
            false,
            None,
            nav(ComparisonSide::Base, 215),
        )
        .expect("fixture dispatch call"),
        CallDiffChange::new(
            CallChangeKind::Added,
            None,
            Some(call("emit", "(value)", 412, &normalize, Vec::new())),
            false,
            false,
            None,
            nav(ComparisonSide::Head, 412),
        )
        .expect("fixture added call"),
        // A brand-new function is full of new calls; that is not behaviour.
        CallDiffChange::new(
            CallChangeKind::Added,
            None,
            Some(call(
                "spawn",
                "(config)",
                640,
                &orchestrate,
                vec![ControlContext::ErrorBranch],
            )),
            false,
            false,
            None,
            nav(ComparisonSide::Head, 640),
        )
        .expect("fixture new-function call"),
    ];

    // The producer emits `ChangedLines` for *every* changed function and the
    // head-side metric triple for every head-side function, changed or not.
    let hotspots = vec![
        changed_lines(&run, (10, 45), 6, 8, ComparisonSide::Head, 10),
        base_changed_lines(&legacy_run, (50, 70), 21, 0, 50),
        changed_lines(&configure, (75, 95), 1, 1, ComparisonSide::Head, 75),
        changed_lines(&dispatch, (200, 244), 3, 5, ComparisonSide::Head, 200),
        changed_lines(&normalize, (400, 436), 6, 12, ComparisonSide::Head, 400),
        changed_lines(&orchestrate, (600, 839), 0, 240, ComparisonSide::Head, 600),
        hotspot(
            &run,
            (10, 45),
            StructuralMetric::FunctionLineCount { lines: 36 },
            10,
        ),
        hotspot(
            &run,
            (10, 45),
            StructuralMetric::ParameterCount { parameters: 3 },
            10,
        ),
        hotspot(
            &run,
            (10, 45),
            StructuralMetric::SyntacticNestingDepth { depth: 3 },
            10,
        ),
        hotspot(
            &orchestrate,
            (600, 839),
            StructuralMetric::FunctionLineCount { lines: 240 },
            600,
        ),
        hotspot(
            &orchestrate,
            (600, 839),
            StructuralMetric::SyntacticNestingDepth { depth: 6 },
            600,
        ),
        hotspot(
            &orchestrate,
            (600, 839),
            StructuralMetric::ParameterCount { parameters: 8 },
            600,
        ),
        // Hotspots on a symbol that did not change; the ranking must skip them.
        hotspot(
            &helper,
            (900, 989),
            StructuralMetric::FunctionLineCount { lines: 90 },
            900,
        ),
        hotspot(
            &helper,
            (900, 989),
            StructuralMetric::SyntacticNestingDepth { depth: 7 },
            900,
        ),
    ];

    StructuredFile::new(
        Some(ENGINE.to_string()),
        Some(ENGINE.to_string()),
        Some(SyntaxLanguage::Rust),
        Some(rust()),
        Some(rust()),
        FileAnalysisStatus::Parsed,
        Vec::new(),
        Vec::new(),
        vec![
            run_change,
            legacy_change,
            configure_change,
            dispatch_change,
            normalize_change,
            orchestrate_change,
        ],
        hotspots,
        call_diff,
        vec![
            hunk(Some((10, 10)), Some((10, 10))),
            hunk(Some((20, 24)), Some((20, 26))),
            hunk(Some((50, 70)), None),
            hunk(Some((75, 75)), Some((75, 75))),
            hunk(Some((210, 212)), Some((210, 214))),
            hunk(Some((410, 415)), Some((410, 421))),
            hunk(None, Some((600, 839))),
        ],
        Vec::new(),
        None,
    )
    .expect("fixture structured file")
}

fn unsupported_file() -> StructuredFile {
    StructuredFile::new(
        Some(UNSUPPORTED.to_string()),
        Some(UNSUPPORTED.to_string()),
        None,
        None,
        None,
        FileAnalysisStatus::Unsupported,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
    )
    .expect("fixture unsupported file")
}

fn failed_file() -> StructuredFile {
    StructuredFile::new(
        Some(FAILED.to_string()),
        Some(FAILED.to_string()),
        None,
        None,
        None,
        FileAnalysisStatus::Failed,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![
            AnalysisError::new(
                Some(FAILED.to_string()),
                AnalysisStage::Parsing,
                "unexpected token",
            )
            .expect("fixture analysis error"),
        ],
        None,
    )
    .expect("fixture failed file")
}

fn rust_coverage(files: u64) -> Vec<LanguageCoverage> {
    vec![LanguageCoverage::new(
        SyntaxLanguage::Rust,
        ReviewCoverage::new(files, files, 0, 0, 0, 0, None).expect("fixture language coverage"),
    )]
}

/// `src/legacy.rs`: analysed even though it is gone, so its removed symbol and
/// its `deleted implementation file` row both appear.
fn deleted_file() -> StructuredFile {
    let render = key(&[], SymbolKind::Function, "render");
    let change = SymbolChange::new(
        SymbolChangeKind::Removed,
        Some(fact(FactSpec {
            key: &render,
            visibility: SymbolVisibility::Private,
            full: (5, 60),
            signature_line: 5,
            body: (6, 60),
            signature: RENDER,
            params: 1,
            depth: 2,
        })),
        None,
        None,
        false,
        vec![hunk(Some((5, 60)), None)],
        nav_at(DELETED, ComparisonSide::Base, 5),
    )
    .expect("fixture render change");
    StructuredFile::new(
        Some(DELETED.to_string()),
        None,
        Some(SyntaxLanguage::Rust),
        Some(rust()),
        None,
        FileAnalysisStatus::Parsed,
        Vec::new(),
        Vec::new(),
        vec![change],
        vec![
            StructuralHotspot::new(
                SymbolReference::new(ComparisonSide::Base, span(5, 60), render.clone()),
                StructuralMetric::ChangedLines { old: 56, new: 0 },
                rust(),
                nav_at(DELETED, ComparisonSide::Base, 5),
            )
            .expect("fixture deleted hotspot"),
        ],
        Vec::new(),
        vec![hunk(Some((5, 60)), None)],
        Vec::new(),
        None,
    )
    .expect("fixture deleted file")
}

/// `src/motion_old.rs` → `src/motion_new.rs`: a rename with 86 residual lines,
/// analysed, so its `moved` row must survive next to its symbol row.
fn moved_file() -> StructuredFile {
    let steps = key(&[], SymbolKind::Function, "steps");
    let head = |line: u32| nav_at(MOVED_NEW, ComparisonSide::Head, line);
    let change = SymbolChange::new(
        SymbolChangeKind::Modified,
        Some(fact(FactSpec {
            key: &steps,
            visibility: SymbolVisibility::Private,
            full: (10, 60),
            signature_line: 10,
            body: (11, 60),
            signature: STEPS,
            params: 1,
            depth: 2,
        })),
        Some(fact(FactSpec {
            key: &steps,
            visibility: SymbolVisibility::Private,
            full: (10, 74),
            signature_line: 10,
            body: (11, 74),
            signature: STEPS,
            params: 1,
            depth: 2,
        })),
        None,
        true,
        vec![hunk(Some((20, 55)), Some((20, 69)))],
        head(10),
    )
    .expect("fixture steps change");
    StructuredFile::new(
        Some(MOVED_OLD.to_string()),
        Some(MOVED_NEW.to_string()),
        Some(SyntaxLanguage::Rust),
        Some(rust()),
        Some(rust()),
        FileAnalysisStatus::Parsed,
        Vec::new(),
        Vec::new(),
        vec![change],
        vec![
            StructuralHotspot::new(
                SymbolReference::new(ComparisonSide::Head, span(10, 74), steps.clone()),
                StructuralMetric::ChangedLines { old: 36, new: 50 },
                rust(),
                head(10),
            )
            .expect("fixture moved hotspot"),
            StructuralHotspot::new(
                SymbolReference::new(ComparisonSide::Head, span(10, 74), steps.clone()),
                StructuralMetric::FunctionLineCount { lines: 65 },
                rust(),
                head(10),
            )
            .expect("fixture moved size hotspot"),
        ],
        Vec::new(),
        vec![hunk(Some((20, 55)), Some((20, 69)))],
        Vec::new(),
        None,
    )
    .expect("fixture moved file")
}

/// Three parsed files, one unsupported, one failed, and a file-limit omission.
pub(crate) fn structure() -> ReviewStructure {
    ReviewStructure::new_with_omissions(
        comparison(),
        vec![
            engine_file(),
            deleted_file(),
            moved_file(),
            unsupported_file(),
            failed_file(),
        ],
        vec![
            OmittedFileGroup::new(
                2,
                None,
                OmittedFileReason::FileLimit,
                Some(ReviewTruncation {
                    reason: TruncationReason::ItemLimit,
                    limit: Some(200),
                    observed: Some(205),
                    detail: None,
                }),
            )
            .expect("fixture omission group"),
        ],
        ReviewCoverage::new(7, 3, 2, 0, 1, 1, None).expect("fixture structure coverage"),
        rust_coverage(3),
        Vec::new(),
    )
    .expect("structure fixture")
}

/// Complete except for one file the parser could not read.
pub(crate) fn structure_with_failure() -> ReviewStructure {
    ReviewStructure::new(
        comparison(),
        vec![engine_file(), failed_file()],
        ReviewCoverage::new(2, 1, 0, 0, 0, 1, None).expect("fixture failure coverage"),
        rust_coverage(1),
        Vec::new(),
    )
    .expect("failure structure fixture")
}

/// Structure that reached no file at all.
pub(crate) fn structure_empty() -> ReviewStructure {
    ReviewStructure::new(
        comparison(),
        Vec::new(),
        ReviewCoverage::new(0, 0, 0, 0, 0, 0, None).expect("fixture empty coverage"),
        Vec::new(),
        Vec::new(),
    )
    .expect("empty structure fixture")
}

/// The ranking over [`inventory`] and [`structure`] — the shared golden model.
pub(crate) fn model() -> ReviewModel {
    let inventory = inventory();
    let structure = structure();
    let mode = DiffMode::BranchCompare {
        base: "main".into(),
        head: "feature".into(),
    };
    build_review_model(ModelInputs {
        inventory: Some(&inventory),
        inventory_error: None,
        structure: Some(&structure),
        structure_state: StructureLoad::Ready,
        diff_mode: &mode,
    })
}
