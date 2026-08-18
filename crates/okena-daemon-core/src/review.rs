//! Bounded, off-reactor execution for deterministic review actions.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use okena_core::api::{ActionRequest, CommandResult};
use okena_core::review::{
    ExactReviewSourceResponse, ImmutableResolvedComparison, ReviewCoverage, ReviewDiffRequest,
    ReviewFileFact, ReviewFileStatus, ReviewInventory, ReviewSourceRequest, ReviewTruncation,
    TruncationReason,
};
use okena_core::types::DiffMode;
use okena_git::{
    DiffLineType, ExactReviewDiffResponse, FileDiff, GitError, ReviewGitControl,
    ReviewSourceBudget, ReviewSourceBudgetKind, get_exact_review_diff_response_with_control,
    get_exact_review_source_response_with_control, get_exact_review_source_with_control,
    get_review_inventory_with_control, resolve_review_comparison_with_control,
};
use okena_review::call_diff::ComparisonStopReason;
use okena_review::classification::classify_file_fact;
use okena_review::structure::compare_structured_file_controlled;
use okena_review::{
    AnalysisError, AnalysisStage, ChangedHunk, ChangedLineRange, FileAnalysisStatus,
    LanguageCoverage, OmittedFileGroup, OmittedFileReason, ReviewStructure, StructuredFile,
};
use okena_syntax::rust::RustAdapter;
use okena_syntax::typescript::TypeScriptAdapter;
use okena_syntax::{
    AnalysisBudget, AnalysisControl, AnalysisInput, SyntaxAdapter, SyntaxLanguage,
    SyntaxTruncation, SyntaxTruncationReason,
};
use okena_workspace::state::Workspace;
use tokio::sync::{Semaphore, oneshot};

pub(crate) const MAX_CONCURRENT_REVIEWS: usize = 2;

const MAX_FILES: usize = 200;
const MAX_SOURCE_SIDE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SOURCE_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_STANDALONE_SOURCE_TOTAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CAPTURE_SIDE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CAPTURE_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 24 * 1024 * 1024;
const MAX_SYMBOLS: u32 = 10_000;
const MAX_CALLS: u32 = 20_000;
const MAX_DIAGNOSTICS: u32 = 64;
const ANALYSIS_TIME_MICROS: u64 = 30 * 1_000_000;
const MAX_AGGREGATE_FACTS: u64 = 100_000;
const MAX_CONSTRUCTED_RESPONSE_BYTES: usize = 20 * 1024 * 1024;
const RESPONSE_CHECKPOINT_BYTES: usize = 16 * 1024;

enum ReviewRequest {
    Inventory(DiffMode),
    Diff(ReviewDiffRequest),
    Source(Box<ReviewSourceRequest>),
    Structure(ReviewDiffRequest),
}

type FilePathPair = (Option<String>, Option<String>);
type IndexedFileDiffs = HashMap<FilePathPair, FileDiff>;

pub(crate) struct PreparedReviewAction {
    project_path: PathBuf,
    request: ReviewRequest,
}

#[derive(Default)]
struct ReviewWorkerControl {
    cancelled: AtomicBool,
    analysis: Mutex<Option<AnalysisControl>>,
}

