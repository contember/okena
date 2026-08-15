//! Global command bus: the single choke point for one-shot external process
//! execution (`git`, `gh`, `docker`, `lsof`, …).
//!
//! Why this exists: the resource these commands contend for — open file
//! descriptors and live child processes — is *process-global*. Per-domain
//! concurrency caps (e.g. one in the git poller, another in the services
//! poller) are each safe in isolation but do **not** compose: their sum can
//! still trip `RLIMIT_NOFILE` and surface as `EMFILE` (#125). Routing every
//! one-shot command through a single bus with bounded worker lanes makes the
//! cap actually global.
//!
//! Scope: this governs *one-shot* "spawn, capture output, exit" commands. It
//! deliberately does **not** carry interactive PTY shells — those are
//! long-lived bidirectional streams owned by `okena-terminal`. PTY headroom is
//! handled separately by bumping the soft FD limit at startup
//! ([`super::raise_fd_limit`]).
//!
//! Design notes:
//! - Runtime-agnostic: pure `std` threads + a condvar work queue. Callers on
//!   gpui/smol, tokio, or raw threads all submit the same way and block on
//!   [`CommandHandle::wait`] (typically inside `smol::unblock`, exactly where
//!   they previously called `safe_output`).
//! - Lanes ([`Lane`]) keep a 5-minute hook from starving the 5-second git
//!   poller: each lane has its own fixed worker pool, so they cannot contend
//!   for the same permits.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io::Read;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::process::Output;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

/// Execution lane. Each lane is an independent bounded worker pool, so work in
/// one lane can never consume the permits another lane needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// User-triggered, latency-sensitive one-shots (checkout, `docker compose
    /// start`, …). The default lane for [`super::safe_output`].
    Interactive,
    /// Background pollers (git status fallbacks, `gh` PR/CI checks, `docker
    /// compose ps`). Tightly capped — these fan out across every project.
    Poll,
    /// Long-running or unbounded-duration commands (headless hook fallback,
    /// updater archive extraction). Isolated so they never block the pollers.
    Long,
}

impl Lane {
    pub(super) fn workers(self) -> usize {
        match self {
            // Sum across lanes (4 + 4 + 2 = 10) is the effective global cap on
            // concurrent child processes — comfortably under every platform's
            // FD budget once the soft limit is raised at startup. Kept modest
            // because each worker is a permanently-resident OS thread; status
            // polling is now in-process (gix), so the bus mostly runs periodic
            // `gh` PR/CI calls and occasional git CLI fallbacks.
            Lane::Interactive => 4,
            Lane::Poll => 4,
            Lane::Long => 2,
        }
    }

    fn index(self) -> usize {
        match self {
            Lane::Interactive => 0,
            Lane::Poll => 1,
            Lane::Long => 2,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Lane::Interactive => "interactive",
            Lane::Poll => "poll",
            Lane::Long => "long",
        }
    }
}

const LANE_COUNT: usize = 3;

thread_local! {
    /// The lane used by [`super::safe_output`] / [`super::safe_output_with_timeout`]
    /// on the current thread. Pollers set this to [`Lane::Poll`] via
    /// [`super::with_lane`] so they opt out of the interactive pool without
    /// having to thread a lane parameter through every git/gh helper.
    static CURRENT_LANE: std::cell::Cell<Lane> = const { std::cell::Cell::new(Lane::Interactive) };
}

/// Run `f` with the bus lane for this thread set to `lane`, restoring the
/// previous lane afterwards (re-entrant safe).
pub fn with_lane<R>(lane: Lane, f: impl FnOnce() -> R) -> R {
    let prev = CURRENT_LANE.with(|c| c.replace(lane));
    let result = f();
    CURRENT_LANE.with(|c| c.set(prev));
    result
}

/// The lane currently active on this thread (defaults to [`Lane::Interactive`]).
pub fn current_lane() -> Lane {
    CURRENT_LANE.with(|c| c.get())
}

/// Fixed-width capture limits for one command's stdout and stderr streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLimits {
    stdout_bytes: NonZeroU64,
    stderr_bytes: NonZeroU64,
}

impl OutputLimits {
    pub fn new(stdout_bytes: NonZeroU64, stderr_bytes: NonZeroU64) -> Self {
        Self {
            stdout_bytes,
            stderr_bytes,
        }
    }

    pub fn stdout_bytes(self) -> NonZeroU64 {
        self.stdout_bytes
    }

    pub fn stderr_bytes(self) -> NonZeroU64 {
        self.stderr_bytes
    }
}

/// Stage at which command execution or cleanup failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOperation {
    Spawn,
    SpawnStdoutReader,
    SpawnStderrReader,
    Poll,
    TerminateTree,
    KillChild,
    WaitChild,
    ReadStdout,
    ReadStderr,
    JoinStdoutReader,
    JoinStderrReader,
    ComputeDeadline,
    Mock,
    Worker,
}

impl fmt::Display for CommandOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Spawn => "spawn",
            Self::SpawnStdoutReader => "spawn stdout reader",
            Self::SpawnStderrReader => "spawn stderr reader",
            Self::Poll => "poll",
            Self::TerminateTree => "terminate process tree",
            Self::KillChild => "kill child",
            Self::WaitChild => "wait for child",
            Self::ReadStdout => "read stdout",
            Self::ReadStderr => "read stderr",
            Self::JoinStdoutReader => "join stdout reader",
            Self::JoinStderrReader => "join stderr reader",
            Self::ComputeDeadline => "compute deadline",
            Self::Mock => "mock command",
            Self::Worker => "command bus worker",
        };
        formatter.write_str(name)
    }
}

/// The first condition that prevented a command from completing normally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandFailureCause {
    Cancelled {
        at: Instant,
    },
    DeadlineExceeded {
        deadline: Instant,
    },
    StdoutLimitExceeded {
        at: Instant,
        limit: u64,
        observed: u64,
    },
    StderrLimitExceeded {
        at: Instant,
        limit: u64,
        observed: u64,
    },
    Process {
        at: Instant,
        operation: CommandOperation,
        kind: std::io::ErrorKind,
        message: String,
    },
}

impl CommandFailureCause {
    fn at(&self) -> Instant {
        match self {
            Self::Cancelled { at }
            | Self::StdoutLimitExceeded { at, .. }
            | Self::StderrLimitExceeded { at, .. }
            | Self::Process { at, .. } => *at,
            Self::DeadlineExceeded { deadline } => *deadline,
        }
    }

    fn tie_priority(&self) -> u8 {
        match self {
            Self::DeadlineExceeded { .. } => 0,
            Self::Cancelled { .. } => 1,
            Self::StdoutLimitExceeded { .. } => 2,
            Self::StderrLimitExceeded { .. } => 3,
            Self::Process { .. } => 4,
        }
    }

    fn precedes(&self, current: &Self) -> bool {
        (self.at(), self.tie_priority()) < (current.at(), current.tie_priority())
    }

