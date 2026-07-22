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
    fn workers(self) -> usize {
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

/// A fully-described one-shot command. Built directly, or extracted from a
/// configured [`std::process::Command`] via [`CommandSpec::from_command`].
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub timeout: Option<Duration>,
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

/// Per-job shared control block. Lets the submitter (and a scope-wide cancel)
/// signal cancellation, and lets the running worker register the live child so
/// it can be killed mid-flight.
struct JobControl {
    cancelled: AtomicBool,
    /// Set by the worker once the process tree is owned; taken to kill it.
    kill: Mutex<Option<KillHandle>>,
}

/// A minimal handle that can kill a spawned process tree from another thread.
struct KillHandle {
    tree: Arc<ProcessTree>,
}

impl KillHandle {
    fn kill(&self) {
        self.tree.terminate();
    }
}

/// Clears the externally reachable kill handle before the owned process-group
/// identifier or job handle can become stale.
struct KillRegistration<'a> {
    control: &'a JobControl,
}

impl Drop for KillRegistration<'_> {
    fn drop(&mut self) {
        let mut guard = self.control.kill.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
}

impl JobControl {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            cancelled: AtomicBool::new(false),
            kill: Mutex::new(None),
        })
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Ok(guard) = self.kill.lock()
            && let Some(handle) = guard.as_ref()
        {
            handle.kill();
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn register(&self, tree: Arc<ProcessTree>) -> KillRegistration<'_> {
        let mut guard = self.kill.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(KillHandle { tree });
        KillRegistration { control: self }
    }
}

struct Job {
    spec: CommandSpec,
    ctl: Arc<JobControl>,
    result_tx: SyncSender<std::io::Result<Output>>,
}

/// Handle to a submitted command. Block on [`wait`](Self::wait) to get the
/// output, or [`cancel`](Self::cancel) to kill it.
pub struct CommandHandle {
    rx: Receiver<std::io::Result<Output>>,
    ctl: Arc<JobControl>,
}

impl CommandHandle {
    /// Block until the command finishes, returning its captured output. Returns
    /// an `Other` error if the bus worker died, or `Interrupted` if cancelled.
    pub fn wait(self) -> std::io::Result<Output> {
        match self.rx.recv() {
            Ok(result) => result,
            Err(_) => Err(std::io::Error::other("command bus worker dropped result")),
        }
    }

    /// Request cancellation: kills the child if it is already running, or
    /// prevents it from starting if still queued.
    pub fn cancel(&self) {
        self.ctl.cancel();
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
            let _ = tx.send(mock(&spec));
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

fn run_job(spec: &CommandSpec, ctl: &Arc<JobControl>) -> std::io::Result<Output> {
    // Cancelled before we even started.
    if ctl.is_cancelled() {
        return Err(cancelled_err());
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
        let msg = panic_message(&panic);
        log::error!(target: "okena::cmd", "[{}] {} panicked: {msg}", spec.lane.name(), detail);
        Err(std::io::Error::other(format!("command panicked: {msg}")))
    });

    let elapsed = started.elapsed().as_millis();
    match &result {
        Ok(out) => log::debug!(
            target: "okena::cmd",
            "[{}] {} -> {} ({elapsed}ms)",
            spec.lane.name(), detail,
            out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
        ),
        Err(e) => log::warn!(
            target: "okena::cmd",
            "[{}] {} failed: {e} ({elapsed}ms)", spec.lane.name(), detail,
        ),
    }
    result
}

/// Spawn the child with piped stdio and poll until it exits, the deadline
/// passes, or cancellation is requested.
fn spawn_and_collect(spec: &CommandSpec, ctl: &Arc<JobControl>) -> std::io::Result<Output> {
    let mut cmd = spec.build();
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let (mut child, tree) = ProcessTree::spawn(&mut cmd)?;

    // Drain stdout/stderr on dedicated threads, concurrently with the wait loop
    // below. Otherwise a child that writes more than the OS pipe buffer (~64KB)
    // blocks on `write` forever while we wait for it to exit and never drain —
    // a classic deadlock, hit by e.g. a large `docker ps -a` or `git diff`.
    let out_reader = spawn_pipe_reader(child.stdout.take());
    let err_reader = spawn_pipe_reader(child.stderr.take());

    // Publish the kill handle so cancel()/cancel_scope() can reach the whole
    // process tree. The registration clears it before the OS identity can be
    // reused for an unrelated process.
    let _registration = ctl.register(tree.clone());
    // Lost a cancellation race between the check above and registering: honor it.
    if ctl.is_cancelled() {
        terminate_and_reap(&tree, &mut child);
        let _ = out_reader.join();
        let _ = err_reader.join();
        return Err(cancelled_err());
    }

    let deadline = spec.timeout.map(|t| Instant::now() + t);
    let mut backoff = POLL_MIN;

    loop {
        // Check cancellation before reaping: a killed child exits, and we must
        // report that as cancelled rather than as a (signal) success.
        if ctl.is_cancelled() {
            terminate_and_reap(&tree, &mut child);
            let _ = out_reader.join();
            let _ = err_reader.join();
            return Err(cancelled_err());
        }

        if process_exited(&mut child)? {
            // A direct parent may exit while a background descendant still
            // owns the inherited pipes. Bound the drain, then terminate the
            // owned tree so joining the readers cannot hang forever.
            let _ = wait_for_readers(&out_reader, &err_reader, POST_EXIT_DRAIN);
            tree.terminate();
            let status = child.wait()?;
            let stdout = out_reader.join().unwrap_or_default();
            let stderr = err_reader.join().unwrap_or_default();
            if ctl.is_cancelled() {
                return Err(cancelled_err());
            }
            return Ok(Output {
                status,
                stdout,
                stderr,
            });
        }

        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            terminate_and_reap(&tree, &mut child);
            let _ = out_reader.join();
            let _ = err_reader.join();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "process timed out",
            ));
        }

        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(POLL_MAX);
    }
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