impl ReviewWorkerControl {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let analysis = self
            .analysis
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(analysis) = analysis {
            analysis.cancel();
        }
    }

    fn checkpoint(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Acquire) {
            Err("review request cancelled".to_string())
        } else {
            Ok(())
        }
    }

    fn start_analysis(&self) -> AnalysisControl {
        let analysis =
            AnalysisControl::new(NonZeroU64::new(ANALYSIS_TIME_MICROS).unwrap_or(NonZeroU64::MIN));
        let mut published = self
            .analysis
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.cancelled.load(Ordering::Acquire) {
            analysis.cancel();
        }
        *published = Some(analysis.clone());
        analysis
    }

    fn analysis(&self) -> Option<AnalysisControl> {
        self.analysis
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

pub(crate) fn is_review_action(action: &ActionRequest) -> bool {
    matches!(
        action,
        ActionRequest::ReviewInventory { .. }
            | ActionRequest::ReviewDiff { .. }
            | ActionRequest::ReviewSource { .. }
            | ActionRequest::ReviewStructure { .. }
    )
}

/// Copy all workspace-owned state before the caller releases the workspace lock.
pub(crate) fn prepare_review_action(
    workspace: &Workspace,
    action: ActionRequest,
) -> Result<PreparedReviewAction, String> {
    let (project_id, request) = match action {
        ActionRequest::ReviewInventory { project_id, mode } => {
            (project_id, ReviewRequest::Inventory(mode))
        }
        ActionRequest::ReviewDiff {
            project_id,
            request,
        } => (project_id, ReviewRequest::Diff(request)),
        ActionRequest::ReviewSource {
            project_id,
            request,
        } => (project_id, ReviewRequest::Source(request)),
        ActionRequest::ReviewStructure {
            project_id,
            request,
        } => (project_id, ReviewRequest::Structure(request)),
        _ => return Err("action is not a review request".to_string()),
    };
    let project = workspace
        .project(&project_id)
        .ok_or_else(|| format!("project not found: {project_id}"))?;
    if project.is_remote {
        return Err(format!(
            "review project is not local to this daemon: {project_id}"
        ));
    }
    Ok(PreparedReviewAction {
        project_path: PathBuf::from(&project.path),
        request,
    })
}

pub(crate) fn spawn_review_action(
    action: PreparedReviewAction,
    reply: Option<oneshot::Sender<CommandResult>>,
    runtime: &tokio::runtime::Handle,
    permits: Arc<Semaphore>,
) {
    spawn_review_action_with(action, reply, runtime, permits, execute_review_action);
}

fn spawn_review_action_with<Run>(
    action: PreparedReviewAction,
    reply: Option<oneshot::Sender<CommandResult>>,
    runtime: &tokio::runtime::Handle,
    permits: Arc<Semaphore>,
    run: Run,
) where
    Run: FnOnce(PreparedReviewAction, &ReviewGitControl, &ReviewWorkerControl) -> CommandResult
        + Send
        + 'static,
{
    if reply.as_ref().is_some_and(oneshot::Sender::is_closed) {
        return;
    }
    let permit = match permits.try_acquire_owned() {
        Ok(permit) => permit,
        Err(tokio::sync::TryAcquireError::NoPermits) => {
            if let Some(reply) = reply {
                let _ = reply.send(CommandResult::Err(
                    "review executor busy: at most 2 review requests may run concurrently"
                        .to_string(),
                ));
            }
            return;
        }
        Err(tokio::sync::TryAcquireError::Closed) => {
            if let Some(reply) = reply {
                let _ = reply.send(CommandResult::Err(
                    "review executor unavailable".to_string(),
                ));
            }
            return;
        }
    };
    let worker_runtime = runtime.clone();
    let _task = runtime.spawn(async move {
        let _permit = permit;
        if reply.as_ref().is_some_and(oneshot::Sender::is_closed) {
            return;
        }

        let git_control = ReviewGitControl::new(Default::default());
        let worker_control = Arc::new(ReviewWorkerControl::default());
        let worker_git_control = git_control.clone();
        let blocking_control = worker_control.clone();
        let mut worker = worker_runtime
            .spawn_blocking(move || run(action, &worker_git_control, &blocking_control));

        match reply {
            Some(mut reply) => {
                tokio::select! {
                    result = &mut worker => {
                        let result = result.unwrap_or_else(|error| {
                            CommandResult::Err(format!("review worker failed: {error}"))
                        });
                        let _ = reply.send(result);
                    }
                    _ = reply.closed() => {
                        git_control.cancel();
                        worker_control.cancel();
                        let _ = worker.await;
                    }
                }
            }
            None => {
                if let Err(error) = worker.await {
                    log::warn!("detached review worker failed: {error}");
                }
            }
        }
    });
}

fn execute_review_action(
    action: PreparedReviewAction,
    git_control: &ReviewGitControl,
    control: &ReviewWorkerControl,
) -> CommandResult {
    let result = match action.request {
        ReviewRequest::Inventory(mode) => {
            build_inventory(&action.project_path, mode, git_control, control)
                .and_then(|response| serialize_inventory_response(response, control))
        }
        ReviewRequest::Diff(request) => {
            get_exact_review_diff_response_with_control(&action.project_path, &request, git_control)
                .map_err(|error| error.to_string())
                .and_then(|response| serialize_diff_response(response, control))
        }
        ReviewRequest::Source(request) => {
            build_source(&action.project_path, &request, git_control, control)
                .and_then(|response| serialize_source_response(response, control))
        }
        ReviewRequest::Structure(request) => {
            build_structure(&action.project_path, request, git_control, control)
                .and_then(|response| serialize_structure_response(response, control))
        }
    };
    match result {
        Ok(value) => CommandResult::Ok(Some(value)),
        Err(error) => CommandResult::Err(error),
    }
}

fn build_source(
    project_path: &Path,
    request: &ReviewSourceRequest,
    git_control: &ReviewGitControl,
    control: &ReviewWorkerControl,
) -> Result<ExactReviewSourceResponse, String> {
    control.checkpoint()?;
    let budget = ReviewSourceBudget::new(MAX_SOURCE_SIDE_BYTES, MAX_STANDALONE_SOURCE_TOTAL_BYTES)
        .map_err(|error| error.to_string())?;
    let response =
        get_exact_review_source_response_with_control(project_path, request, budget, git_control)
            .map_err(|error| error.to_string())?;
    control.checkpoint()?;
    Ok(response)
}

fn build_inventory(
    project_path: &Path,
    mode: DiffMode,
    git_control: &ReviewGitControl,
    control: &ReviewWorkerControl,
) -> Result<ReviewInventory, String> {
    if matches!(mode, DiffMode::WorkingTree | DiffMode::Staged) {
        return Err("review inventory V1 requires an immutable commit or branch comparison".into());
    }
    let resolved = resolve_review_comparison_with_control(project_path, mode, git_control)
        .map_err(|error| error.to_string())?;
    let immutable = ImmutableResolvedComparison::try_from(resolved)
        .map_err(|error| format!("resolved review comparison is not immutable: {error}"))?;
    let mut inventory = get_review_inventory_with_control(project_path, &immutable, git_control)
        .map_err(|error| error.to_string())?;
    control.checkpoint()?;
    classify_inventory(&mut inventory, control)?;
    Ok(inventory)
}

fn classify_inventory(
    inventory: &mut ReviewInventory,
    control: &ReviewWorkerControl,
) -> Result<(), String> {
    for file in &mut inventory.files {
        control.checkpoint()?;
        file.classification = classify_file_fact(file).map_err(|error| {
            format!(
                "failed to classify review path {}: {error}",
                selected_path(file).unwrap_or("<missing>")
            )
        })?;
    }
    Ok(())
}

fn build_structure(
    project_path: &Path,
    request: ReviewDiffRequest,
    git_control: &ReviewGitControl,
    control: &ReviewWorkerControl,
) -> Result<ReviewStructure, String> {
    let exact_diff =
        get_exact_review_diff_response_with_control(project_path, &request, git_control)
            .map_err(|error| error.to_string())?;
    control.checkpoint()?;
    let (comparison, diff) = exact_diff.into_parts();
    let mut inventory = get_review_inventory_with_control(project_path, &comparison, git_control)
        .map_err(|error| error.to_string())?;
    control.checkpoint()?;
    classify_inventory(&mut inventory, control)?;
    let mut diffs = index_file_diffs(diff.files, control)?;
    control.checkpoint()?;
    let analysis_control = control.start_analysis();
    control.checkpoint()?;

    let mut files = Vec::with_capacity(inventory.files.len().min(MAX_FILES));
    let mut omissions = Vec::<OmissionAccumulator>::new();
    let mut source_bytes = 0_u64;
    let mut capture_bytes = 0_u64;
    let mut aggregate_facts = 0_u64;
    let mut response_bytes = 1024_usize;
    let mut analyzable_started = 0_usize;
    let mut halted: Option<(OmittedFileReason, ReviewTruncation)> = None;
    for fact in &inventory.files {
        control.checkpoint()?;
        let key = (fact.old_path.clone(), fact.new_path.clone());
        let Some(file_diff) = diffs.remove(&key) else {
            if request.ignore_whitespace {
                add_omission(
                    &mut omissions,
                    detect_language(fact),
                    OmittedFileReason::WhitespaceIgnored,
                    None,
                );
                continue;
            }
            return Err(format!(
                "exact diff omitted inventory path {}",
                display_paths(fact)
            ));
        };
        if let Some((language, reason)) = deterministic_omission(fact, file_diff.hunks.is_empty()) {
            add_omission(&mut omissions, language, reason, None);
            continue;
        }
        let language = detect_language(fact);
        if let Some((reason, truncation)) = halted_omission(&halted) {
            add_omission(&mut omissions, language, reason, Some(truncation));
            continue;
        }
        if aggregate_facts >= MAX_AGGREGATE_FACTS {
            let truncation = measured_truncation(
                TruncationReason::CaptureLimit,
                MAX_AGGREGATE_FACTS,
                aggregate_facts
                    .checked_add(1)
                    .ok_or_else(|| "aggregate fact observation overflowed".to_string())?,
                "aggregate structured facts",
            );
            add_omission(
                &mut omissions,
                language,
                OmittedFileReason::FactLimit,
                Some(truncation.clone()),
            );
            halted = Some((OmittedFileReason::FactLimit, truncation));
            continue;
        }
        if response_bytes >= MAX_CONSTRUCTED_RESPONSE_BYTES {
            let limit = u64::try_from(MAX_CONSTRUCTED_RESPONSE_BYTES)
                .map_err(|_| "response byte limit does not fit u64".to_string())?;
            let truncation = measured_truncation(
                TruncationReason::ResponseLimit,
                limit,
                limit
                    .checked_add(1)
                    .ok_or_else(|| "response byte observation overflowed".to_string())?,
                "constructed structured-review response",
            );
            add_omission(
                &mut omissions,
                language,
                OmittedFileReason::ResponseLimit,
                Some(truncation.clone()),
            );
            halted = Some((OmittedFileReason::ResponseLimit, truncation));
            continue;
        }
        if !claim_analyzable_slot(&mut analyzable_started) {
            let truncation = measured_truncation(
                TruncationReason::ItemLimit,
                MAX_FILES as u64,
                MAX_FILES as u64 + 1,
                "analyzable structured-review files",
            );
            add_omission(
                &mut omissions,
                language,
                OmittedFileReason::FileLimit,
                Some(truncation.clone()),
            );
            halted = Some((OmittedFileReason::FileLimit, truncation));
            continue;
        }
        let hunks = changed_hunks(&file_diff, control)?;
        match structure_file(
            &mut StructureFileContext {
                project_path,
                comparison: &comparison,
                source_bytes: &mut source_bytes,
                capture_bytes: &mut capture_bytes,
                git_control,
                analysis_control: &analysis_control,
                worker_control: control,
            },
            fact,
            hunks,
        )? {
            FileBuildOutcome::Omitted(reason, truncation) => {
                add_omission(&mut omissions, language, reason, Some(truncation.clone()));
                if matches!(
                    reason,
                    OmittedFileReason::AggregateByteLimit
                        | OmittedFileReason::TimeLimit
                        | OmittedFileReason::Cancelled
                ) {
                    halted = Some((reason, truncation));
                }
            }
            FileBuildOutcome::Halted(reason, truncation) => {
                add_omission(&mut omissions, language, reason, Some(truncation.clone()));
                halted = Some((reason, truncation));
            }
            FileBuildOutcome::File(file) => {
                let file = *file;
                control.checkpoint()?;
                if let Some((reason, truncation)) = analysis_omission_now(&analysis_control)? {
                    add_omission(&mut omissions, language, reason, Some(truncation.clone()));
                    halted = Some((reason, truncation));
                    continue;
                }
                let fact_count = structured_fact_count(&file, control)?;
                if let Some((reason, truncation)) = analysis_omission_now(&analysis_control)? {
                    add_omission(&mut omissions, language, reason, Some(truncation.clone()));
                    halted = Some((reason, truncation));
                    continue;
                }
                let observed = aggregate_facts
                    .checked_add(fact_count)
                    .ok_or_else(|| "aggregate structured fact count overflowed".to_string())?;
                if observed > MAX_AGGREGATE_FACTS {
                    let truncation = measured_truncation(
                        TruncationReason::CaptureLimit,
                        MAX_AGGREGATE_FACTS,
                        observed,
                        "aggregate structured facts",
                    );
                    add_omission(
                        &mut omissions,
                        language,
                        OmittedFileReason::FactLimit,
                        Some(truncation.clone()),
                    );
                    halted = Some((OmittedFileReason::FactLimit, truncation));
                    continue;
                }
                let remaining = MAX_CONSTRUCTED_RESPONSE_BYTES
                    .checked_sub(response_bytes)
                    .ok_or_else(|| {
                        "constructed response byte count exceeded its limit".to_string()
                    })?;
                let file_bytes =
                    match serialized_file_size(&file, remaining, control, &analysis_control)? {
                        MeasuredFileSize::Bytes(bytes) => bytes,
                        MeasuredFileSize::Exceeded => {
                            let truncation = measured_truncation(
                                TruncationReason::ResponseLimit,
                                MAX_CONSTRUCTED_RESPONSE_BYTES as u64,
                                MAX_CONSTRUCTED_RESPONSE_BYTES as u64 + 1,
                                "constructed structured-review response",
                            );
                            add_omission(
                                &mut omissions,
                                language,
                                OmittedFileReason::ResponseLimit,
                                Some(truncation.clone()),
                            );
                            halted = Some((OmittedFileReason::ResponseLimit, truncation));
                            continue;
                        }
                        MeasuredFileSize::AnalysisStopped(reason, truncation) => {
                            add_omission(
                                &mut omissions,
                                language,
                                reason,
                                Some(truncation.clone()),
                            );
                            halted = Some((reason, truncation));
                            continue;
                        }
                    };
                if let Some((reason, truncation)) = analysis_omission_now(&analysis_control)? {
                    add_omission(&mut omissions, language, reason, Some(truncation.clone()));
                    halted = Some((reason, truncation));
                    continue;
                }
                aggregate_facts = observed;
                response_bytes = response_bytes
                    .checked_add(file_bytes)
                    .ok_or_else(|| "constructed response byte count overflowed".to_string())?;
                files.push(file);
            }
        }
    }
    if !diffs.is_empty() {
        return Err(format!(
            "exact diff contained {} path(s) absent from inventory",
            diffs.len()
        ));
    }

    control.checkpoint()?;
    ensure_analysis_active(&analysis_control, "before final review coverage")?;
    let omissions = finish_omissions(omissions)?;
    let coverage = coverage_for(&files, &omissions, None)?;
    let language_coverage = language_coverage_for(&files, &omissions)?;
    control.checkpoint()?;
    ensure_analysis_active(&analysis_control, "before final review construction")?;
    let response = ReviewStructure::new_with_omissions(
        comparison,
        files,
        omissions,
        coverage,
        language_coverage,
        Vec::new(),
    )
    .map_err(|error| error.to_string())?;
    control.checkpoint()?;
    ensure_analysis_active(&analysis_control, "before accepting final review structure")?;
    Ok(response)
}

enum FileBuildOutcome {
    File(Box<StructuredFile>),
    Omitted(OmittedFileReason, ReviewTruncation),
    Halted(OmittedFileReason, ReviewTruncation),
}

fn halted_omission(
    halted: &Option<(OmittedFileReason, ReviewTruncation)>,
) -> Option<(OmittedFileReason, ReviewTruncation)> {
    halted.clone()
}

enum AggregateSourceCapacity {
    Remaining(u64),
    Exhausted(ReviewTruncation),
}

fn aggregate_source_capacity(consumed: u64) -> Result<AggregateSourceCapacity, String> {
    if consumed < MAX_SOURCE_TOTAL_BYTES {
        return Ok(AggregateSourceCapacity::Remaining(
            MAX_SOURCE_TOTAL_BYTES - consumed,
        ));
    }
    let first_rejected_byte = MAX_SOURCE_TOTAL_BYTES
        .checked_add(1)
        .ok_or_else(|| "aggregate source limit observation overflowed".to_string())?;
    Ok(AggregateSourceCapacity::Exhausted(measured_truncation(
        TruncationReason::ByteLimit,
        MAX_SOURCE_TOTAL_BYTES,
        consumed.max(first_rejected_byte),
        "aggregate structured-review source",
    )))
}

fn claim_analyzable_slot(started: &mut usize) -> bool {
    if *started >= MAX_FILES {
        false
    } else {
        *started += 1;
        true
    }
}

struct StructureFileContext<'a> {
    project_path: &'a Path,
    comparison: &'a ImmutableResolvedComparison,
    source_bytes: &'a mut u64,
    capture_bytes: &'a mut u64,
    git_control: &'a ReviewGitControl,
    analysis_control: &'a AnalysisControl,
    worker_control: &'a ReviewWorkerControl,
}