    fn io_kind(&self) -> std::io::ErrorKind {
        match self {
            Self::Cancelled { .. } => std::io::ErrorKind::Interrupted,
            Self::DeadlineExceeded { .. } => std::io::ErrorKind::TimedOut,
            Self::StdoutLimitExceeded { .. } | Self::StderrLimitExceeded { .. } => {
                std::io::ErrorKind::Other
            }
            Self::Process { kind, .. } => *kind,
        }
    }
}

impl fmt::Display for CommandFailureCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled { .. } => formatter.write_str("command cancelled"),
            Self::DeadlineExceeded { .. } => formatter.write_str("process timed out"),
            Self::StdoutLimitExceeded {
                limit, observed, ..
            } => write!(
                formatter,
                "stdout exceeded {limit} bytes (observed at least {observed})"
            ),
            Self::StderrLimitExceeded {
                limit, observed, ..
            } => write!(
                formatter,
                "stderr exceeded {limit} bytes (observed at least {observed})"
            ),
            Self::Process {
                operation, message, ..
            } => write!(formatter, "{operation}: {message}"),
        }
    }
}

/// A secondary failure observed while terminating, reaping, or draining a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCleanupFailure {
    pub operation: CommandOperation,
    pub kind: std::io::ErrorKind,
    pub message: String,
}

/// Detailed command failure with a stable primary cause and cleanup evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandFailure {
    pub primary: CommandFailureCause,
    pub cleanup: Vec<CommandCleanupFailure>,
}

impl CommandFailure {
    fn into_io_error(self) -> std::io::Error {
        std::io::Error::new(self.primary.io_kind(), self)
    }
}

impl fmt::Display for CommandFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.primary)?;
        if !self.cleanup.is_empty() {
            write!(formatter, " ({} cleanup failure(s))", self.cleanup.len())?;
        }
        Ok(())
    }
}

impl std::error::Error for CommandFailure {}

/// A fully-described one-shot command. Built directly, or extracted from a
/// configured [`std::process::Command`] via [`CommandSpec::from_command`].
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub timeout: Option<Duration>,
    /// Optional request-wide absolute deadline. Unlike `timeout`, queue time is included.
    pub deadline: Option<Instant>,
    /// Optional independent stdout/stderr capture limits.
    pub output_limits: Option<OutputLimits>,
    pub lane: Lane,
    /// Stable short label for the audit log (e.g. `"git.worktree.list"`). Falls
    /// back to the program name when `None`.
    pub label: Option<&'static str>,
    /// Optional cancellation group. All in-flight commands sharing a scope can
    /// be killed at once via [`CommandBus::cancel_scope`] (e.g. on project
    /// close or app shutdown).
    pub scope: Option<u64>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            timeout: None,
            deadline: None,
            output_limits: None,
            lane: current_lane(),
            label: None,
            scope: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    pub fn output_limits(mut self, limits: OutputLimits) -> Self {
        self.output_limits = Some(limits);
        self
    }

    pub fn lane(mut self, lane: Lane) -> Self {
        self.lane = lane;
        self
    }

    pub fn label(mut self, label: &'static str) -> Self {
        self.label = Some(label);
        self
    }

    pub fn scope(mut self, scope: u64) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Extract a spec from an already-configured [`std::process::Command`].
    ///
    /// This is what lets `safe_output(command("git").args(..).current_dir(..))`
    /// route transparently through the bus: program, args, cwd and explicit env
    /// overrides are read back off the builder. Custom stdio / creation flags
    /// are *not* preserved — the bus re-applies its own piped stdio and the
    /// Windows no-window flag — which is fine because every `safe_output`
    /// caller only sets args/cwd/env. Lane defaults to the thread's
    /// [`current_lane`].
    pub fn from_command(cmd: &std::process::Command) -> Self {
        let program = cmd.get_program().to_string_lossy().into_owned();
        let args = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let cwd = cmd.get_current_dir().map(|p| p.to_path_buf());
        let env = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        Self {
            program,
            args,
            cwd,
            env,
            timeout: None,
            deadline: None,
            output_limits: None,
            lane: current_lane(),
            label: None,
            scope: None,
        }
    }

    /// Full invocation for the audit log: optional label + program + args
    /// (+ cwd). Lets a review of `okena::cmd` see exactly what ran, with which
    /// arguments and in which repo, not just the program name.
    fn audit_detail(&self) -> String {
        let mut s = String::new();
        if let Some(label) = self.label {
            s.push_str(label);
            s.push_str(": ");
        }
        s.push_str(&self.program);
        for a in &self.args {
            s.push(' ');
            s.push_str(a);
        }
        if let Some(cwd) = &self.cwd {
            use std::fmt::Write as _;
            let _ = write!(s, " (cwd={})", cwd.display());
        }
        s
    }

    fn build(&self) -> std::process::Command {
        let mut cmd = super::command(&self.program);
        cmd.args(&self.args);
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd
    }
}

#[derive(Default)]
struct FailureState {
    primary: Option<CommandFailureCause>,
    cleanup: Vec<CommandCleanupFailure>,
    completion_accepted: bool,
    finalized: bool,
}

/// Per-job shared control block. It latches the actual first stop condition and
/// owns the live process-tree registration used by every cancellation source.
struct JobControl {
    failure: Mutex<FailureState>,
    /// Set by the worker once the process tree is owned; taken to kill it.
    kill: Mutex<Option<KillHandle>>,
}

/// A minimal handle that can kill a spawned process tree from another thread.
struct KillHandle {
    tree: Arc<ProcessTree>,
}

impl KillHandle {
    fn kill(&self) -> std::io::Result<()> {
        self.tree.terminate()
    }
}

/// Clears the externally reachable kill handle before the owned process-group
/// identifier or job handle can become stale.
struct KillRegistration<'a> {
    control: &'a JobControl,
    tree: Arc<ProcessTree>,
}

impl Drop for KillRegistration<'_> {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Err(error) = self.tree.terminate()
        {
            self.control
                .append_cleanup(CommandOperation::TerminateTree, error);
        }
        let mut guard = self.control.kill.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
}