/// Spawn a thread that reads a child pipe to EOF into a buffer. Reading
/// concurrently with the wait loop prevents a full-pipe write deadlock.
fn spawn_pipe_reader<R: std::io::Read + Send + 'static>(
    pipe: Option<R>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = pipe {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    })
}

fn wait_for_readers(
    stdout: &std::thread::JoinHandle<Vec<u8>>,
    stderr: &std::thread::JoinHandle<Vec<u8>>,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while !(stdout.is_finished() && stderr.is_finished()) {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL_MIN);
    }
    true
}

fn terminate_and_reap(tree: &ProcessTree, child: &mut std::process::Child) {
    tree.terminate();
    // The direct kill is an exact Child handle fallback if platform tree
    // setup succeeded but group/job termination itself was denied.
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
struct ProcessTree {
    process_group: libc::pid_t,
    terminated: AtomicBool,
}

#[cfg(unix)]
impl ProcessTree {
    fn spawn(cmd: &mut std::process::Command) -> std::io::Result<(std::process::Child, Arc<Self>)> {
        use std::os::unix::process::CommandExt;

        cmd.process_group(0);
        let mut child = cmd.spawn()?;
        let process_group = match libc::pid_t::try_from(child.id()) {
            Ok(process_group) if process_group > 0 => process_group,
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::other(
                    "spawned process has an invalid process group",
                ));
            }
        };
        Ok((
            child,
            Arc::new(Self {
                process_group,
                terminated: AtomicBool::new(false),
            }),
        ))
    }

    fn terminate(&self) {
        if self.terminated.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(group_target) = self.process_group.checked_neg() else {
            return;
        };
        // SAFETY: process_group is the positive PID returned for a child that
        // was atomically placed in its own group before exec. Registration is
        // cleared before this identity can be reused by an unrelated process.
        let _ = unsafe { libc::kill(group_target, libc::SIGKILL) };
    }
}

#[cfg(windows)]
struct ProcessTree {
    job: std::os::windows::io::OwnedHandle,
    terminated: AtomicBool,
}

#[cfg(windows)]
impl ProcessTree {
    fn spawn(cmd: &mut std::process::Command) -> std::io::Result<(std::process::Child, Arc<Self>)> {
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
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if let Err(error) = resume_suspended_process(child.id()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }

        Ok((
            child,
            Arc::new(Self {
                job,
                terminated: AtomicBool::new(false),
            }),
        ))
    }

    fn terminate(&self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        if self.terminated.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: the owned job handle remains live for this call.
        let _ = unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) };
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

fn cancelled_err() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "command cancelled")
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