fn structure_file(
    context: &mut StructureFileContext<'_>,
    fact: &ReviewFileFact,
    hunks: Vec<ChangedHunk>,
) -> Result<FileBuildOutcome, String> {
    if let Some(truncation) = context
        .analysis_control
        .stop_truncation(std::time::Instant::now())
        .map_err(|error| error.to_string())?
    {
        let (reason, truncation) = syntax_omission(&truncation);
        return Ok(FileBuildOutcome::Omitted(reason, truncation));
    }

    let (old_language, new_language) = side_languages(fact);
    if let (Some(old_language), Some(new_language)) = (old_language, new_language)
        && old_language != new_language
    {
        let path = selected_path(fact).map(str::to_owned);
        let errors = vec![
            AnalysisError::new(
                path.clone(),
                AnalysisStage::Detection,
                format!(
                    "base language {old_language:?} differs from head language {new_language:?}"
                ),
            )
            .map_err(|error| error.to_string())?,
            AnalysisError::new(
                path,
                AnalysisStage::Comparison,
                "cross-language symbol matching is not supported",
            )
            .map_err(|error| error.to_string())?,
        ];
        return empty_file(fact, None, FileAnalysisStatus::Failed, hunks, errors, None)
            .map(Box::new)
            .map(FileBuildOutcome::File);
    }
    let Some(language) = new_language.or(old_language) else {
        return Err("analyzable file has no syntax language after deterministic preflight".into());
    };

    let remaining = match aggregate_source_capacity(*context.source_bytes)? {
        AggregateSourceCapacity::Remaining(remaining) => remaining,
        AggregateSourceCapacity::Exhausted(truncation) => {
            return Ok(FileBuildOutcome::Halted(
                OmittedFileReason::AggregateByteLimit,
                truncation,
            ));
        }
    };
    let source_request = ReviewSourceRequest::new(
        context.comparison.as_resolved().clone(),
        fact.old_path.clone(),
        fact.new_path.clone(),
    )
    .map_err(|error| error.to_string())?;
    let source_budget = ReviewSourceBudget::new(MAX_SOURCE_SIDE_BYTES, remaining)
        .map_err(|error| error.to_string())?;
    let source = match get_exact_review_source_with_control(
        context.project_path,
        &source_request,
        source_budget,
        context.git_control,
    ) {
        Ok(source) => source,
        Err(GitError::ReviewSourceBudgetExceeded {
            kind,
            observed,
            limit,
        }) => {
            let (reason, truncation) =
                source_limit_omission(kind, observed, limit, *context.source_bytes)?;
            return Ok(FileBuildOutcome::Omitted(reason, truncation));
        }
        Err(error) => {
            return unsuccessful_file(
                fact,
                detect_language(fact),
                FileAnalysisStatus::Failed,
                hunks,
                format!("failed to load exact source: {error}"),
                AnalysisStage::Parsing,
            )
            .map(Box::new)
            .map(FileBuildOutcome::File);
        }
    };
    if fact.old_path.is_some() != source.old_content.is_some()
        || fact.new_path.is_some() != source.new_content.is_some()
    {
        return unsuccessful_file(
            fact,
            Some(language),
            FileAnalysisStatus::Failed,
            hunks,
            "exact source response did not contain every requested comparison side",
            AnalysisStage::Parsing,
        )
        .map(Box::new)
        .map(FileBuildOutcome::File);
    }
    let loaded = source
        .old_content
        .as_ref()
        .map(|content| u64::try_from(content.len()))
        .transpose()
        .map_err(|_| "base source length does not fit the review byte counter".to_string())?
        .unwrap_or(0)
        .checked_add(
            source
                .new_content
                .as_ref()
                .map(|content| u64::try_from(content.len()))
                .transpose()
                .map_err(|_| "head source length does not fit the review byte counter".to_string())?
                .unwrap_or(0),
        )
        .ok_or_else(|| "source byte count overflowed".to_string())?;
    *context.source_bytes = context
        .source_bytes
        .checked_add(loaded)
        .ok_or_else(|| "aggregate source byte count overflowed".to_string())?;

    let rust = RustAdapter::new();
    let typescript = TypeScriptAdapter::new();
    let adapter: &dyn SyntaxAdapter = if rust.supports(language) {
        &rust
    } else if typescript.supports(language) {
        &typescript
    } else {
        return unsuccessful_file(
            fact,
            None,
            FileAnalysisStatus::Unsupported,
            hunks,
            "detected language has no registered syntax adapter",
            AnalysisStage::Detection,
        )
        .map(Box::new)
        .map(FileBuildOutcome::File);
    };
    let mut file_capture_bytes = 0_u64;
    let old_document = source
        .old_content
        .map(|content| {
            analyze_side_bounded(
                adapter,
                fact.old_path.as_deref(),
                language,
                content,
                context
                    .capture_bytes
                    .checked_add(file_capture_bytes)
                    .ok_or_else(|| "aggregate capture byte count overflowed".to_string())?,
                context.analysis_control,
            )
        })
        .transpose();
    let old_document = match old_document {
        Ok(Some(CapturedAnalysis::Document { document, bytes })) => {
            file_capture_bytes = file_capture_bytes
                .checked_add(bytes)
                .ok_or_else(|| "file capture byte count overflowed".to_string())?;
            Some(document)
        }
        Ok(Some(CapturedAnalysis::AggregateExhausted(truncation))) => {
            return Ok(FileBuildOutcome::Halted(
                OmittedFileReason::FactLimit,
                truncation,
            ));
        }
        Ok(None) => None,
        Err(error) => {
            return unsuccessful_file(
                fact,
                Some(language),
                FileAnalysisStatus::Failed,
                hunks,
                format!("base syntax analysis failed: {error}"),
                AnalysisStage::Parsing,
            )
            .map(Box::new)
            .map(FileBuildOutcome::File);
        }
    };
    let new_document = source
        .new_content
        .map(|content| {
            analyze_side_bounded(
                adapter,
                fact.new_path.as_deref(),
                language,
                content,
                context
                    .capture_bytes
                    .checked_add(file_capture_bytes)
                    .ok_or_else(|| "aggregate capture byte count overflowed".to_string())?,
                context.analysis_control,
            )
        })
        .transpose();
    let new_document = match new_document {
        Ok(Some(CapturedAnalysis::Document { document, bytes })) => {
            file_capture_bytes = file_capture_bytes
                .checked_add(bytes)
                .ok_or_else(|| "file capture byte count overflowed".to_string())?;
            Some(document)
        }
        Ok(Some(CapturedAnalysis::AggregateExhausted(truncation))) => {
            // Charge the retained base document before globally halting later analysis.
            commit_file_capture(context.capture_bytes, file_capture_bytes)?;
            return Ok(FileBuildOutcome::Halted(
                OmittedFileReason::FactLimit,
                truncation,
            ));
        }
        Ok(None) => None,
        Err(error) => {
            return unsuccessful_file(
                fact,
                Some(language),
                FileAnalysisStatus::Failed,
                hunks,
                format!("head syntax analysis failed: {error}"),
                AnalysisStage::Parsing,
            )
            .map(Box::new)
            .map(FileBuildOutcome::File);
        }
    };
    commit_file_capture(context.capture_bytes, file_capture_bytes)?;

    let mut checkpoint_error = None;
    let comparison = compare_structured_file_controlled(
        fact.old_path.as_deref(),
        fact.new_path.as_deref(),
        old_document.as_ref(),
        new_document.as_ref(),
        &hunks,
        &mut || {
            comparison_stop(
                context.worker_control,
                context.analysis_control,
                &mut checkpoint_error,
            )
        },
    );
    if let Some(error) = checkpoint_error {
        return Err(error);
    }
    match comparison {
        Ok(file) => Ok(FileBuildOutcome::File(Box::new(file))),
        Err(error) if error.stop_reason().is_some() => {
            context.worker_control.checkpoint()?;
            let truncation = context
                .analysis_control
                .stop_truncation(std::time::Instant::now())
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    "structured comparison stopped without control evidence".to_string()
                })?;
            let (reason, truncation) = syntax_omission(&truncation);
            Ok(FileBuildOutcome::Omitted(reason, truncation))
        }
        Err(error) => unsuccessful_file(
            fact,
            Some(language),
            FileAnalysisStatus::Failed,
            hunks,
            format!("structured comparison failed: {error}"),
            AnalysisStage::Comparison,
        )
        .map(Box::new)
        .map(FileBuildOutcome::File),
    }
}

fn deterministic_omission(
    fact: &ReviewFileFact,
    has_no_hunks: bool,
) -> Option<(Option<SyntaxLanguage>, OmittedFileReason)> {
    if fact.binary {
        return Some((detect_language(fact), OmittedFileReason::Binary));
    }
    if fact.submodule.is_some()
        || fact.old_mode.as_deref() == Some("160000")
        || fact.new_mode.as_deref() == Some("160000")
    {
        return Some((detect_language(fact), OmittedFileReason::Submodule));
    }
    if fact.status == ReviewFileStatus::ModeChanged && has_no_hunks {
        return Some((detect_language(fact), OmittedFileReason::ModeOnly));
    }
    let (old_language, new_language) = side_languages(fact);
    if fact.old_path.is_some() && old_language.is_none()
        || fact.new_path.is_some() && new_language.is_none()
    {
        return Some((None, OmittedFileReason::UnsupportedLanguage));
    }
    None
}

fn side_languages(fact: &ReviewFileFact) -> (Option<SyntaxLanguage>, Option<SyntaxLanguage>) {
    let old = fact
        .old_path
        .as_deref()
        .and_then(|path| SyntaxLanguage::from_path(Path::new(path)));
    let new = fact
        .new_path
        .as_deref()
        .and_then(|path| SyntaxLanguage::from_path(Path::new(path)));
    (old, new)
}

#[derive(Clone)]
struct OmissionAccumulator {
    count: u64,
    language: Option<SyntaxLanguage>,
    reason: OmittedFileReason,
    truncation: Option<ReviewTruncation>,
}

fn add_omission(
    omissions: &mut Vec<OmissionAccumulator>,
    language: Option<SyntaxLanguage>,
    reason: OmittedFileReason,
    truncation: Option<ReviewTruncation>,
) {
    if let Some(existing) = omissions.iter_mut().find(|existing| {
        existing.language == language
            && existing.reason == reason
            && existing.truncation == truncation
    }) {
        existing.count = existing.count.saturating_add(1);
    } else {
        omissions.push(OmissionAccumulator {
            count: 1,
            language,
            reason,
            truncation,
        });
    }
}

fn finish_omissions(omissions: Vec<OmissionAccumulator>) -> Result<Vec<OmittedFileGroup>, String> {
    omissions
        .into_iter()
        .map(|omission| {
            OmittedFileGroup::new(
                omission.count,
                omission.language,
                omission.reason,
                omission.truncation,
            )
            .map_err(|error| error.to_string())
        })
        .collect()
}

fn measured_truncation(
    reason: TruncationReason,
    limit: u64,
    observed: u64,
    detail: &str,
) -> ReviewTruncation {
    ReviewTruncation {
        reason,
        limit: Some(limit),
        observed: Some(observed),
        detail: Some(detail.to_string()),
    }
}

fn syntax_omission(truncation: &SyntaxTruncation) -> (OmittedFileReason, ReviewTruncation) {
    match truncation.reason() {
        SyntaxTruncationReason::Cancelled => (
            OmittedFileReason::Cancelled,
            ReviewTruncation {
                reason: TruncationReason::Cancelled,
                limit: None,
                observed: None,
                detail: Some("shared structured-review analysis".to_string()),
            },
        ),
        SyntaxTruncationReason::Time => (
            OmittedFileReason::TimeLimit,
            ReviewTruncation {
                reason: TruncationReason::TimeLimit,
                limit: truncation.limit(),
                observed: truncation.observed(),
                detail: Some("shared structured-review analysis".to_string()),
            },
        ),
        _ => (
            OmittedFileReason::FactLimit,
            ReviewTruncation {
                reason: TruncationReason::CaptureLimit,
                limit: truncation.limit(),
                observed: truncation.observed(),
                detail: Some("syntax capture".to_string()),
            },
        ),
    }
}

fn analysis_omission_at(
    control: &AnalysisControl,
    now: std::time::Instant,
) -> Result<Option<(OmittedFileReason, ReviewTruncation)>, String> {
    control
        .stop_truncation(now)
        .map_err(|error| error.to_string())
        .map(|truncation| truncation.as_ref().map(syntax_omission))
}

fn analysis_omission_now(
    control: &AnalysisControl,
) -> Result<Option<(OmittedFileReason, ReviewTruncation)>, String> {
    analysis_omission_at(control, std::time::Instant::now())
}

fn ensure_analysis_active(control: &AnalysisControl, phase: &str) -> Result<(), String> {
    let Some((reason, truncation)) = analysis_omission_now(control)? else {
        return Ok(());
    };
    Err(format!(
        "structured review stopped {phase}: {reason:?} ({:?})",
        truncation.reason
    ))
}

fn source_limit_omission(
    kind: ReviewSourceBudgetKind,
    observed: u64,
    limit: u64,
    consumed: u64,
) -> Result<(OmittedFileReason, ReviewTruncation), String> {
    match kind {
        ReviewSourceBudgetKind::PerFileSourceBytes => Ok((
            OmittedFileReason::SourceByteLimit,
            measured_truncation(TruncationReason::ByteLimit, limit, observed, "source side"),
        )),
        ReviewSourceBudgetKind::AggregateSourceBytes => Ok((
            OmittedFileReason::AggregateByteLimit,
            measured_truncation(
                TruncationReason::ByteLimit,
                MAX_SOURCE_TOTAL_BYTES,
                consumed
                    .checked_add(observed)
                    .ok_or_else(|| "aggregate source budget observation overflowed".to_string())?,
                "aggregate structured-review source",
            ),
        )),
    }
}

fn structured_fact_count(
    file: &StructuredFile,
    control: &ReviewWorkerControl,
) -> Result<u64, String> {
    let mut outline_count = 0_u64;
    let mut pending: Vec<&okena_review::OutlineFact> = file
        .old_outline()
        .iter()
        .chain(file.new_outline())
        .collect();
    while let Some(fact) = pending.pop() {
        control.checkpoint()?;
        outline_count = outline_count
            .checked_add(1)
            .ok_or_else(|| "structured fact count overflowed".to_string())?;
        pending.extend(fact.children());
    }
    let counts = [
        outline_count,
        file.symbol_changes().len() as u64,
        file.hotspots().len() as u64,
        file.call_diff().len() as u64,
    ];
    counts.into_iter().try_fold(0_u64, |total, count| {
        total
            .checked_add(count)
            .ok_or_else(|| "structured fact count overflowed".to_string())
    })
}