impl JobControl {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            failure: Mutex::new(FailureState::default()),
            kill: Mutex::new(None),
        })
    }

    fn cancel(&self) {
        let should_terminate = {
            let mut state = self
                .failure
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.completion_accepted || state.finalized {
                false
            } else {
                let cause = CommandFailureCause::Cancelled { at: Instant::now() };
                Self::insert_cause(&mut state, cause);
                true
            }
        };
        if should_terminate {
            self.terminate_registered();
        }
    }

    fn register(&self, tree: Arc<ProcessTree>) -> KillRegistration<'_> {
        {
            let mut guard = self.kill.lock().unwrap_or_else(|error| error.into_inner());
            *guard = Some(KillHandle { tree: tree.clone() });
        }
        // A pre-publication stop is visible here; a later stop sees the handle.
        if self.has_failure()
            && let Err(error) = tree.terminate()
        {
            self.record_cleanup(CommandOperation::TerminateTree, error);
        }
        KillRegistration {
            control: self,
            tree,
        }
    }

    fn has_failure(&self) -> bool {
        self.failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .primary
            .is_some()
    }

    fn record_cause(&self, cause: CommandFailureCause, terminate: bool) {
        let accepted = {
            let mut state = self
                .failure
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.finalized
                || state.completion_accepted
                    && matches!(cause, CommandFailureCause::Cancelled { .. })
            {
                false
            } else {
                Self::insert_cause(&mut state, cause);
                true
            }
        };
        if terminate && accepted {
            self.terminate_registered();
        }
    }

    fn insert_cause(state: &mut FailureState, cause: CommandFailureCause) {
        match state.primary.as_ref() {
            Some(current) if !cause.precedes(current) => {
                if let CommandFailureCause::Process {
                    operation,
                    kind,
                    message,
                    ..
                } = cause
                {
                    state.cleanup.push(CommandCleanupFailure {
                        operation,
                        kind,
                        message,
                    });
                }
            }
            Some(_) => {
                let previous = state.primary.replace(cause);
                if let Some(CommandFailureCause::Process {
                    operation,
                    kind,
                    message,
                    ..
                }) = previous
                {
                    state.cleanup.push(CommandCleanupFailure {
                        operation,
                        kind,
                        message,
                    });
                }
            }
            None => state.primary = Some(cause),
        }
    }

    fn terminate_registered(&self) {
        let result = self
            .kill
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(KillHandle::kill);
        if let Some(Err(error)) = result {
            self.record_cleanup(CommandOperation::TerminateTree, error);
        }
    }

    fn record_io(&self, operation: CommandOperation, error: std::io::Error, terminate: bool) {
        self.record_cause(
            CommandFailureCause::Process {
                at: Instant::now(),
                operation,
                kind: error.kind(),
                message: error.to_string(),
            },
            terminate,
        );
    }

    fn record_cleanup(&self, operation: CommandOperation, error: std::io::Error) {
        let mut state = self
            .failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.primary.is_none() {
            state.primary = Some(CommandFailureCause::Process {
                at: Instant::now(),
                operation,
                kind: error.kind(),
                message: error.to_string(),
            });
        } else {
            state.cleanup.push(CommandCleanupFailure {
                operation,
                kind: error.kind(),
                message: error.to_string(),
            });
        }
    }

    fn append_cleanup(&self, operation: CommandOperation, error: std::io::Error) {
        self.failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cleanup
            .push(CommandCleanupFailure {
                operation,
                kind: error.kind(),
                message: error.to_string(),
            });
    }

    fn latch_deadline(&self, deadline: Option<Instant>, now: Instant) {
        if let Some(deadline) = deadline
            && now >= deadline
        {
            self.record_cause(CommandFailureCause::DeadlineExceeded { deadline }, true);
        }
    }

    fn accept_completion(&self, deadline: Option<Instant>, observed_at: Instant) -> bool {
        let mut state = self
            .failure
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(deadline) = deadline
            && observed_at >= deadline
        {
            Self::insert_cause(
                &mut state,
                CommandFailureCause::DeadlineExceeded { deadline },
            );
        }
        if state.primary.is_some() {
            return false;
        }
        state.completion_accepted = true;
        true
    }

    fn finalize_success(
        &self,
        deadline: Option<Instant>,
        observed_at: Instant,
    ) -> Result<(), CommandFailure> {
        let mut state = self
            .failure
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(deadline) = deadline
            && observed_at >= deadline
        {
            Self::insert_cause(
                &mut state,
                CommandFailureCause::DeadlineExceeded { deadline },
            );
        }
        if let Some(primary) = state.primary.clone() {
            return Err(CommandFailure {
                primary,
                cleanup: state.cleanup.clone(),
            });
        }
        state.completion_accepted = true;
        state.finalized = true;
        Ok(())
    }

    fn failure(&self) -> Option<CommandFailure> {
        let state = self
            .failure
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.primary.clone().map(|primary| CommandFailure {
            primary,
            cleanup: state.cleanup.clone(),
        })
    }
}

struct Job {
    spec: CommandSpec,
    ctl: Arc<JobControl>,
    result_tx: SyncSender<Result<Output, CommandFailure>>,
}

/// Handle to a submitted command. Block on [`wait`](Self::wait) to get the
/// output, or [`cancel`](Self::cancel) to kill it.
pub struct CommandHandle {
    rx: Receiver<Result<Output, CommandFailure>>,
    ctl: Arc<JobControl>,
}

/// Cloneable cancellation capability for one submitted command.
#[derive(Clone)]
pub struct CommandCancellationHandle {
    ctl: Arc<JobControl>,
}

impl fmt::Debug for CommandCancellationHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CommandCancellationHandle")
    }
}

impl CommandCancellationHandle {
    pub fn cancel(&self) {
        self.ctl.cancel();
    }
}

impl CommandHandle {
    /// Block until the command finishes, returning its captured output. Returns
    /// an `Other` error if the bus worker died, or `Interrupted` if cancelled.
    pub fn wait(self) -> std::io::Result<Output> {
        self.wait_detailed().map_err(CommandFailure::into_io_error)
    }

    /// Block until completion while preserving the typed primary and cleanup causes.
    pub fn wait_detailed(self) -> Result<Output, CommandFailure> {
        self.rx.recv().unwrap_or_else(|_| {
            Err(CommandFailure {
                primary: CommandFailureCause::Process {
                    at: Instant::now(),
                    operation: CommandOperation::Worker,
                    kind: std::io::ErrorKind::Other,
                    message: "command bus worker dropped result".to_string(),
                },
                cleanup: Vec::new(),
            })
        })
    }

    /// Request cancellation: kills the child if it is already running, or
    /// prevents it from starting if still queued.
    pub fn cancel(&self) {
        self.ctl.cancel();
    }

    /// Obtain a cloneable cancellation capability without moving the result receiver.
    pub fn cancellation_handle(&self) -> CommandCancellationHandle {
        CommandCancellationHandle {
            ctl: self.ctl.clone(),
        }
    }
}

/// FIFO work queue shared by one lane's workers.
struct LaneQueue {
    queue: Mutex<VecDeque<Job>>,
    cv: Condvar,
}