enum MeasuredFileSize {
    Bytes(usize),
    Exceeded,
    AnalysisStopped(OmittedFileReason, ReviewTruncation),
}

fn serialized_file_size(
    file: &StructuredFile,
    limit: usize,
    control: &ReviewWorkerControl,
    analysis: &AnalysisControl,
) -> Result<MeasuredFileSize, String> {
    let mut writer = SizeLimitedWriter::new(limit, control, analysis);
    match serde_json::to_writer(&mut writer, file) {
        Ok(()) => Ok(MeasuredFileSize::Bytes(writer.written)),
        Err(_) if writer.exceeded => Ok(MeasuredFileSize::Exceeded),
        Err(_) if writer.stop_error.is_some() => {
            control.checkpoint()?;
            if let Some((reason, truncation)) = analysis_omission_now(analysis)? {
                return Ok(MeasuredFileSize::AnalysisStopped(reason, truncation));
            }
            Err(writer
                .stop_error
                .unwrap_or_else(|| "review response sizing stopped".to_string()))
        }
        Err(error) => Err(format!("failed to size structured file response: {error}")),
    }
}

struct SizeLimitedWriter<'a> {
    written: usize,
    limit: usize,
    exceeded: bool,
    stop_error: Option<String>,
    control: &'a ReviewWorkerControl,
    analysis: &'a AnalysisControl,
}

impl<'a> SizeLimitedWriter<'a> {
    fn new(limit: usize, control: &'a ReviewWorkerControl, analysis: &'a AnalysisControl) -> Self {
        Self {
            written: 0,
            limit,
            exceeded: false,
            stop_error: None,
            control,
            analysis,
        }
    }
}

impl Write for SizeLimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Err(error) = response_checkpoint(self.control, Some(self.analysis)) {
            self.stop_error = Some(error);
            return Err(io::Error::other("review response sizing stopped"));
        }
        let write_len = bytes.len().min(RESPONSE_CHECKPOINT_BYTES);
        let Some(next) = self.written.checked_add(write_len) else {
            self.exceeded = true;
            return Err(io::Error::other("structured file size overflowed"));
        };
        if next > self.limit {
            self.exceeded = true;
            return Err(io::Error::other("structured file response limit reached"));
        }
        self.written = next;
        Ok(write_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

enum CapturedAnalysis {
    Document {
        document: okena_syntax::DocumentStructure,
        bytes: u64,
    },
    AggregateExhausted(ReviewTruncation),
}

fn commit_file_capture(aggregate: &mut u64, retained: u64) -> Result<(), String> {
    *aggregate = aggregate
        .checked_add(retained)
        .ok_or_else(|| "aggregate capture byte count overflowed".to_string())?;
    Ok(())
}

fn analyze_side_bounded(
    adapter: &dyn SyntaxAdapter,
    path: Option<&str>,
    language: SyntaxLanguage,
    content: String,
    retained_before: u64,
    control: &AnalysisControl,
) -> Result<CapturedAnalysis, String> {
    let aggregate_remaining = MAX_CAPTURE_TOTAL_BYTES.saturating_sub(retained_before);
    if aggregate_remaining == 0 {
        return Ok(CapturedAnalysis::AggregateExhausted(measured_truncation(
            TruncationReason::CaptureLimit,
            MAX_CAPTURE_TOTAL_BYTES,
            retained_before
                .checked_add(1)
                .ok_or_else(|| "aggregate capture observation overflowed".to_string())?,
            "aggregate retained syntax capture",
        )));
    }
    let capture_limit = MAX_CAPTURE_SIDE_BYTES.min(aggregate_remaining);
    let budget = AnalysisBudget::new(
        NonZeroU64::new(MAX_SOURCE_SIDE_BYTES).unwrap_or(NonZeroU64::MIN),
        NonZeroU32::new(MAX_SYMBOLS).unwrap_or(NonZeroU32::MIN),
        NonZeroU32::new(MAX_CALLS).unwrap_or(NonZeroU32::MIN),
        NonZeroU32::new(MAX_DIAGNOSTICS).unwrap_or(NonZeroU32::MIN),
    )
    .with_max_capture_bytes(NonZeroU64::new(capture_limit).unwrap_or(NonZeroU64::MIN));
    let path = path.ok_or_else(|| "source content has no comparison path".to_string())?;
    let input = AnalysisInput::new(path, language, content).map_err(|error| error.to_string())?;
    let document = adapter
        .analyze(input, budget, control)
        .map_err(|error| error.to_string())?;
    let bytes = document.estimated_owned_bytes();
    let observed = retained_before
        .checked_add(bytes)
        .ok_or_else(|| "aggregate capture byte count overflowed".to_string())?;
    if observed > MAX_CAPTURE_TOTAL_BYTES {
        return Ok(CapturedAnalysis::AggregateExhausted(measured_truncation(
            TruncationReason::CaptureLimit,
            MAX_CAPTURE_TOTAL_BYTES,
            observed,
            "aggregate retained syntax capture",
        )));
    }
    Ok(CapturedAnalysis::Document { document, bytes })
}

fn comparison_stop(
    worker: &ReviewWorkerControl,
    analysis: &AnalysisControl,
    error: &mut Option<String>,
) -> Option<ComparisonStopReason> {
    if worker.cancelled.load(Ordering::Acquire) {
        return Some(ComparisonStopReason::Disconnected);
    }
    match analysis.stop_truncation(std::time::Instant::now()) {
        Ok(Some(truncation)) => match truncation.reason() {
            SyntaxTruncationReason::Cancelled => Some(ComparisonStopReason::Cancelled),
            SyntaxTruncationReason::Time => Some(ComparisonStopReason::Deadline),
            _ => None,
        },
        Ok(None) => None,
        Err(model_error) => {
            *error = Some(format!("failed to read comparison control: {model_error}"));
            Some(ComparisonStopReason::Disconnected)
        }
    }
}

fn index_file_diffs(
    files: Vec<FileDiff>,
    control: &ReviewWorkerControl,
) -> Result<IndexedFileDiffs, String> {
    let mut indexed = HashMap::with_capacity(files.len());
    for file in files {
        control.checkpoint()?;
        let key = (file.old_path.clone(), file.new_path.clone());
        if indexed.insert(key.clone(), file).is_some() {
            return Err(format!(
                "exact diff contains duplicate path pair {:?} -> {:?}",
                key.0, key.1
            ));
        }
    }
    Ok(indexed)
}

fn changed_hunks(
    file: &FileDiff,
    control: &ReviewWorkerControl,
) -> Result<Vec<ChangedHunk>, String> {
    control.checkpoint()?;
    let mut blocks = Vec::new();
    for hunk in &file.hunks {
        let mut old = Vec::new();
        let mut new = Vec::new();
        for line in &hunk.lines {
            control.checkpoint()?;
            match line.line_type {
                DiffLineType::Removed => {
                    if let Some(line) = line.old_line_num {
                        old.push(line);
                    }
                }
                DiffLineType::Added => {
                    if let Some(line) = line.new_line_num {
                        new.push(line);
                    }
                }
                DiffLineType::Context | DiffLineType::Header => {
                    flush_edit_block(&mut blocks, &mut old, &mut new)?;
                }
            }
        }
        flush_edit_block(&mut blocks, &mut old, &mut new)?;
    }
    Ok(blocks)
}

fn flush_edit_block(
    blocks: &mut Vec<ChangedHunk>,
    old: &mut Vec<usize>,
    new: &mut Vec<usize>,
) -> Result<(), String> {
    if old.is_empty() && new.is_empty() {
        return Ok(());
    }
    let old_range = changed_range(old.drain(..))?;
    let new_range = changed_range(new.drain(..))?;
    blocks.push(ChangedHunk::new(old_range, new_range).map_err(|error| error.to_string())?);
    Ok(())
}

fn changed_range(lines: impl Iterator<Item = usize>) -> Result<Option<ChangedLineRange>, String> {
    let mut lines = lines.peekable();
    let Some(first) = lines.peek().copied() else {
        return Ok(None);
    };
    let mut last = first;
    for line in lines {
        last = line;
    }
    let start = u32::try_from(first)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| "changed line does not fit the 1-based review range".to_string())?;
    let end = u32::try_from(last)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| "changed line does not fit the 1-based review range".to_string())?;
    ChangedLineRange::new(start, end)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn unsuccessful_file(
    fact: &ReviewFileFact,
    language: Option<SyntaxLanguage>,
    status: FileAnalysisStatus,
    hunks: Vec<ChangedHunk>,
    message: impl Into<String>,
    stage: AnalysisStage,
) -> Result<StructuredFile, String> {
    let error = AnalysisError::new(selected_path(fact).map(str::to_owned), stage, message)
        .map_err(|error| error.to_string())?;
    empty_file(fact, language, status, hunks, vec![error], None)
}

fn empty_file(
    fact: &ReviewFileFact,
    language: Option<SyntaxLanguage>,
    status: FileAnalysisStatus,
    hunks: Vec<ChangedHunk>,
    errors: Vec<AnalysisError>,
    truncation: Option<ReviewTruncation>,
) -> Result<StructuredFile, String> {
    StructuredFile::new(
        fact.old_path.clone(),
        fact.new_path.clone(),
        language,
        None,
        None,
        status,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        hunks,
        errors,
        truncation,
    )
    .map_err(|error| error.to_string())
}

fn coverage_for(
    files: &[StructuredFile],
    omissions: &[OmittedFileGroup],
    language: Option<SyntaxLanguage>,
) -> Result<ReviewCoverage, String> {
    let mut counts = [0_u64; 5];
    for file in files
        .iter()
        .filter(|file| language.is_none_or(|language| file.language() == Some(language)))
    {
        add_status_count(&mut counts, file.status(), 1)?;
    }
    for omission in omissions
        .iter()
        .filter(|omission| language.is_none_or(|language| omission.language() == Some(language)))
    {
        add_status_count(&mut counts, omission.status(), omission.count())?;
    }
    let total = counts.into_iter().try_fold(0_u64, |total, count| {
        total
            .checked_add(count)
            .ok_or_else(|| "review coverage count overflowed".to_string())
    })?;
    ReviewCoverage::new(
        total,
        counts[0],
        counts[1],
        counts[2],
        counts[3],
        counts[4],
        aggregate_truncation(files, omissions, language),
    )
    .map_err(|error| error.to_string())
}

fn add_status_count(
    counts: &mut [u64; 5],
    status: FileAnalysisStatus,
    count: u64,
) -> Result<(), String> {
    let index = match status {
        FileAnalysisStatus::Parsed | FileAnalysisStatus::Partial => 0,
        FileAnalysisStatus::Pending => 1,
        FileAnalysisStatus::Skipped => 2,
        FileAnalysisStatus::Unsupported => 3,
        FileAnalysisStatus::Failed => 4,
    };
    counts[index] = counts[index]
        .checked_add(count)
        .ok_or_else(|| "review coverage count overflowed".to_string())?;
    Ok(())
}

fn language_coverage_for(
    files: &[StructuredFile],
    omissions: &[OmittedFileGroup],
) -> Result<Vec<LanguageCoverage>, String> {
    let mut entries = Vec::new();
    for language in [
        SyntaxLanguage::Rust,
        SyntaxLanguage::TypeScript,
        SyntaxLanguage::Tsx,
    ] {
        if files.iter().any(|file| file.language() == Some(language))
            || omissions
                .iter()
                .any(|omission| omission.language() == Some(language))
        {
            entries.push(LanguageCoverage::new(
                language,
                coverage_for(files, omissions, Some(language))?,
            ));
        }
    }
    Ok(entries)
}

fn aggregate_truncation(
    files: &[StructuredFile],
    omissions: &[OmittedFileGroup],
    language: Option<SyntaxLanguage>,
) -> Option<ReviewTruncation> {
    let truncations: Vec<&ReviewTruncation> = files
        .iter()
        .filter(|file| language.is_none_or(|language| file.language() == Some(language)))
        .filter_map(StructuredFile::truncation)
        .chain(
            omissions
                .iter()
                .filter(|omission| {
                    language.is_none_or(|language| omission.language() == Some(language))
                })
                .filter_map(OmittedFileGroup::truncation),
        )
        .collect();
    let first = truncations.first()?.to_owned().clone();
    if truncations.iter().all(|candidate| **candidate == first) {
        return Some(first);
    }
    let mut reasons: Vec<String> = truncations
        .iter()
        .map(|truncation| format!("{:?}", truncation.reason))
        .collect();
    reasons.sort();
    reasons.dedup();
    Some(ReviewTruncation {
        reason: TruncationReason::Other,
        limit: None,
        observed: None,
        detail: Some(format!(
            "multiple truncation reasons: {}",
            reasons.join(", ")
        )),
    })
}

fn detect_language(file: &ReviewFileFact) -> Option<SyntaxLanguage> {
    selected_path(file).and_then(|path| SyntaxLanguage::from_path(Path::new(path)))
}

fn selected_path(file: &ReviewFileFact) -> Option<&str> {
    match file.status {
        ReviewFileStatus::Deleted => file.old_path.as_deref(),
        _ => file.new_path.as_deref().or(file.old_path.as_deref()),
    }
}

fn display_paths(file: &ReviewFileFact) -> String {
    match (&file.old_path, &file.new_path) {
        (Some(old), Some(new)) if old != new => format!("{old} -> {new}"),
        (Some(path), _) | (_, Some(path)) => path.clone(),
        (None, None) => "<missing>".to_string(),
    }
}

fn serialize_inventory_response(
    response: ReviewInventory,
    control: &ReviewWorkerControl,
) -> Result<serde_json::Value, String> {
    serialize_bounded(control, |writer| serde_json::to_writer(writer, &response))
}

fn serialize_diff_response(
    response: ExactReviewDiffResponse,
    control: &ReviewWorkerControl,
) -> Result<serde_json::Value, String> {
    serialize_bounded(control, |writer| serde_json::to_writer(writer, &response))
}

fn serialize_source_response(
    response: ExactReviewSourceResponse,
    control: &ReviewWorkerControl,
) -> Result<serde_json::Value, String> {
    serialize_bounded(control, |writer| serde_json::to_writer(writer, &response))
}

fn serialize_structure_response(
    response: ReviewStructure,
    control: &ReviewWorkerControl,
) -> Result<serde_json::Value, String> {
    serialize_bounded(control, |writer| serde_json::to_writer(writer, &response))
}

fn serialize_bounded(
    control: &ReviewWorkerControl,
    serialize: impl FnOnce(&mut LimitedWriter<'_>) -> serde_json::Result<()>,
) -> Result<serde_json::Value, String> {
    serialize_bounded_with_limit(MAX_RESPONSE_BYTES, control, serialize)
}

fn serialize_bounded_with_limit(
    limit: usize,
    control: &ReviewWorkerControl,
    serialize: impl FnOnce(&mut LimitedWriter<'_>) -> serde_json::Result<()>,
) -> Result<serde_json::Value, String> {
    let analysis = control.analysis();
    response_checkpoint(control, analysis.as_ref())?;
    let mut writer = LimitedWriter::new(limit, control, analysis.as_ref());
    if let Err(error) = serialize(&mut writer) {
        if let Some(stop_error) = writer.stop_error {
            return Err(stop_error);
        }
        if writer.exceeded {
            return Err(format!(
                "review response exceeds the {} byte response limit",
                limit
            ));
        }
        return Err(format!("failed to serialize review response: {error}"));
    }
    response_checkpoint(control, analysis.as_ref())?;
    let mut reader = CheckpointReader::new(&writer.bytes, control, analysis.as_ref());
    let materialized = {
        let buffered = io::BufReader::with_capacity(RESPONSE_CHECKPOINT_BYTES, &mut reader);
        serde_json::from_reader(buffered)
    };
    let value = match materialized {
        Ok(value) => value,
        Err(_) if reader.stop_error.is_some() => {
            return Err(reader
                .stop_error
                .unwrap_or_else(|| "review response materialization stopped".to_string()));
        }
        Err(error) => {
            return Err(format!(
                "failed to materialize review response JSON: {error}"
            ));
        }
    };
    response_checkpoint(control, analysis.as_ref())?;
    Ok(value)
}

fn response_checkpoint(
    control: &ReviewWorkerControl,
    analysis: Option<&AnalysisControl>,
) -> Result<(), String> {
    response_checkpoint_at(control, analysis, std::time::Instant::now())
}

fn response_checkpoint_at(
    control: &ReviewWorkerControl,
    analysis: Option<&AnalysisControl>,
    now: std::time::Instant,
) -> Result<(), String> {
    control.checkpoint()?;
    if let Some(analysis) = analysis {
        let Some((reason, truncation)) = analysis_omission_at(analysis, now)? else {
            return Ok(());
        };
        return Err(format!(
            "structured review stopped during response materialization: {reason:?} ({:?})",
            truncation.reason
        ));
    }
    Ok(())
}

struct LimitedWriter<'a> {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
    stop_error: Option<String>,
    control: &'a ReviewWorkerControl,
    analysis: Option<&'a AnalysisControl>,
}

impl<'a> LimitedWriter<'a> {
    fn new(
        limit: usize,
        control: &'a ReviewWorkerControl,
        analysis: Option<&'a AnalysisControl>,
    ) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
            exceeded: false,
            stop_error: None,
            control,
            analysis,
        }
    }
}

impl Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Err(error) = response_checkpoint(self.control, self.analysis) {
            self.stop_error = Some(error);
            return Err(io::Error::other("review response serialization stopped"));
        }
        let write_len = bytes.len().min(RESPONSE_CHECKPOINT_BYTES);
        let Some(next_len) = self.bytes.len().checked_add(write_len) else {
            self.exceeded = true;
            return Err(io::Error::other("review response size overflowed"));
        };
        if next_len > self.limit {
            self.exceeded = true;
            return Err(io::Error::other("review response limit reached"));
        }
        self.bytes.extend_from_slice(&bytes[..write_len]);
        Ok(write_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct CheckpointReader<'a> {
    bytes: &'a [u8],
    position: usize,
    stop_error: Option<String>,
    control: &'a ReviewWorkerControl,
    analysis: Option<&'a AnalysisControl>,
}

impl<'a> CheckpointReader<'a> {
    fn new(
        bytes: &'a [u8],
        control: &'a ReviewWorkerControl,
        analysis: Option<&'a AnalysisControl>,
    ) -> Self {
        Self {
            bytes,
            position: 0,
            stop_error: None,
            control,
            analysis,
        }
    }
}