impl LaneQueue {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
        }
    }

    fn push(&self, job: Job) {
        if let Ok(mut q) = self.queue.lock() {
            q.push_back(job);
            self.cv.notify_one();
        }
    }

    fn pop(&self) -> Job {
        // Poison-tolerant: a panicking job poisons the mutex, but the queue
        // itself is still valid, so recover the guard and keep serving.
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(job) = q.pop_front() {
                return job;
            }
            q = self.cv.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Optional test interceptor: when installed, the bus returns its result
/// instead of spawning a real process. See [`super::testing`].
type MockFn = Box<dyn Fn(&CommandSpec) -> std::io::Result<Output> + Send + Sync>;

pub struct CommandBus {
    lanes: [Arc<LaneQueue>; LANE_COUNT],
    scopes: Mutex<HashMap<u64, Vec<Weak<JobControl>>>>,
    mock: Mutex<Option<MockFn>>,
}

static BUS: OnceLock<CommandBus> = OnceLock::new();

impl CommandBus {
    /// The process-global bus. Lazily spins up its worker threads on first use.
    pub fn global() -> &'static CommandBus {
        BUS.get_or_init(CommandBus::start)
    }

    fn start() -> CommandBus {
        let lanes: [Arc<LaneQueue>; LANE_COUNT] =
            std::array::from_fn(|_| Arc::new(LaneQueue::new()));

        for lane in [Lane::Interactive, Lane::Poll, Lane::Long] {
            let queue = lanes[lane.index()].clone();
            for n in 0..lane.workers() {
                let queue = queue.clone();
                if let Err(e) = std::thread::Builder::new()
                    .name(format!("okena-cmd-{}-{n}", lane.name()))
                    .spawn(move || worker_loop(&queue))
                {
                    log::error!("failed to spawn command bus worker: {e}");
                }
            }
        }

        CommandBus {
            lanes,
            scopes: Mutex::new(HashMap::new()),
            mock: Mutex::new(None),
        }
    }

    /// Submit a command. Returns immediately with a handle; the command runs on
    /// a bus worker as soon as a permit in its lane is free.
    pub fn submit(&self, spec: CommandSpec) -> CommandHandle {
        let ctl = JobControl::new();

        if let Some(scope) = spec.scope
            && let Ok(mut scopes) = self.scopes.lock()
        {
            let controls = scopes.entry(scope).or_default();
            // Prune handles for jobs that have already completed (their JobControl
            // Arc was dropped). Without this the vec — and the scope key — would
            // grow unbounded for a long-lived scope that's never cancelled.
            controls.retain(|w| w.strong_count() > 0);
            controls.push(Arc::downgrade(&ctl));
        }

        // Bounded oneshot: capacity 1 so the worker never blocks delivering the
        // result even if the handle was dropped.
        let (tx, rx) = std::sync::mpsc::sync_channel(1);

        // Test fast-path: resolve synchronously against the installed mock.
        if let Ok(guard) = self.mock.lock()
            && let Some(mock) = guard.as_ref()
        {
            let result = mock(&spec).map_err(|error| CommandFailure {
                primary: CommandFailureCause::Process {
                    at: Instant::now(),
                    operation: CommandOperation::Mock,
                    kind: error.kind(),
                    message: error.to_string(),
                },
                cleanup: Vec::new(),
            });
            let _ = tx.send(result);
            return CommandHandle { rx, ctl };
        }

        let lane = spec.lane;
        self.lanes[lane.index()].push(Job {
            spec,
            ctl: ctl.clone(),
            result_tx: tx,
        });

        CommandHandle { rx, ctl }
    }

    /// Kill every in-flight (or queued) command tagged with `scope`.
    pub fn cancel_scope(&self, scope: u64) {
        let Ok(mut scopes) = self.scopes.lock() else {
            return;
        };
        if let Some(controls) = scopes.remove(&scope) {
            for weak in controls {
                if let Some(ctl) = weak.upgrade() {
                    ctl.cancel();
                }
            }
        }
    }

    /// Install a test interceptor. See [`super::testing::mock`].
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn set_mock(&self, mock: Option<MockFn>) {
        if let Ok(mut guard) = self.mock.lock() {
            *guard = mock;
        }
    }
}

fn worker_loop(queue: &LaneQueue) {
    loop {
        let job = queue.pop();
        let Job {
            spec,
            ctl,
            result_tx,
        } = job;
        let result = run_job(&spec, &ctl);
        let _ = result_tx.send(result);
    }
}

/// Completion-poll backoff: start tight so short commands (`pgrep`, `git
/// rev-parse`) are detected within a couple ms even when called from a
/// latency-sensitive UI handler, then back off so long commands don't spin.
const POLL_MIN: Duration = Duration::from_millis(1);
const POLL_MAX: Duration = Duration::from_millis(20);
/// Give reader threads a brief chance to collect bytes already in the pipes
/// before treating open pipe handles as descendants left behind by the parent.
const POST_EXIT_DRAIN: Duration = Duration::from_millis(100);
/// Never let an inherited pipe held by an escaped descendant pin a bus lane.
const READER_JOIN_TIMEOUT: Duration = Duration::from_millis(250);

fn run_job(spec: &CommandSpec, ctl: &Arc<JobControl>) -> Result<Output, CommandFailure> {
    ctl.latch_deadline(spec.deadline, Instant::now());
    if let Some(failure) = ctl.failure() {
        return Err(failure);
    }
    if let Err(error) = validate_relative_timeout(spec.timeout, Instant::now()) {
        ctl.record_io(CommandOperation::ComputeDeadline, error, false);
        return Err(ctl.failure().unwrap_or_else(|| CommandFailure {
            primary: CommandFailureCause::Process {
                at: Instant::now(),
                operation: CommandOperation::ComputeDeadline,
                kind: std::io::ErrorKind::InvalidInput,
                message: "relative command timeout validation failed".to_string(),
            },
            cleanup: Vec::new(),
        }));
    }
    let started = Instant::now();
    let detail = spec.audit_detail();
    log::trace!(target: "okena::cmd", "[{}] start {}", spec.lane.name(), detail);

    // Catch the rare EBADF panic from std's pipe reader under FD pressure and
    // turn it into a normal error (preserves the old `safe_output` guarantee).
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        spawn_and_collect(spec, ctl)
    }))
    .unwrap_or_else(|panic| {
        let message = panic_message(&panic);
        log::error!(target: "okena::cmd", "[{}] {} panicked: {message}", spec.lane.name(), detail);
        ctl.record_io(
            CommandOperation::Worker,
            std::io::Error::other(format!("command panicked: {message}")),
            true,
        );
        Err(ctl.failure().unwrap_or_else(|| CommandFailure {
            primary: CommandFailureCause::Process {
                at: Instant::now(),
                operation: CommandOperation::Worker,
                kind: std::io::ErrorKind::Other,
                message,
            },
            cleanup: Vec::new(),
        }))
    });

    let elapsed = started.elapsed().as_millis();
    match &result {
        Ok(out) => log::debug!(
            target: "okena::cmd",
            "[{}] {} -> {} ({elapsed}ms)",
            spec.lane.name(), detail,
            out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
        ),
        Err(error) => log::warn!(
            target: "okena::cmd",
            "[{}] {} failed: {error} ({elapsed}ms)", spec.lane.name(), detail,
        ),
    }
    result
}

/// Spawn the child with piped stdio and poll until it exits, the deadline
/// passes, or cancellation is requested.
fn spawn_and_collect(spec: &CommandSpec, ctl: &Arc<JobControl>) -> Result<Output, CommandFailure> {
    let mut cmd = spec.build();
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let (mut child, tree) = match ProcessTree::spawn(&mut cmd, ctl) {
        Ok(process) => process,
        Err(error) => {
            ctl.record_io(CommandOperation::Spawn, error, false);
            return Err(ctl.failure().unwrap_or_else(|| CommandFailure {
                primary: CommandFailureCause::Process {
                    at: Instant::now(),
                    operation: CommandOperation::Spawn,
                    kind: std::io::ErrorKind::Other,
                    message: "process spawn failed without error evidence".to_string(),
                },
                cleanup: Vec::new(),
            }));
        }
    };

    // Publish ownership before creating readers. A cancellation that raced
    // with spawn either sees this handle or is observed by register itself.
    let _registration = ctl.register(tree.clone());
    let mut deadline = spec.deadline;
    let mut out_reader = None;
    let mut err_reader = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        deadline = match effective_deadline(spec.deadline, spec.timeout, Instant::now()) {
            Ok(deadline) => deadline,
            Err(error) => {
                ctl.record_io(CommandOperation::ComputeDeadline, error, true);
                return finish_child(&tree, &mut child, None, None, ctl, spec.deadline, false);
            }
        };

        // Drain both pipes while polling. A child can otherwise block after
        // filling an OS pipe buffer before the parent observes its exit.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        if stdout.is_none() {
            ctl.record_io(
                CommandOperation::SpawnStdoutReader,
                std::io::Error::other("stdout pipe was not created"),
                true,
            );
        }
        if stderr.is_none() {
            ctl.record_io(
                CommandOperation::SpawnStderrReader,
                std::io::Error::other("stderr pipe was not created"),
                true,
            );
        }
        let stdout_limit = spec.output_limits.map(OutputLimits::stdout_bytes);
        let stderr_limit = spec.output_limits.map(OutputLimits::stderr_bytes);
        out_reader = stdout.and_then(|pipe| {
            match spawn_pipe_reader(pipe, OutputStream::Stdout, stdout_limit, ctl.clone()) {
                Ok(reader) => Some(reader),
                Err(error) => {
                    ctl.record_io(CommandOperation::SpawnStdoutReader, error, true);
                    None
                }
            }
        });
        err_reader = stderr.and_then(|pipe| {
            match spawn_pipe_reader(pipe, OutputStream::Stderr, stderr_limit, ctl.clone()) {
                Ok(reader) => Some(reader),
                Err(error) => {
                    ctl.record_io(CommandOperation::SpawnStderrReader, error, true);
                    None
                }
            }
        });

        maybe_panic_after_spawn(spec);
        drive_child(
            &tree,
            &mut child,
            &mut out_reader,
            &mut err_reader,
            ctl,
            deadline,
        )
    }));
    match result {
        Ok(result) => result,
        Err(panic) => {
            let message = panic_message(&panic);
            ctl.record_io(
                CommandOperation::Worker,
                std::io::Error::other(format!("command panicked after spawn: {message}")),
                true,
            );
            finish_child(
                &tree,
                &mut child,
                out_reader.take(),
                err_reader.take(),
                ctl,
                deadline,
                false,
            )
        }
    }
}

fn effective_deadline(
    absolute: Option<Instant>,
    relative_timeout: Option<Duration>,
    relative_start: Instant,
) -> std::io::Result<Option<Instant>> {
    let relative = relative_timeout
        .map(|timeout| checked_relative_deadline(relative_start, timeout))
        .transpose()?;
    Ok(match (absolute, relative) {
        (Some(absolute), Some(relative)) => Some(absolute.min(relative)),
        (Some(absolute), None) => Some(absolute),
        (None, relative) => relative,
    })
}

fn validate_relative_timeout(timeout: Option<Duration>, at: Instant) -> std::io::Result<()> {
    timeout
        .map(|timeout| checked_relative_deadline(at, timeout).map(|_| ()))
        .transpose()
        .map(|_| ())
}

fn checked_relative_deadline(start: Instant, timeout: Duration) -> std::io::Result<Instant> {
    start.checked_add(timeout).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "relative command timeout exceeds the supported Instant range",
        )
    })
}

fn drive_child(
    tree: &ProcessTree,
    child: &mut std::process::Child,
    stdout_reader: &mut Option<PipeReader>,
    stderr_reader: &mut Option<PipeReader>,
    ctl: &JobControl,
    deadline: Option<Instant>,
) -> Result<Output, CommandFailure> {
    let mut backoff = POLL_MIN;
    loop {
        ctl.latch_deadline(deadline, Instant::now());
        if ctl.has_failure() {
            return finish_child(
                tree,
                child,
                stdout_reader.take(),
                stderr_reader.take(),
                ctl,
                deadline,
                false,
            );
        }

        let exited = match process_exited(child) {
            Ok(exited) => exited,
            Err(error) => {
                ctl.record_io(CommandOperation::Poll, error, true);
                return finish_child(
                    tree,
                    child,
                    stdout_reader.take(),
                    stderr_reader.take(),
                    ctl,
                    deadline,
                    false,
                );
            }
        };
        if exited {
            let observed_at = Instant::now();
            if !ctl.accept_completion(deadline, observed_at) {
                return finish_child(
                    tree,
                    child,
                    stdout_reader.take(),
                    stderr_reader.take(),
                    ctl,
                    deadline,
                    false,
                );
            }
            // A direct parent may exit while a background descendant still
            // owns inherited pipes. Drain briefly, then terminate the tree.
            wait_for_readers(stdout_reader, stderr_reader, POST_EXIT_DRAIN);
            return finish_child(
                tree,
                child,
                stdout_reader.take(),
                stderr_reader.take(),
                ctl,
                deadline,
                true,
            );
        }

        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(POLL_MAX);
    }
}