impl Read for CheckpointReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if let Err(error) = response_checkpoint(self.control, self.analysis) {
            self.stop_error = Some(error);
            return Err(io::Error::other("review response materialization stopped"));
        }
        if self.position == self.bytes.len() || output.is_empty() {
            return Ok(0);
        }
        let remaining = self.bytes.len() - self.position;
        let read_len = remaining.min(output.len()).min(RESPONSE_CHECKPOINT_BYTES);
        let end = self.position + read_len;
        output[..read_len].copy_from_slice(&self.bytes[self.position..end]);
        self.position = end;
        Ok(read_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::review::{
        ComparisonSide, FactProvenance, FileClassification, FileRole, ReviewFileStatus,
    };
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo(PathBuf);

    impl TestRepo {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "okena-daemon-review-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(path.join("src")).unwrap();
            let repo = Self(path);
            repo.git(&["init", "-b", "main"]);
            repo.git(&["config", "user.email", "review@example.com"]);
            repo.git(&["config", "user.name", "Review Test"]);
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .args(args)
                .current_dir(&self.0)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        }

        fn write(&self, path: &str, content: &str) {
            std::fs::write(self.0.join(path), content).unwrap();
        }

        fn commit_all(&self, message: &str) {
            self.git(&["add", "."]);
            self.git(&["commit", "-m", message]);
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn file(old: Option<&str>, new: Option<&str>, status: ReviewFileStatus) -> ReviewFileFact {
        ReviewFileFact {
            old_path: old.map(str::to_owned),
            new_path: new.map(str::to_owned),
            status,
            similarity: None,
            old_mode: old.map(|_| "100644".to_string()),
            new_mode: new.map(|_| "100644".to_string()),
            lines_added: Some(1),
            lines_deleted: Some(1),
            binary: false,
            submodule: None,
            classification: FileClassification::from_rule(
                FileRole::Unclassified,
                "builtin.unclassified",
            )
            .unwrap(),
            provenance: FactProvenance::Git,
        }
    }

    fn fake_action() -> PreparedReviewAction {
        PreparedReviewAction {
            project_path: PathBuf::from("/unused/review-test"),
            request: ReviewRequest::Inventory(DiffMode::WorkingTree),
        }
    }

    fn resolved_comparison(repo: &TestRepo) -> okena_core::review::ResolvedComparison {
        resolve_review_comparison_with_control(
            &repo.0,
            DiffMode::BranchCompare {
                base: "main".to_string(),
                head: "feature".to_string(),
            },
            &ReviewGitControl::new(Default::default()),
        )
        .unwrap()
    }

    fn execute_source_request(
        repo: &TestRepo,
        request: ReviewSourceRequest,
    ) -> Result<ExactReviewSourceResponse, String> {
        let result = execute_review_action(
            PreparedReviewAction {
                project_path: repo.0.clone(),
                request: ReviewRequest::Source(Box::new(request)),
            },
            &ReviewGitControl::new(Default::default()),
            &ReviewWorkerControl::default(),
        );
        match result {
            CommandResult::Ok(Some(value)) => serde_json::from_value(value)
                .map_err(|error| format!("invalid exact source response: {error}")),
            CommandResult::Ok(None) => Err("exact source response had no payload".to_string()),
            CommandResult::OkBytes(_) => {
                Err("exact source response unexpectedly returned raw bytes".to_string())
            }
            CommandResult::OkSnapshot { .. } => {
                Err("exact source response unexpectedly returned a snapshot".to_string())
            }
            CommandResult::Err(error) => Err(error),
        }
    }

    #[test]
    fn status_aware_language_detection_uses_the_surviving_side() {
        assert_eq!(
            detect_language(&file(None, Some("src/view.tsx"), ReviewFileStatus::Added)),
            Some(SyntaxLanguage::Tsx)
        );
        assert_eq!(
            detect_language(&file(Some("src/lib.rs"), None, ReviewFileStatus::Deleted)),
            Some(SyntaxLanguage::Rust)
        );
    }

    #[test]
    fn exact_source_action_returns_rename_add_and_delete_sides() {
        let repo = TestRepo::new();
        repo.write("src/old.rs", "pub fn renamed() -> u32 { 1 }\n");
        repo.write("src/deleted.rs", "pub fn deleted() {}\n");
        repo.commit_all("base");
        repo.git(&["checkout", "-b", "feature"]);
        repo.git(&["mv", "src/old.rs", "src/new.rs"]);
        repo.write("src/new.rs", "pub fn renamed() -> u32 { 2 }\n");
        repo.write("src/added.rs", "pub fn added() {}\n");
        std::fs::remove_file(repo.0.join("src/deleted.rs")).unwrap();
        repo.commit_all("feature");

        let comparison = resolved_comparison(&repo);
        assert!(is_review_action(&ActionRequest::ReviewSource {
            project_id: "test".to_string(),
            request: Box::new(
                ReviewSourceRequest::new(
                    comparison.clone(),
                    Some("src/old.rs".to_string()),
                    Some("src/new.rs".to_string()),
                )
                .unwrap(),
            ),
        }));
        let rename = execute_source_request(
            &repo,
            ReviewSourceRequest::new(
                comparison.clone(),
                Some("src/old.rs".to_string()),
                Some("src/new.rs".to_string()),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(rename.old_path(), Some("src/old.rs"));
        assert_eq!(rename.new_path(), Some("src/new.rs"));
        assert_eq!(
            rename.old_content(),
            Some("pub fn renamed() -> u32 { 1 }\n")
        );
        assert_eq!(
            rename.new_content(),
            Some("pub fn renamed() -> u32 { 2 }\n")
        );
        assert_eq!(rename.comparison().as_resolved(), &comparison);

        let addition = execute_source_request(
            &repo,
            ReviewSourceRequest::new(comparison.clone(), None, Some("src/added.rs".to_string()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(addition.old_content(), None);
        assert_eq!(addition.new_content(), Some("pub fn added() {}\n"));

        let deletion = execute_source_request(
            &repo,
            ReviewSourceRequest::new(comparison, Some("src/deleted.rs".to_string()), None).unwrap(),
        )
        .unwrap();
        assert_eq!(deletion.old_content(), Some("pub fn deleted() {}\n"));
        assert_eq!(deletion.new_content(), None);
    }

    #[test]
    fn exact_source_request_is_immune_to_a_moved_head_ref() {
        let repo = TestRepo::new();
        repo.write("src/lib.rs", "pub fn value() -> u32 { 1 }\n");
        repo.commit_all("base");
        repo.git(&["checkout", "-b", "feature"]);
        repo.write("src/lib.rs", "pub fn value() -> u32 { 2 }\n");
        repo.commit_all("frozen feature");
        let comparison = resolved_comparison(&repo);
        let request = ReviewSourceRequest::new(
            comparison.clone(),
            Some("src/lib.rs".to_string()),
            Some("src/lib.rs".to_string()),
        )
        .unwrap();

        repo.write("src/lib.rs", "pub fn value() -> u32 { 3 }\n");
        repo.commit_all("move feature ref");

        let source = execute_source_request(&repo, request).unwrap();
        assert_eq!(source.new_content(), Some("pub fn value() -> u32 { 2 }\n"));
        assert_eq!(source.comparison().as_resolved(), &comparison);
    }

    #[test]
    fn exact_source_enforces_side_limit_and_accepts_full_pair_boundary() {
        let repo = TestRepo::new();
        let side_len = usize::try_from(MAX_SOURCE_SIDE_BYTES).unwrap();
        repo.write("src/old.txt", &"a".repeat(side_len));
        repo.commit_all("base");
        repo.git(&["checkout", "-b", "feature"]);
        repo.git(&["mv", "src/old.txt", "src/new.txt"]);
        repo.write("src/new.txt", &"b".repeat(side_len));
        repo.write("src/oversize.txt", &"x".repeat(side_len + 1));
        repo.commit_all("feature");
        let comparison = resolved_comparison(&repo);

        let boundary = execute_source_request(
            &repo,
            ReviewSourceRequest::new(
                comparison.clone(),
                Some("src/old.txt".to_string()),
                Some("src/new.txt".to_string()),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(boundary.old_content().unwrap().len(), side_len);
        assert_eq!(boundary.new_content().unwrap().len(), side_len);

        let error = execute_source_request(
            &repo,
            ReviewSourceRequest::new(comparison, None, Some("src/oversize.txt".to_string()))
                .unwrap(),
        )
        .unwrap_err();
        assert!(error.contains("per-file byte budget exceeded"), "{error}");
        assert!(error.contains(&(MAX_SOURCE_SIDE_BYTES + 1).to_string()));
        assert!(error.contains(&MAX_SOURCE_SIDE_BYTES.to_string()));
    }

    #[test]
    fn exact_source_honors_worker_and_git_cancellation() {
        let repo = TestRepo::new();
        repo.write("src/lib.rs", "pub fn value() {}\n");
        repo.commit_all("base");
        repo.git(&["checkout", "-b", "feature"]);
        repo.write("src/lib.rs", "pub fn value() { changed(); }\n");
        repo.commit_all("feature");
        let request = ReviewSourceRequest::new(
            resolved_comparison(&repo),
            Some("src/lib.rs".to_string()),
            Some("src/lib.rs".to_string()),
        )
        .unwrap();
        let budget =
            ReviewSourceBudget::new(MAX_SOURCE_SIDE_BYTES, MAX_STANDALONE_SOURCE_TOTAL_BYTES)
                .unwrap();

        let git = ReviewGitControl::new(Default::default());
        git.cancel();
        assert!(
            get_exact_review_source_response_with_control(&repo.0, &request, budget, &git).is_err()
        );

        let worker = ReviewWorkerControl::default();
        worker.cancel();
        assert!(
            build_source(
                &repo.0,
                &request,
                &ReviewGitControl::new(Default::default()),
                &worker,
            )
            .is_err()
        );
    }

    #[test]
    fn changed_hunks_only_include_changed_lines() {
        let diff = FileDiff {
            old_path: Some("src/lib.rs".to_string()),
            new_path: Some("src/lib.rs".to_string()),
            hunks: vec![okena_git::diff::DiffHunk {
                header: "@@ -2,2 +2,2 @@".to_string(),
                old_start: 2,
                new_start: 2,
                lines: vec![
                    okena_git::diff::DiffLine {
                        line_type: DiffLineType::Removed,
                        content: "old".to_string(),
                        old_line_num: Some(2),
                        new_line_num: None,
                    },
                    okena_git::diff::DiffLine {
                        line_type: DiffLineType::Added,
                        content: "new".to_string(),
                        old_line_num: None,
                        new_line_num: Some(2),
                    },
                ],
            }],
            is_binary: false,
            lines_added: 1,
            lines_removed: 1,
        };
        let hunks = changed_hunks(&diff, &ReviewWorkerControl::default()).unwrap();
        assert_eq!(hunks[0].old().unwrap().start().get(), 2);
        assert_eq!(hunks[0].new_range().unwrap().end().get(), 2);
    }

    #[test]
    fn changed_hunks_split_clusters_separated_by_context() {
        use okena_git::diff::DiffLine;
        let diff = FileDiff {
            old_path: Some("src/lib.rs".to_string()),
            new_path: Some("src/lib.rs".to_string()),
            hunks: vec![okena_git::diff::DiffHunk {
                header: "@@ -2,4 +2,4 @@".to_string(),
                old_start: 2,
                new_start: 2,
                lines: vec![
                    DiffLine {
                        line_type: DiffLineType::Removed,
                        content: "old_a".into(),
                        old_line_num: Some(2),
                        new_line_num: None,
                    },
                    DiffLine {
                        line_type: DiffLineType::Added,
                        content: "new_a".into(),
                        old_line_num: None,
                        new_line_num: Some(2),
                    },
                    DiffLine {
                        line_type: DiffLineType::Context,
                        content: "unchanged_symbol".into(),
                        old_line_num: Some(3),
                        new_line_num: Some(3),
                    },
                    DiffLine {
                        line_type: DiffLineType::Removed,
                        content: "old_b".into(),
                        old_line_num: Some(4),
                        new_line_num: None,
                    },
                    DiffLine {
                        line_type: DiffLineType::Added,
                        content: "new_b".into(),
                        old_line_num: None,
                        new_line_num: Some(4),
                    },
                ],
            }],
            is_binary: false,
            lines_added: 2,
            lines_removed: 2,
        };
        let blocks = changed_hunks(&diff, &ReviewWorkerControl::default()).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].old().unwrap().end().get(), 2);
        assert_eq!(blocks[1].new_range().unwrap().start().get(), 4);
    }

    #[test]
    fn partial_file_truncation_reaches_aggregate_and_language_coverage() {
        use okena_syntax::{DocumentStatus, DocumentStructure, SyntaxProvenance};
        let truncation =
            SyntaxTruncation::new(SyntaxTruncationReason::SymbolCount, Some(1), Some(2)).unwrap();
        let document = DocumentStructure::new(
            "src/lib.rs",
            SyntaxProvenance::tree_sitter(SyntaxLanguage::Rust, "test-parser").unwrap(),
            DocumentStatus::Partial,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(truncation),
        )
        .unwrap();
        let file = compare_structured_file_controlled(
            None,
            Some("src/lib.rs"),
            None,
            Some(&document),
            &[],
            &mut || None,
        )
        .unwrap();
        let coverage = coverage_for(std::slice::from_ref(&file), &[], None).unwrap();
        let language = language_coverage_for(std::slice::from_ref(&file), &[]).unwrap();
        assert_eq!(
            coverage.truncation().unwrap().reason,
            TruncationReason::CaptureLimit
        );
        assert_eq!(
            language[0].coverage().truncation().unwrap().reason,
            TruncationReason::CaptureLimit
        );
    }

    #[test]
    fn parser_clock_starts_when_analysis_is_published() {
        let control = ReviewWorkerControl::default();
        std::thread::sleep(std::time::Duration::from_millis(60));
        let analysis = control.start_analysis();
        assert!(analysis.elapsed_micros(std::time::Instant::now()) < 20_000);

        let cancelled = ReviewWorkerControl::default();
        cancelled.cancel();
        assert!(cancelled.start_analysis().is_cancelled());
    }

    #[test]
    fn exact_aggregate_source_fill_rejects_the_next_file_strictly() {
        let AggregateSourceCapacity::Exhausted(truncation) =
            aggregate_source_capacity(MAX_SOURCE_TOTAL_BYTES).unwrap()
        else {
            panic!("an exact aggregate fill must reject the next analyzable file");
        };
        assert_eq!(truncation.reason, TruncationReason::ByteLimit);
        assert_eq!(truncation.limit, Some(MAX_SOURCE_TOTAL_BYTES));
        assert_eq!(
            truncation.observed,
            Some(MAX_SOURCE_TOTAL_BYTES.checked_add(1).unwrap())
        );
    }

    #[test]
    fn deterministic_deadline_checkpoint_rejects_late_success() {
        let analysis = AnalysisControl::new(NonZeroU64::new(1_000_000).unwrap());
        let future = std::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(2))
            .unwrap();
        let (reason, truncation) = analysis_omission_at(&analysis, future)
            .unwrap()
            .expect("the injected checkpoint instant is beyond the deadline");
        assert_eq!(reason, OmittedFileReason::TimeLimit);
        assert_eq!(truncation.reason, TruncationReason::TimeLimit);
        assert!(
            response_checkpoint_at(&ReviewWorkerControl::default(), Some(&analysis), future,)
                .is_err()
        );
    }

    #[test]
    fn daemon_control_maps_disconnect_cancellation_and_deadline() {
        let disconnected = ReviewWorkerControl::default();
        disconnected.cancel();
        let analysis = AnalysisControl::new(NonZeroU64::new(1_000_000).unwrap());
        assert_eq!(
            comparison_stop(&disconnected, &analysis, &mut None),
            Some(ComparisonStopReason::Disconnected)
        );

        let cancelled = AnalysisControl::new(NonZeroU64::new(1_000_000).unwrap());
        cancelled.cancel();
        assert_eq!(
            comparison_stop(&ReviewWorkerControl::default(), &cancelled, &mut None),
            Some(ComparisonStopReason::Cancelled)
        );

        let expired = AnalysisControl::new(NonZeroU64::MIN);
        while !expired.deadline_exceeded(std::time::Instant::now()) {
            std::hint::spin_loop();
        }
        assert_eq!(
            comparison_stop(&ReviewWorkerControl::default(), &expired, &mut None),
            Some(ComparisonStopReason::Deadline)
        );
    }

    #[test]
    fn aggregate_capture_budget_rejects_a_document_before_comparison() {
        let analysis = AnalysisControl::new(NonZeroU64::new(1_000_000).unwrap());
        let outcome = analyze_side_bounded(
            &RustAdapter::new(),
            Some("src/lib.rs"),
            SyntaxLanguage::Rust,
            "pub fn value() {}".into(),
            MAX_CAPTURE_TOTAL_BYTES - 1,
            &analysis,
        )
        .unwrap();
        let CapturedAnalysis::AggregateExhausted(truncation) = outcome else {
            panic!("one remaining capture byte must not retain a syntax document");
        };
        assert_eq!(truncation.reason, TruncationReason::CaptureLimit);
        assert_eq!(truncation.limit, Some(MAX_CAPTURE_TOTAL_BYTES));
        assert!(truncation.observed.unwrap() > MAX_CAPTURE_TOTAL_BYTES);
    }

    #[test]
    fn retained_old_side_is_charged_before_aggregate_capture_halt() {
        let analysis = AnalysisControl::new(NonZeroU64::new(1_000_000).unwrap());
        let CapturedAnalysis::Document {
            bytes: old_bytes, ..
        } = analyze_side_bounded(
            &RustAdapter::new(),
            Some("src/old.rs"),
            SyntaxLanguage::Rust,
            "pub fn old_value() {}".into(),
            0,
            &analysis,
        )
        .unwrap()
        else {
            panic!("small base document must fit");
        };
        let retained_before_old = MAX_CAPTURE_TOTAL_BYTES.checked_sub(old_bytes).unwrap();
        let mut retained_after_old = retained_before_old;
        commit_file_capture(&mut retained_after_old, old_bytes).unwrap();
        assert_eq!(retained_after_old, MAX_CAPTURE_TOTAL_BYTES);

        let CapturedAnalysis::AggregateExhausted(truncation) = analyze_side_bounded(
            &RustAdapter::new(),
            Some("src/head.rs"),
            SyntaxLanguage::Rust,
            "pub fn head_value() {}".into(),
            retained_after_old,
            &analysis,
        )
        .unwrap() else {
            panic!("head analysis must halt once the retained base fills the request budget");
        };
        assert_eq!(truncation.reason, TruncationReason::CaptureLimit);
        assert_eq!(truncation.limit, Some(MAX_CAPTURE_TOTAL_BYTES));
        assert_eq!(truncation.observed, Some(MAX_CAPTURE_TOTAL_BYTES + 1));

        let outcome = FileBuildOutcome::Halted(OmittedFileReason::FactLimit, truncation);
        let mut halted = None;
        if let FileBuildOutcome::Halted(reason, truncation) = outcome {
            halted = Some((reason, truncation));
        }
        let mut later_adapter_calls = 0_u32;
        let later_omission = match halted_omission(&halted) {
            Some(omission) => omission,
            None => {
                later_adapter_calls += 1;
                (
                    OmittedFileReason::FactLimit,
                    measured_truncation(
                        TruncationReason::CaptureLimit,
                        MAX_CAPTURE_TOTAL_BYTES,
                        MAX_CAPTURE_TOTAL_BYTES + 1,
                        "unexpected later parse",
                    ),
                )
            }
        };
        assert_eq!(later_adapter_calls, 0);
        assert_eq!(later_omission.0, OmittedFileReason::FactLimit);
        assert_eq!(later_omission.1.reason, TruncationReason::CaptureLimit);
    }

    #[test]
    fn unsupported_files_do_not_consume_analyzable_slots() {
        let unsupported = file(None, Some("notes.txt"), ReviewFileStatus::Added);
        for _ in 0..250 {
            assert_eq!(
                deterministic_omission(&unsupported, true).unwrap().1,
                OmittedFileReason::UnsupportedLanguage
            );
        }
        let mut started = 0;
        for _ in 0..MAX_FILES {
            assert!(claim_analyzable_slot(&mut started));
        }
        assert!(!claim_analyzable_slot(&mut started));
        assert_eq!(started, MAX_FILES);

        let mut submodule = file(None, Some("deps/lib"), ReviewFileStatus::Added);
        submodule.new_mode = Some("160000".to_string());
        assert_eq!(
            deterministic_omission(&submodule, true).unwrap().1,
            OmittedFileReason::Submodule
        );
    }

    #[test]
    fn cancelled_cpu_loop_stops_at_checkpoint() {
        let control = ReviewWorkerControl::default();
        control.cancel();
        let diff = FileDiff {
            old_path: Some("src/lib.rs".into()),
            new_path: Some("src/lib.rs".into()),
            hunks: Vec::new(),
            is_binary: false,
            lines_added: 0,
            lines_removed: 0,
        };
        assert!(changed_hunks(&diff, &control).is_err());
    }

    #[test]
    fn pending_files_are_counted_as_pending() {
        let pending = OmittedFileGroup::new(
            1,
            Some(SyntaxLanguage::Rust),
            OmittedFileReason::FileLimit,
            Some(ReviewTruncation {
                reason: TruncationReason::ItemLimit,
                limit: Some(1),
                observed: Some(2),
                detail: Some("files".to_string()),
            }),
        )
        .unwrap();
        let coverage = coverage_for(&[], &[pending], None).unwrap();
        assert_eq!(coverage.pending_items(), 1);
        assert_eq!(coverage.analyzed_items(), 0);
    }

    #[test]
    fn response_writer_stops_before_allocating_past_the_limit() {
        let error = serialize_bounded_with_limit(4, &ReviewWorkerControl::default(), |writer| {
            serde_json::to_writer(writer, &"too large")
        })
        .unwrap_err();
        assert!(error.contains("4 byte response limit"));
    }

    #[test]
    fn response_reader_stops_when_reply_closes_during_materialization() {
        let control = ReviewWorkerControl::default();
        let bytes = vec![b' '; RESPONSE_CHECKPOINT_BYTES * 2];
        let mut reader = CheckpointReader::new(&bytes, &control, None);
        let mut output = vec![0_u8; RESPONSE_CHECKPOINT_BYTES];
        assert_eq!(reader.read(&mut output).unwrap(), RESPONSE_CHECKPOINT_BYTES);
        control.cancel();
        assert!(reader.read(&mut output).is_err());
        assert_eq!(
            reader.stop_error.as_deref(),
            Some("review request cancelled")
        );
    }

    #[test]
    fn immutable_structure_uses_merge_base_and_ignores_a_moved_ref() {
        let repo = TestRepo::new();
        repo.write(
            "src/lib.rs",
            "pub fn value() -> u32 { 1 }\npub fn stable_a() {}\npub fn stable_b() {}\npub fn stable_c() {}\npub fn stable_d() {}\npub fn stable_e() {}\n",
        );
        repo.write(
            "src/view.tsx",
            "export function View() { return <div>{oldValue()}</div>; }\n",
        );
        std::fs::write(repo.0.join("src/image.bin"), [0_u8, 159]).unwrap();
        repo.write("notes.txt", "old note\n");
        repo.write("src/mode.rs", "pub fn unchanged() {}\n");
        repo.write("src/deleted.rs", "pub fn removed() {}\n");
        repo.commit_all("base");

        repo.git(&["checkout", "-b", "feature"]);
        repo.git(&["mv", "src/lib.rs", "src/core.rs"]);
        repo.write(
            "src/core.rs",
            "pub fn value(input: u32) -> u32 { input + 1 }\npub fn stable_a() {}\npub fn stable_b() {}\npub fn stable_c() {}\npub fn stable_d() {}\npub fn stable_e() {}\n",
        );
        repo.write(
            "src/view.tsx",
            "export function View() { return <div>{newValue(1)}</div>; }\n",
        );
        std::fs::write(repo.0.join("src/image.bin"), [0_u8, 160]).unwrap();
        repo.write("notes.txt", "new note\n");
        repo.write("src/added.rs", "pub fn added() {}\n");
        std::fs::remove_file(repo.0.join("src/deleted.rs")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(repo.0.join("src/mode.rs"))
                .unwrap()
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(repo.0.join("src/mode.rs"), permissions).unwrap();
        }
        repo.commit_all("feature change");
        let frozen_feature = repo.git(&["rev-parse", "HEAD"]);

        repo.git(&["checkout", "main"]);
        repo.write("README.md", "main moved\n");
        repo.commit_all("main moved");

        let git_control = ReviewGitControl::new(Default::default());
        let resolved = resolve_review_comparison_with_control(
            &repo.0,
            DiffMode::BranchCompare {
                base: "main".to_string(),
                head: "feature".to_string(),
            },
            &git_control,
        )
        .unwrap();
        assert_ne!(resolved.requested_base_oid(), resolved.merge_base_oid());
        assert_eq!(
            resolved.requested_head_oid().unwrap().as_str(),
            frozen_feature
        );
        let immutable = ImmutableResolvedComparison::try_from(resolved.clone()).unwrap();
        let request = ReviewDiffRequest::new(resolved, false).unwrap();
        let exact_diff =
            get_exact_review_diff_response_with_control(&repo.0, &request, &git_control).unwrap();
        assert_eq!(exact_diff.comparison(), &immutable);

        repo.git(&["checkout", "feature"]);
        repo.write("src/after-resolution.rs", "pub fn later() {}\n");
        repo.commit_all("move feature ref");

        let worker_control = ReviewWorkerControl::default();
        let structure = build_structure(&repo.0, request, &git_control, &worker_control).unwrap();
        assert_eq!(structure.comparison(), &immutable);
        assert_eq!(structure.files().len(), 4);
        assert!(structure.files().iter().all(|file| matches!(
            file.status(),
            FileAnalysisStatus::Parsed | FileAnalysisStatus::Partial
        )));
        assert_eq!(structure.coverage().unsupported_items(), 2);
        assert_eq!(structure.coverage().analyzed_items(), 4);
        assert_eq!(
            structure.coverage().total_items(),
            if cfg!(unix) { 7 } else { 6 }
        );
        assert_eq!(
            structure.coverage().skipped_items(),
            usize::from(cfg!(unix)) as u64
        );
        assert!(
            structure
                .files()
                .iter()
                .any(|file| file.language() == Some(SyntaxLanguage::Rust))
        );
        let renamed_rust = structure
            .files()
            .iter()
            .find(|file| file.new_path() == Some("src/core.rs"))
            .unwrap();
        assert_eq!(renamed_rust.old_path(), Some("src/lib.rs"));
        let renamed_symbol = renamed_rust
            .symbol_changes()
            .iter()
            .find(|change| {
                change
                    .new_fact()
                    .is_some_and(|symbol| symbol.key().name() == "value")
            })
            .unwrap();
        assert_eq!(renamed_symbol.navigation().side, ComparisonSide::Head);
        assert_eq!(renamed_symbol.navigation().path, "src/core.rs");
        let added_file = structure
            .files()
            .iter()
            .find(|file| file.new_path() == Some("src/added.rs"))
            .unwrap();
        let added_symbol = added_file.symbol_changes().first().unwrap();
        assert_eq!(added_symbol.navigation().side, ComparisonSide::Head);
        assert_eq!(added_symbol.navigation().path, "src/added.rs");
        let deleted_file = structure
            .files()
            .iter()
            .find(|file| file.old_path() == Some("src/deleted.rs"))
            .unwrap();
        let deleted_symbol = deleted_file.symbol_changes().first().unwrap();
        assert_eq!(deleted_symbol.navigation().side, ComparisonSide::Base);
        assert_eq!(deleted_symbol.navigation().path, "src/deleted.rs");
        assert!(
            structure
                .files()
                .iter()
                .any(|file| file.language() == Some(SyntaxLanguage::Tsx))
        );
        let tsx_file = structure
            .files()
            .iter()
            .find(|file| file.language() == Some(SyntaxLanguage::Tsx))
            .unwrap();
        assert!(!tsx_file.call_diff().is_empty());
        assert_eq!(structure.language_coverage().len(), 2);
        assert_eq!(
            structure
                .language_coverage()
                .iter()
                .find(|entry| entry.language() == SyntaxLanguage::Tsx)
                .unwrap()
                .coverage()
                .analyzed_items(),
            1
        );
        assert!(
            structure
                .files()
                .iter()
                .all(|file| file.new_path() != Some("src/after-resolution.rs"))
        );
        #[cfg(unix)]
        {
            let mode_only = structure
                .omissions()
                .iter()
                .find(|omission| omission.reason() == OmittedFileReason::ModeOnly)
                .unwrap();
            assert_eq!(mode_only.status(), FileAnalysisStatus::Skipped);
            assert_eq!(mode_only.count(), 1);
        }

        let inventory_result = execute_review_action(
            PreparedReviewAction {
                project_path: repo.0.clone(),
                request: ReviewRequest::Inventory(DiffMode::BranchCompare {
                    base: "main".to_string(),
                    head: "feature".to_string(),
                }),
            },
            &ReviewGitControl::new(Default::default()),
            &ReviewWorkerControl::default(),
        );
        let CommandResult::Ok(Some(inventory_value)) = inventory_result else {
            panic!("inventory action worker did not return a JSON payload");
        };
        let inventory: ReviewInventory = serde_json::from_value(inventory_value).unwrap();
        assert!(
            inventory
                .files
                .iter()
                .all(|file| file.provenance == FactProvenance::Git)
        );
        let rust_file = inventory
            .files
            .iter()
            .find(|file| file.new_path.as_deref() == Some("src/core.rs"))
            .unwrap();
        assert_eq!(rust_file.classification.role(), FileRole::Implementation);
    }

    #[test]
    fn whitespace_ignored_diff_is_a_skipped_omission() {
        let repo = TestRepo::new();
        repo.write("src/lib.rs", "pub fn value() -> u32 { 1 }\n");
        repo.commit_all("base");
        repo.git(&["checkout", "-b", "feature"]);
        repo.write("src/lib.rs", "pub fn value()  ->  u32  { 1 }\n");
        repo.commit_all("whitespace only");

        let git_control = ReviewGitControl::new(Default::default());
        let resolved = resolve_review_comparison_with_control(
            &repo.0,
            DiffMode::BranchCompare {
                base: "main".to_string(),
                head: "feature".to_string(),
            },
            &git_control,
        )
        .unwrap();
        let request = ReviewDiffRequest::new(resolved, true).unwrap();
        let structure = build_structure(
            &repo.0,
            request,
            &git_control,
            &ReviewWorkerControl::default(),
        )
        .unwrap();
        assert!(structure.files().is_empty());
        assert_eq!(structure.omissions().len(), 1);
        assert_eq!(
            structure.omissions()[0].reason(),
            OmittedFileReason::WhitespaceIgnored
        );
        assert_eq!(structure.coverage().skipped_items(), 1);
    }

    #[test]
    fn binary_addition_and_deletion_pair_with_inventory() {
        let repo = TestRepo::new();
        std::fs::create_dir_all(repo.0.join("assets")).unwrap();
        std::fs::write(repo.0.join("assets/deleted.png"), [0_u8, 1, 2]).unwrap();
        repo.commit_all("base");
        repo.git(&["checkout", "-b", "feature"]);
        std::fs::remove_file(repo.0.join("assets/deleted.png")).unwrap();
        std::fs::write(repo.0.join("assets/added.png"), [0_u8, 3, 4]).unwrap();
        repo.commit_all("replace binary asset");

        let git_control = ReviewGitControl::new(Default::default());
        let resolved = resolve_review_comparison_with_control(
            &repo.0,
            DiffMode::BranchCompare {
                base: "main".into(),
                head: "feature".into(),
            },
            &git_control,
        )
        .unwrap();
        let request = ReviewDiffRequest::new(resolved, false).unwrap();
        let structure = build_structure(
            &repo.0,
            request,
            &git_control,
            &ReviewWorkerControl::default(),
        )
        .unwrap();

        assert!(structure.files().is_empty());
        assert_eq!(structure.omissions().len(), 1);
        assert_eq!(structure.omissions()[0].reason(), OmittedFileReason::Binary);
        assert_eq!(structure.omissions()[0].count(), 2);
        assert_eq!(structure.coverage().unsupported_items(), 2);
    }

    #[test]
    fn supported_cross_grammar_rename_is_an_explicit_failed_file() {
        let repo = TestRepo::new();
        repo.write(
            "src/value.rs",
            "// stable one\n// stable two\n// stable three\npub fn value() {}\n",
        );
        repo.commit_all("base");
        repo.git(&["checkout", "-b", "feature"]);
        repo.git(&["mv", "src/value.rs", "src/value.ts"]);
        repo.commit_all("rename across grammars");

        let git_control = ReviewGitControl::new(Default::default());
        let resolved = resolve_review_comparison_with_control(
            &repo.0,
            DiffMode::BranchCompare {
                base: "main".into(),
                head: "feature".into(),
            },
            &git_control,
        )
        .unwrap();
        let request = ReviewDiffRequest::new(resolved, false).unwrap();
        let structure = build_structure(
            &repo.0,
            request,
            &git_control,
            &ReviewWorkerControl::default(),
        )
        .unwrap();

        assert!(structure.omissions().is_empty());
        assert_eq!(structure.files().len(), 1);
        let file = &structure.files()[0];
        assert_eq!(file.old_path(), Some("src/value.rs"));
        assert_eq!(file.new_path(), Some("src/value.ts"));
        assert_eq!(file.status(), FileAnalysisStatus::Failed);
        assert!(
            file.errors()
                .iter()
                .any(|error| error.stage() == AnalysisStage::Detection)
        );
        assert!(
            file.errors()
                .iter()
                .any(|error| error.stage() == AnalysisStage::Comparison)
        );
        assert_eq!(structure.coverage().failed_items(), 1);
    }

    #[test]
    fn changed_submodule_keeps_exact_oids_and_becomes_structure_omission() {
        let child = TestRepo::new();
        child.write("src/lib.rs", "pub fn version() -> u32 { 1 }\n");
        child.commit_all("child base");
        let old_oid = child.git(&["rev-parse", "HEAD"]);

        let parent = TestRepo::new();
        let child_path = child.0.to_str().unwrap();
        parent.git(&[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            child_path,
            "deps/lib",
        ]);
        parent.commit_all("add submodule");
        parent.git(&["checkout", "-b", "feature"]);

        child.write("src/lib.rs", "pub fn version() -> u32 { 2 }\n");
        child.commit_all("child feature");
        let new_oid = child.git(&["rev-parse", "HEAD"]);
        parent.git(&["-C", "deps/lib", "fetch", "origin"]);
        parent.git(&["-C", "deps/lib", "checkout", &new_oid]);
        parent.commit_all("update submodule");

        let git_control = ReviewGitControl::new(Default::default());
        let resolved = resolve_review_comparison_with_control(
            &parent.0,
            DiffMode::BranchCompare {
                base: "main".into(),
                head: "feature".into(),
            },
            &git_control,
        )
        .unwrap();
        let immutable = ImmutableResolvedComparison::try_from(resolved.clone()).unwrap();
        let inventory =
            get_review_inventory_with_control(&parent.0, &immutable, &git_control).unwrap();
        let submodule = inventory
            .files
            .iter()
            .find(|file| file.new_path.as_deref() == Some("deps/lib"))
            .unwrap();
        assert_eq!(submodule.old_path.as_deref(), Some("deps/lib"));
        assert_eq!(submodule.status, ReviewFileStatus::SubmoduleChanged);
        let submodule_change = submodule.submodule.as_ref().unwrap();
        assert_eq!(submodule_change.old_oid.as_ref().unwrap().as_str(), old_oid);
        assert_eq!(submodule_change.new_oid.as_ref().unwrap().as_str(), new_oid);

        let request = ReviewDiffRequest::new(resolved, false).unwrap();
        let exact =
            get_exact_review_diff_response_with_control(&parent.0, &request, &git_control).unwrap();
        let paired_diff = exact
            .diff()
            .files
            .iter()
            .find(|file| file.new_path.as_deref() == Some("deps/lib"))
            .unwrap();
        assert_eq!(paired_diff.old_path.as_deref(), Some("deps/lib"));

        let structure = build_structure(
            &parent.0,
            request,
            &git_control,
            &ReviewWorkerControl::default(),
        )
        .unwrap();
        assert!(structure.files().is_empty());
        let omission = structure
            .omissions()
            .iter()
            .find(|omission| omission.reason() == OmittedFileReason::Submodule)
            .unwrap();
        assert_eq!(omission.count(), 1);
        assert_eq!(structure.coverage().unsupported_items(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reply_close_cancels_both_controls_and_waits_for_worker() {
        let permits = Arc::new(Semaphore::new(1));
        let (reply, receiver) = oneshot::channel();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        spawn_review_action_with(
            fake_action(),
            Some(reply),
            &tokio::runtime::Handle::current(),
            permits.clone(),
            move |_, git, control| {
                let parser = control.start_analysis();
                started_tx.send(()).unwrap();
                while !git.is_cancelled()
                    || !control.cancelled.load(Ordering::Acquire)
                    || !parser.is_cancelled()
                {
                    std::thread::yield_now();
                }
                finished_tx.send(()).unwrap();
                CommandResult::Ok(None)
            },
        );
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        drop(receiver);
        finished_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while permits.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn semaphore_caps_review_workers_at_two() {
        let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_REVIEWS));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let mut receivers = Vec::new();
        for _ in 0..3 {
            let (reply, receiver) = oneshot::channel();
            receivers.push(receiver);
            let active = active.clone();
            let maximum = maximum.clone();
            let release = release.clone();
            let started_tx = started_tx.clone();
            spawn_review_action_with(
                fake_action(),
                Some(reply),
                &tokio::runtime::Handle::current(),
                permits.clone(),
                move |_, _, _| {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    started_tx.send(()).unwrap();
                    while !release.load(Ordering::SeqCst) {
                        std::thread::yield_now();
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                    CommandResult::Ok(None)
                },
            );
        }
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(started_rx.try_recv().is_err());
        assert_eq!(maximum.load(Ordering::SeqCst), 2);
        release.store(true, Ordering::SeqCst);
        let mut results = Vec::new();
        for receiver in receivers {
            results.push(
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver)
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        assert!(started_rx.try_recv().is_err());
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, CommandResult::Err(error) if error.contains("executor busy")))
                .count(),
            1
        );
        assert_eq!(maximum.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn aggregate_budget_omissions_produce_pending_coverage() {
        let (source_reason, source_truncation) = source_limit_omission(
            ReviewSourceBudgetKind::AggregateSourceBytes,
            MAX_SOURCE_TOTAL_BYTES,
            1,
            1,
        )
        .unwrap();
        assert_eq!(source_reason, OmittedFileReason::AggregateByteLimit);
        let mut omissions = Vec::new();
        add_omission(
            &mut omissions,
            Some(SyntaxLanguage::Rust),
            source_reason,
            Some(source_truncation),
        );
        add_omission(
            &mut omissions,
            Some(SyntaxLanguage::Rust),
            OmittedFileReason::FactLimit,
            Some(measured_truncation(
                TruncationReason::CaptureLimit,
                10,
                11,
                "facts",
            )),
        );
        add_omission(
            &mut omissions,
            Some(SyntaxLanguage::Rust),
            OmittedFileReason::ResponseLimit,
            Some(measured_truncation(
                TruncationReason::ResponseLimit,
                20,
                21,
                "response",
            )),
        );
        let omissions = finish_omissions(omissions).unwrap();
        let coverage = coverage_for(&[], &omissions, None).unwrap();
        assert_eq!(coverage.pending_items(), 3);
        assert_eq!(
            coverage.truncation().unwrap().reason,
            TruncationReason::Other
        );
    }
}