fn finish_child(
    tree: &ProcessTree,
    child: &mut std::process::Child,
    stdout_reader: Option<PipeReader>,
    stderr_reader: Option<PipeReader>,
    ctl: &JobControl,
    deadline: Option<Instant>,
    parent_exited: bool,
) -> Result<Output, CommandFailure> {
    if let Err(error) = tree.terminate() {
        ctl.record_cleanup(CommandOperation::TerminateTree, error);
    }

    let status = if parent_exited {
        match child.wait() {
            Ok(status) => Some(status),
            Err(error) => {
                ctl.record_cleanup(CommandOperation::WaitChild, error);
                None
            }
        }
    } else {
        match child.try_wait() {
            Ok(Some(status)) => Some(status),
            Ok(None) => {
                if let Err(error) = child.kill() {
                    ctl.record_cleanup(CommandOperation::KillChild, error);
                }
                match child.wait() {
                    Ok(status) => Some(status),
                    Err(error) => {
                        ctl.record_cleanup(CommandOperation::WaitChild, error);
                        None
                    }
                }
            }
            Err(error) => {
                ctl.record_cleanup(CommandOperation::Poll, error);
                if let Err(error) = child.kill() {
                    ctl.record_cleanup(CommandOperation::KillChild, error);
                }
                match child.wait() {
                    Ok(status) => Some(status),
                    Err(error) => {
                        ctl.record_cleanup(CommandOperation::WaitChild, error);
                        None
                    }
                }
            }
        }
    };

    let stdout = join_pipe_reader(stdout_reader, OutputStream::Stdout, ctl);
    let stderr = join_pipe_reader(stderr_reader, OutputStream::Stderr, ctl);

    if parent_exited {
        ctl.finalize_success(deadline, Instant::now())?;
    } else {
        ctl.latch_deadline(deadline, Instant::now());
        if let Some(failure) = ctl.failure() {
            return Err(failure);
        }
    }
    let status = status.ok_or_else(|| CommandFailure {
        primary: CommandFailureCause::Process {
            at: Instant::now(),
            operation: CommandOperation::WaitChild,
            kind: std::io::ErrorKind::Other,
            message: "child status was unavailable".to_string(),
        },
        cleanup: Vec::new(),
    })?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Detect exit without reaping on supported Unix hosts. Keeping the group
/// leader as a zombie until tree cleanup prevents its process-group ID from
/// being reused for an unrelated process before the group signal is sent.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn process_exited(child: &mut std::process::Child) -> std::io::Result<bool> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: info points to writable siginfo storage. WNOWAIT observes only
    // this owned child and deliberately leaves it waitable by Child::wait.
    if unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: waitid initialized the siginfo structure on success; si_pid is
    // zero for the WNOHANG case where the child has not exited.
    Ok(unsafe { info.assume_init().si_pid() } != 0)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_exited(child: &mut std::process::Child) -> std::io::Result<bool> {
    child.try_wait().map(|status| status.is_some())
}

#[derive(Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

struct PipeReader {
    handle: std::thread::JoinHandle<()>,
    captured: Arc<Mutex<Vec<u8>>>,
    discard: Arc<AtomicBool>,
}

/// Drain one pipe concurrently. Once the first byte beyond the limit arrives,
/// retain no more data, latch the event, and keep draining until tree cleanup.
fn spawn_pipe_reader<R: Read + Send + 'static>(
    mut pipe: R,
    stream: OutputStream,
    limit: Option<NonZeroU64>,
    ctl: Arc<JobControl>,
) -> std::io::Result<PipeReader> {
    let name = match stream {
        OutputStream::Stdout => "okena-cmd-stdout",
        OutputStream::Stderr => "okena-cmd-stderr",
    };
    let captured = Arc::new(Mutex::new(Vec::new()));
    let reader_capture = captured.clone();
    let discard = Arc::new(AtomicBool::new(false));
    let reader_discard = discard.clone();
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let mut observed = 0_u64;
            let mut overflowed = false;
            let mut chunk = [0_u8; 8192];
            loop {
                let count = match pipe.read(&mut chunk) {
                    Ok(0) => return,
                    Ok(count) => count,
                    Err(error) => {
                        let operation = match stream {
                            OutputStream::Stdout => CommandOperation::ReadStdout,
                            OutputStream::Stderr => CommandOperation::ReadStderr,
                        };
                        ctl.record_io(operation, error, true);
                        return;
                    }
                };
                let previous = observed;
                observed = observed.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
                if let Some(limit) = limit {
                    let limit = limit.get();
                    if previous < limit {
                        let retained = usize::try_from(limit - previous)
                            .unwrap_or(usize::MAX)
                            .min(count);
                        let mut capture = reader_capture
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if !reader_discard.load(Ordering::Acquire) {
                            capture.extend_from_slice(&chunk[..retained]);
                        }
                    }
                    if !overflowed && observed > limit {
                        overflowed = true;
                        let at = Instant::now();
                        let cause = match stream {
                            OutputStream::Stdout => CommandFailureCause::StdoutLimitExceeded {
                                at,
                                limit,
                                observed: limit.saturating_add(1),
                            },
                            OutputStream::Stderr => CommandFailureCause::StderrLimitExceeded {
                                at,
                                limit,
                                observed: limit.saturating_add(1),
                            },
                        };
                        ctl.record_cause(cause, true);
                    }
                } else {
                    let mut capture = reader_capture
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if !reader_discard.load(Ordering::Acquire) {
                        capture.extend_from_slice(&chunk[..count]);
                    }
                }
            }
        })
        .map(|handle| PipeReader {
            handle,
            captured,
            discard,
        })
}

fn wait_for_readers(stdout: &Option<PipeReader>, stderr: &Option<PipeReader>, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !(reader_finished(stdout) && reader_finished(stderr)) {
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(POLL_MIN);
    }
}

fn reader_finished(reader: &Option<PipeReader>) -> bool {
    reader
        .as_ref()
        .is_none_or(|reader| reader.handle.is_finished())
}

fn join_pipe_reader(reader: Option<PipeReader>, stream: OutputStream, ctl: &JobControl) -> Vec<u8> {
    let Some(reader) = reader else {
        return Vec::new();
    };
    let operation = match stream {
        OutputStream::Stdout => CommandOperation::JoinStdoutReader,
        OutputStream::Stderr => CommandOperation::JoinStderrReader,
    };
    let deadline = Instant::now() + READER_JOIN_TIMEOUT;
    while !reader.handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(POLL_MIN);
    }
    if reader.handle.is_finished() {
        if reader.handle.join().is_err() {
            ctl.record_cleanup(operation, std::io::Error::other("output reader panicked"));
        }
    } else {
        reader.discard.store(true, Ordering::Release);
        ctl.record_cleanup(
            operation,
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "output reader did not stop after process-tree cleanup; detached",
            ),
        );
    }
    reader
        .captured
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

#[cfg(test)]
fn maybe_panic_after_spawn(spec: &CommandSpec) {
    if spec
        .env
        .iter()
        .any(|(key, value)| key == "OKENA_TEST_PANIC_AFTER_SPAWN" && value == "1")
    {
        std::thread::sleep(Duration::from_millis(50));
        panic!("injected post-spawn panic");
    }
}

#[cfg(not(test))]
fn maybe_panic_after_spawn(_spec: &CommandSpec) {}

#[cfg(unix)]
struct ProcessTree {
    process_group: libc::pid_t,
    terminated: Mutex<bool>,
}

#[cfg(unix)]
impl ProcessTree {
    fn spawn(
        cmd: &mut std::process::Command,
        ctl: &JobControl,
    ) -> std::io::Result<(std::process::Child, Arc<Self>)> {
        use std::os::unix::process::CommandExt;

        cmd.process_group(0);
        let mut child = cmd.spawn()?;
        let process_group = match libc::pid_t::try_from(child.id()) {
            Ok(process_group) if process_group > 0 => process_group,
            _ => {
                let error = std::io::Error::other("spawned process has an invalid process group");
                if let Err(cleanup) = child.kill() {
                    ctl.append_cleanup(CommandOperation::KillChild, cleanup);
                }
                if let Err(cleanup) = child.wait() {
                    ctl.append_cleanup(CommandOperation::WaitChild, cleanup);
                }
                return Err(error);
            }
        };
        Ok((
            child,
            Arc::new(Self {
                process_group,
                terminated: Mutex::new(false),
            }),
        ))
    }

    fn terminate(&self) -> std::io::Result<()> {
        let mut terminated = self
            .terminated
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *terminated {
            return Ok(());
        }
        let Some(group_target) = self.process_group.checked_neg() else {
            return Err(std::io::Error::other("invalid process group"));
        };
        // SAFETY: process_group is the positive PID returned for a child that
        // was atomically placed in its own group before exec. Registration is
        // cleared before this identity can be reused by an unrelated process.
        if unsafe { libc::kill(group_target, libc::SIGKILL) } == 0 {
            *terminated = true;
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            *terminated = true;
            Ok(())
        } else {
            Err(error)
        }
    }
}

#[cfg(windows)]
struct ProcessTree {
    job: std::os::windows::io::OwnedHandle,
    terminated: Mutex<bool>,
}

#[cfg(windows)]
impl ProcessTree {
    fn spawn(
        cmd: &mut std::process::Command,
        ctl: &JobControl,
    ) -> std::io::Result<(std::process::Child, Arc<Self>)> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        // Assign the process before any user code can create descendants that
        // would escape the job. This repeats CREATE_NO_WINDOW from command().
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);

        // SAFETY: null security/name pointers request a private unnamed job.
        let raw_job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw_job.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: CreateJobObjectW returned a unique owned handle on success.
        let job = unsafe { OwnedHandle::from_raw_handle(raw_job) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let limits_size = u32::try_from(std::mem::size_of_val(&limits))
            .map_err(|_| std::io::Error::other("job limits structure is too large"))?;
        // SAFETY: limits points to the structure required by this information
        // class and remains valid for the duration of the call.
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                limits_size,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }

        let mut child = cmd.spawn()?;
        // SAFETY: both handles are live and owned for the duration of the call.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
            let error = std::io::Error::last_os_error();
            if let Err(cleanup) = child.kill() {
                ctl.append_cleanup(CommandOperation::KillChild, cleanup);
            }
            if let Err(cleanup) = child.wait() {
                ctl.append_cleanup(CommandOperation::WaitChild, cleanup);
            }
            return Err(error);
        }
        if let Err(error) = resume_suspended_process(child.id()) {
            if let Err(cleanup) = child.kill() {
                ctl.append_cleanup(CommandOperation::KillChild, cleanup);
            }
            if let Err(cleanup) = child.wait() {
                ctl.append_cleanup(CommandOperation::WaitChild, cleanup);
            }
            return Err(error);
        }

        Ok((
            child,
            Arc::new(Self {
                job,
                terminated: Mutex::new(false),
            }),
        ))
    }

    fn terminate(&self) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        let mut terminated = self
            .terminated
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *terminated {
            return Ok(());
        }
        // SAFETY: the owned job handle remains live for this call.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } != 0 {
            *terminated = true;
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[cfg(windows)]
fn resume_suspended_process(process_id: u32) -> std::io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    // SAFETY: the snapshot call has no borrowed pointer arguments.
    let raw_snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw_snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful snapshot call returned a unique owned handle.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw_snapshot) };
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(std::mem::size_of::<THREADENTRY32>())
            .map_err(|_| std::io::Error::other("thread entry structure is too large"))?,
        ..THREADENTRY32::default()
    };

    // SAFETY: entry is initialized with the required size and remains live.
    let mut has_entry = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
    while has_entry {
        if entry.th32OwnerProcessID == process_id {
            // SAFETY: the enumerated thread ID belongs to the suspended child.
            let raw_thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if raw_thread.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: OpenThread returned a unique owned handle on success.
            let thread = unsafe { OwnedHandle::from_raw_handle(raw_thread) };
            // SAFETY: the thread handle grants THREAD_SUSPEND_RESUME access.
            if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                return Err(std::io::Error::last_os_error());
            }
            return Ok(());
        }
        // SAFETY: entry and snapshot remain valid across enumeration calls.
        has_entry = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "suspended process thread was not found",
    ))
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenReader;

    impl Read for BrokenReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("synthetic read failure"))
        }
    }

    struct BlockingReader {
        released: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Read for BlockingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            let (released, cv) = &*self.released;
            let mut released = released.lock().unwrap_or_else(|error| error.into_inner());
            while !*released {
                released = cv.wait(released).unwrap_or_else(|error| error.into_inner());
            }
            Ok(0)
        }
    }

    fn stdout(at: Instant) -> CommandFailureCause {
        CommandFailureCause::StdoutLimitExceeded {
            at,
            limit: 10,
            observed: 11,
        }
    }

    fn stderr(at: Instant) -> CommandFailureCause {
        CommandFailureCause::StderrLimitExceeded {
            at,
            limit: 10,
            observed: 11,
        }
    }

    #[test]
    fn first_cause_uses_time_then_deterministic_tie_priority() {
        let at = Instant::now();
        let ctl = JobControl::new();
        ctl.record_cause(stderr(at), false);
        ctl.record_cause(stdout(at), false);
        ctl.record_cause(CommandFailureCause::Cancelled { at }, false);
        ctl.record_cause(
            CommandFailureCause::DeadlineExceeded { deadline: at },
            false,
        );

        assert!(matches!(
            ctl.failure().unwrap().primary,
            CommandFailureCause::DeadlineExceeded { .. }
        ));
    }

    #[test]
    fn earlier_cause_replaces_later_but_later_cause_never_replaces_first() {
        let base = Instant::now();
        let early = base + Duration::from_millis(1);
        let late = base + Duration::from_millis(2);
        let ctl = JobControl::new();
        ctl.record_cause(stderr(late), false);
        ctl.record_cause(stdout(early), false);
        ctl.record_cause(CommandFailureCause::Cancelled { at: late }, false);

        assert!(matches!(
            ctl.failure().unwrap().primary,
            CommandFailureCause::StdoutLimitExceeded { .. }
        ));
    }

    #[test]
    fn cleanup_failure_does_not_replace_primary() {
        let ctl = JobControl::new();
        ctl.record_cause(CommandFailureCause::Cancelled { at: Instant::now() }, false);
        ctl.record_cleanup(
            CommandOperation::WaitChild,
            std::io::Error::other("synthetic cleanup failure"),
        );

        let failure = ctl.failure().unwrap();
        assert!(matches!(
            failure.primary,
            CommandFailureCause::Cancelled { .. }
        ));
        assert_eq!(failure.cleanup.len(), 1);
        assert_eq!(failure.cleanup[0].operation, CommandOperation::WaitChild);
    }

    #[test]
    fn reader_cleanup_evidence_is_attached_to_existing_primary() {
        let ctl = JobControl::new();
        ctl.record_cause(CommandFailureCause::Cancelled { at: Instant::now() }, false);
        let reader = spawn_pipe_reader(BrokenReader, OutputStream::Stdout, None, ctl.clone())
            .expect("reader thread");
        let _ = join_pipe_reader(Some(reader), OutputStream::Stdout, &ctl);

        let failure = ctl.failure().unwrap();
        assert!(matches!(
            failure.primary,
            CommandFailureCause::Cancelled { .. }
        ));
        assert!(
            failure
                .cleanup
                .iter()
                .any(|evidence| evidence.operation == CommandOperation::ReadStdout)
        );
    }

    #[test]
    fn stuck_reader_is_detached_with_cleanup_evidence() {
        let ctl = JobControl::new();
        ctl.record_cause(CommandFailureCause::Cancelled { at: Instant::now() }, false);
        let released = Arc::new((Mutex::new(false), Condvar::new()));
        let reader = spawn_pipe_reader(
            BlockingReader {
                released: released.clone(),
            },
            OutputStream::Stdout,
            None,
            ctl.clone(),
        )
        .expect("reader thread");

        let started = Instant::now();
        let _ = join_pipe_reader(Some(reader), OutputStream::Stdout, &ctl);
        let elapsed = started.elapsed();
        let (flag, cv) = &*released;
        *flag.lock().unwrap_or_else(|error| error.into_inner()) = true;
        cv.notify_all();

        assert!(elapsed < Duration::from_secs(1), "elapsed: {elapsed:?}");
        assert!(ctl.failure().unwrap().cleanup.iter().any(|evidence| {
            evidence.operation == CommandOperation::JoinStdoutReader
                && evidence.kind == std::io::ErrorKind::TimedOut
        }));
    }

    #[cfg(unix)]
    #[test]
    fn termination_failure_is_cleanup_evidence_not_a_new_primary() {
        let ctl = JobControl::new();
        ctl.record_cause(CommandFailureCause::Cancelled { at: Instant::now() }, false);
        let tree = ProcessTree {
            process_group: libc::pid_t::MIN,
            terminated: Mutex::new(false),
        };
        let error = tree.terminate().expect_err("invalid process group");
        ctl.record_cleanup(CommandOperation::TerminateTree, error);

        let failure = ctl.failure().unwrap();
        assert!(matches!(
            failure.primary,
            CommandFailureCause::Cancelled { .. }
        ));
        assert!(
            failure
                .cleanup
                .iter()
                .any(|evidence| evidence.operation == CommandOperation::TerminateTree)
        );
    }

    #[test]
    fn cancellation_before_observed_completion_wins() {
        let ctl = JobControl::new();
        ctl.cancel();
        assert!(!ctl.accept_completion(None, Instant::now()));
        assert!(matches!(
            ctl.failure().unwrap().primary,
            CommandFailureCause::Cancelled { .. }
        ));
    }

    #[test]
    fn cancellation_after_accepted_completion_cannot_replace_success() {
        let ctl = JobControl::new();
        assert!(ctl.accept_completion(None, Instant::now()));
        ctl.cancel();
        assert!(ctl.finalize_success(None, Instant::now()).is_ok());
        assert!(ctl.failure().is_none());
    }

    #[test]
    fn overflow_during_final_drain_beats_accepted_completion() {
        let ctl = JobControl::new();
        assert!(ctl.accept_completion(None, Instant::now()));
        ctl.record_cause(stdout(Instant::now()), false);
        let failure = ctl
            .finalize_success(None, Instant::now())
            .expect_err("final-drain overflow");
        assert!(matches!(
            failure.primary,
            CommandFailureCause::StdoutLimitExceeded { .. }
        ));
    }

    #[test]
    fn deadline_is_checked_atomically_when_completion_is_observed() {
        let ctl = JobControl::new();
        let deadline = Instant::now();
        assert!(!ctl.accept_completion(Some(deadline), deadline));
        assert!(matches!(
            ctl.failure().unwrap().primary,
            CommandFailureCause::DeadlineExceeded { .. }
        ));
    }

    #[test]
    fn relative_timeout_overflow_is_rejected() {
        let error = effective_deadline(None, Some(Duration::MAX), Instant::now())
            .expect_err("overflowing timeout");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn escaped_noisy_reader_stops_retaining_after_detach() {
        let pid_file = tempfile::NamedTempFile::new().expect("pid file");
        let pid_path = pid_file.path().to_string_lossy().into_owned();
        let ctl = JobControl::new();
        let spec = CommandSpec::new("/bin/sh").args([
            "-c",
            "setsid /bin/sh -c 'echo $$ > \"$1\"; i=0; while [ \"$i\" -lt 300 ]; do printf 0123456789abcdef; i=$((i + 1)); sleep 0.01; done' okena-escaped \"$1\" &",
            "okena-test",
            &pid_path,
        ]);
        let mut command = spec.build();
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let (mut child, tree) = ProcessTree::spawn(&mut command, &ctl).expect("spawn parent");
        let _registration = ctl.register(tree.clone());
        let mut stdout_reader = Some(
            spawn_pipe_reader(
                child.stdout.take().expect("stdout pipe"),
                OutputStream::Stdout,
                None,
                ctl.clone(),
            )
            .expect("stdout reader"),
        );
        let retained = stdout_reader.as_ref().unwrap().captured.clone();
        let mut stderr_reader = Some(
            spawn_pipe_reader(
                child.stderr.take().expect("stderr pipe"),
                OutputStream::Stderr,
                None,
                ctl.clone(),
            )
            .expect("stderr reader"),
        );

        let exit_deadline = Instant::now() + Duration::from_secs(2);
        while !process_exited(&mut child).expect("poll parent") {
            assert!(Instant::now() < exit_deadline, "parent did not exit");
            std::thread::sleep(POLL_MIN);
        }
        assert!(ctl.accept_completion(None, Instant::now()));
        wait_for_readers(&stdout_reader, &stderr_reader, POST_EXIT_DRAIN);
        let failure = finish_child(
            &tree,
            &mut child,
            stdout_reader.take(),
            stderr_reader.take(),
            &ctl,
            None,
            true,
        )
        .expect_err("escaped readers must detach");

        let length_at_return = retained
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len();
        std::thread::sleep(Duration::from_millis(150));
        let length_later = retained
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len();
        let escaped_pid = wait_for_pid_file(pid_file.path(), Duration::from_secs(1));
        kill_process_group(escaped_pid);

        assert!(matches!(
            failure.primary,
            CommandFailureCause::Process {
                operation: CommandOperation::JoinStdoutReader | CommandOperation::JoinStderrReader,
                kind: std::io::ErrorKind::TimedOut,
                ..
            }
        ));
        assert!(
            length_at_return < 128 * 1024,
            "retained {length_at_return} bytes before detach"
        );
        assert_eq!(
            length_later, length_at_return,
            "detached reader kept retaining output"
        );
    }

    #[cfg(target_os = "linux")]
    fn wait_for_pid_file(path: &std::path::Path, timeout: Duration) -> u32 {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(contents) = std::fs::read_to_string(path)
                && let Ok(pid) = contents.trim().parse()
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "timed out waiting for pid");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(target_os = "linux")]
    fn kill_process_group(pid: u32) {
        let pid = libc::pid_t::try_from(pid).expect("test pid fits pid_t");
        let group = pid.checked_neg().expect("positive test pid");
        // SAFETY: the test created this session and read its group leader PID.
        let _ = unsafe { libc::kill(group, libc::SIGKILL) };
    }
}
