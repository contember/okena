use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use okena_core::agent_session::is_valid_transcript_path;
use okena_core::agent_status::AgentLifecycle;
use okena_core::claude_remote::{
    ApiClaudeDecision, ApiClaudeDialog, ApiClaudePromptDelivery, ApiClaudeSession,
    ApiClaudeTranscriptPage, ClaudeHookRequest, ClaudeTerminalInfo,
};
use okena_core::keys::SpecialKey;
use parking_lot::Mutex;
use tokio::sync::{oneshot, watch};

use super::ClaudeRemoteError;
use super::channel::{ChannelState, PollStart};
use super::decision::{cancelled_output, decided_output};
use super::hook_input::{HookEvent, HookInput};
use super::transcript::{has_transcript_extension, read_transcript};
use crate::bridge::{BridgeMessage, BridgeSender, CommandResult, RemoteCommand};
use crate::types::ActionRequest;

/// The channel server re-polls after this; well below common HTTP idle limits.
const CHANNEL_POLL_TIMEOUT: Duration = Duration::from_secs(25);
/// Gap between pasting a prompt and pressing Enter, so the TUI has taken the
/// paste in before the submit arrives.
const ENTER_DELAY: Duration = Duration::from_millis(150);
/// How long a prompt Okena typed keeps the pane busy while no
/// `UserPromptSubmit` confirms it. A slash command, a swallowed paste or a
/// killed `claude` never confirms, and must not keep the pane busy for good.
const TYPED_PROMPT_GRACE: Duration = Duration::from_secs(5);
/// Per terminal, for the PTY queue and the channel queue each.
const MAX_QUEUED_PROMPTS: usize = 32;
const MAX_PROMPT_BYTES: usize = 64 * 1024;

/// Shared state between the `/v1/claude` routes and the daemon: open dialogs
/// (each held by a long-polling hook process), queued prompts and channel
/// attachments, keyed by terminal id.
pub struct ClaudeRemoteHub {
    bridge_tx: BridgeSender,
    state_version: Arc<watch::Sender<u64>>,
    terminals: Mutex<HashMap<String, TerminalState>>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct TerminalState {
    /// Oldest first.
    dialogs: Vec<OpenDialog>,
    session_id: Option<String>,
    transcript_path: Option<String>,
    hooks_attached: bool,
    /// From hooks only.
    busy: Option<bool>,
    /// A prompt Okena typed that Claude has not confirmed yet.
    typed_prompt: Option<TypedPrompt>,
    pty_queue: VecDeque<String>,
    channel: ChannelState,
}

struct TypedPrompt {
    /// Identifies this typing, so a failed one clears only its own marker.
    generation: u64,
    at: Instant,
}

struct OpenDialog {
    id: String,
    session_id: Option<String>,
    tool_name: String,
    tool_input: serde_json::Value,
    permission_suggestions: Vec<serde_json::Value>,
    created_at_ms: u64,
    /// Answers the hook's long-poll with what the hook prints.
    respond: oneshot::Sender<serde_json::Value>,
}

impl OpenDialog {
    fn to_api(&self) -> ApiClaudeDialog {
        ApiClaudeDialog {
            id: self.id.clone(),
            tool_name: self.tool_name.clone(),
            tool_input: self.tool_input.clone(),
            permission_suggestions: self.permission_suggestions.clone(),
            created_at_ms: self.created_at_ms,
        }
    }

    fn cancel(self) {
        // The hook may already be gone; nothing is waiting for the answer then.
        let _ = self.respond.send(cancelled_output());
    }
}

impl TerminalState {
    fn record_hook(&mut self, input: &HookInput) {
        self.hooks_attached = true;
        if let Some(session_id) = &input.session_id {
            self.session_id = Some(session_id.clone());
        }
        if let Some(transcript_path) = &input.transcript_path {
            self.transcript_path = Some(transcript_path.clone());
        }
    }

    /// A connected `claude`: hooks reported, or the channel server is polling.
    fn has_session(&self, now: Instant) -> bool {
        self.hooks_attached || self.channel.is_attached(now)
    }

    fn typed_prompt_pending(&self, now: Instant) -> bool {
        self.typed_prompt
            .as_ref()
            .is_some_and(|typed| now.saturating_duration_since(typed.at) < TYPED_PROMPT_GRACE)
    }

    /// Hook state first, the daemon's `lifecycle` when no hook has reported.
    fn busy(&self, now: Instant, lifecycle: Option<AgentLifecycle>) -> Option<bool> {
        if self.typed_prompt_pending(now) {
            return Some(true);
        }
        self.busy.or(lifecycle.map(lifecycle_is_busy))
    }

    /// Prompts for a session that has ended are not delivered to the next one.
    fn drop_undelivered_prompts(&mut self) {
        self.pty_queue.clear();
        self.channel.clear_prompts();
        self.typed_prompt = None;
    }

    fn take_all_dialogs(&mut self) -> Vec<OpenDialog> {
        std::mem::take(&mut self.dialogs)
    }

    /// The dialog a `PostToolUse` for this exact call answers: the terminal
    /// resolved it before any remote client did.
    fn take_matching_dialog(&mut self, input: &HookInput) -> Option<OpenDialog> {
        let index = self.dialogs.iter().position(|dialog| {
            dialog.session_id == input.session_id
                && Some(&dialog.tool_name) == input.tool_name.as_ref()
                && dialog.tool_input == input.tool_input
        })?;
        Some(self.dialogs.remove(index))
    }

    fn take_dialog(&mut self, dialog_id: &str) -> Option<OpenDialog> {
        let index = self
            .dialogs
            .iter()
            .position(|dialog| dialog.id == dialog_id)?;
        Some(self.dialogs.remove(index))
    }
}

/// Where a pane's transcript is read from, and which session it belongs to.
struct TranscriptSource {
    session_id: Option<String>,
    path: Option<String>,
}

impl ClaudeRemoteHub {
    /// `bridge_tx` reaches the daemon (terminal info, PTY writes);
    /// `state_version` is bumped whenever the open-dialog counts or the set of
    /// connected sessions change.
    pub fn new(bridge_tx: BridgeSender, state_version: Arc<watch::Sender<u64>>) -> Arc<Self> {
        Arc::new(Self {
            bridge_tx,
            state_version,
            terminals: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    /// Open dialogs per terminal id, for [`okena_core::api::StateResponse::claude_dialogs`].
    pub fn dialog_counts(&self) -> HashMap<String, u32> {
        self.terminals
            .lock()
            .iter()
            .filter(|(_, terminal)| !terminal.dialogs.is_empty())
            .map(|(terminal_id, terminal)| {
                let count = u32::try_from(terminal.dialogs.len()).unwrap_or(u32::MAX);
                (terminal_id.clone(), count)
            })
            .collect()
    }

    /// Terminals with a connected `claude`, for
    /// [`okena_core::api::StateResponse::claude_sessions`].
    pub fn session_terminals(&self) -> Vec<String> {
        let now = Instant::now();
        let mut terminal_ids: Vec<String> = self
            .terminals
            .lock()
            .iter()
            .filter(|(_, terminal)| terminal.has_session(now))
            .map(|(terminal_id, _)| terminal_id.clone())
            .collect();
        terminal_ids.sort();
        terminal_ids
    }

    /// Register the dialog a `PermissionRequest` hook reports and wait until a
    /// remote client answers it or it is cancelled. Returns what the hook
    /// prints. Dropping the future removes the dialog.
    pub(crate) async fn permission_request(
        &self,
        request: ClaudeHookRequest,
    ) -> Result<serde_json::Value, ClaudeRemoteError> {
        let input = HookInput::parse(&request.input)?;
        if input.event != HookEvent::PermissionRequest {
            return Err(ClaudeRemoteError::InvalidHookInput(
                "expected a PermissionRequest hook".to_string(),
            ));
        }
        let Some(tool_name) = input.tool_name.clone() else {
            return Err(ClaudeRemoteError::InvalidHookInput(
                "missing tool_name".to_string(),
            ));
        };

        let dialog_id = self.next_id().to_string();
        let (respond, answer) = oneshot::channel();
        {
            let mut terminals = self.terminals.lock();
            let terminal = terminals.entry(request.terminal_id.clone()).or_default();
            terminal.record_hook(&input);
            terminal.busy = Some(true);
            terminal.dialogs.push(OpenDialog {
                id: dialog_id.clone(),
                session_id: input.session_id,
                tool_name,
                tool_input: input.tool_input,
                permission_suggestions: input.permission_suggestions,
                created_at_ms: now_ms(),
                respond,
            });
        }
        self.bump_state_version();

        let _guard = DialogGuard {
            hub: self,
            terminal_id: request.terminal_id,
            dialog_id,
        };
        // Every removal path sends before dropping the sender; a bare drop
        // would still mean "no remote answer", which is the cancelled output.
        Ok(answer.await.unwrap_or_else(|_| cancelled_output()))
    }

    /// Apply a non-blocking hook event: track busy state, cancel the dialogs
    /// the terminal has resolved, and type the next queued prompt on `Stop`.
    pub(crate) fn hook_event(
        self: &Arc<Self>,
        request: ClaudeHookRequest,
    ) -> Result<(), ClaudeRemoteError> {
        let input = HookInput::parse(&request.input)?;
        let mut cancelled = Vec::new();
        let mut prompt_to_type = None;
        let session_changed;
        {
            let mut terminals = self.terminals.lock();
            let terminal = terminals.entry(request.terminal_id.clone()).or_default();
            let had_session = terminal.has_session(Instant::now());
            terminal.record_hook(&input);
            match input.event {
                // Compaction continues the same conversation mid-turn.
                HookEvent::SessionStart if input.source.as_deref() == Some("compact") => {}
                HookEvent::SessionStart => {
                    terminal.busy = Some(false);
                    terminal.drop_undelivered_prompts();
                    cancelled = terminal.take_all_dialogs();
                }
                HookEvent::UserPromptSubmit => {
                    terminal.busy = Some(true);
                    terminal.typed_prompt = None;
                    cancelled = terminal.take_all_dialogs();
                }
                HookEvent::PreToolUse | HookEvent::PermissionRequest => {
                    terminal.busy = Some(true);
                }
                HookEvent::PostToolUse | HookEvent::PostToolUseFailure => {
                    terminal.busy = Some(true);
                    cancelled.extend(terminal.take_matching_dialog(&input));
                }
                HookEvent::Stop => {
                    terminal.busy = Some(false);
                    cancelled = terminal.take_all_dialogs();
                    prompt_to_type = terminal.pty_queue.pop_front().map(|prompt| {
                        let generation = self.next_id();
                        terminal.typed_prompt = Some(TypedPrompt {
                            generation,
                            at: Instant::now(),
                        });
                        (prompt, generation)
                    });
                }
                HookEvent::SessionEnd => {
                    terminal.busy = None;
                    terminal.hooks_attached = false;
                    terminal.channel.detach();
                    terminal.drop_undelivered_prompts();
                    cancelled = terminal.take_all_dialogs();
                }
                HookEvent::Other => {}
            }
            session_changed = had_session != terminal.has_session(Instant::now());
        }

        if !cancelled.is_empty() || session_changed {
            cancelled.into_iter().for_each(OpenDialog::cancel);
            self.bump_state_version();
        }
        if let Some((prompt, generation)) = prompt_to_type {
            // Nobody waits for the result; the task logs a failure itself.
            drop(self.spawn_typing(request.terminal_id, prompt, generation));
        }
        Ok(())
    }

    /// Long-poll for prompts to deliver through the pane's MCP channel.
    pub(crate) async fn poll_channel(&self, terminal_id: &str) -> Vec<String> {
        self.poll_channel_for(terminal_id, CHANNEL_POLL_TIMEOUT)
            .await
    }

    async fn poll_channel_for(&self, terminal_id: &str, timeout: Duration) -> Vec<String> {
        let poll_id = self.next_id();
        let (start, attached) = {
            let mut terminals = self.terminals.lock();
            let terminal = terminals.entry(terminal_id.to_string()).or_default();
            let now = Instant::now();
            let had_session = terminal.has_session(now);
            let start = terminal.channel.begin_poll(poll_id, now);
            (start, !had_session)
        };
        if attached {
            self.bump_state_version();
        }
        let mut answer = match start {
            PollStart::Ready(prompts) => return prompts,
            PollStart::Wait(answer) => answer,
        };

        let guard = PollGuard {
            hub: self,
            terminal_id,
            poll_id,
        };
        match tokio::time::timeout(timeout, &mut answer).await {
            Ok(prompts) => prompts.unwrap_or_default(),
            Err(_elapsed) => {
                drop(guard);
                // A prompt pushed just before the guard detached the poll.
                answer.try_recv().unwrap_or_default()
            }
        }
    }

    /// What a remote client shows for the Claude session in a pane.
    pub(crate) async fn session(
        &self,
        terminal_id: &str,
    ) -> Result<ApiClaudeSession, ClaudeRemoteError> {
        let info = self.terminal_info(terminal_id).await?;
        let mut session = {
            let terminals = self.terminals.lock();
            let terminal = terminals.get(terminal_id);
            ApiClaudeSession {
                terminal_id: terminal_id.to_string(),
                session_id: terminal.and_then(|terminal| terminal.session_id.clone()),
                has_transcript: false,
                hooks_attached: terminal.is_some_and(|terminal| terminal.hooks_attached),
                channel_attached: terminal
                    .is_some_and(|terminal| terminal.channel.is_attached(Instant::now())),
                busy: match terminal {
                    Some(terminal) => terminal.busy(Instant::now(), info.lifecycle),
                    None => info.lifecycle.map(lifecycle_is_busy),
                },
                queued_prompts: terminal.map_or(0, |terminal| {
                    let queued = terminal.pty_queue.len() + terminal.channel.queued();
                    u32::try_from(queued).unwrap_or(u32::MAX)
                }),
                dialogs: terminal.map_or_else(Vec::new, |terminal| {
                    terminal.dialogs.iter().map(OpenDialog::to_api).collect()
                }),
            }
        };

        let source = self
            .hooked_transcript_source(terminal_id)
            .unwrap_or_else(|| TranscriptSource::from_terminal_info(&info));
        if session.session_id.is_none() {
            session.session_id = source.session_id;
        }
        session.has_transcript = match source.path {
            Some(path) => {
                has_transcript_extension(Path::new(&path))
                    && tokio::fs::metadata(&path)
                        .await
                        .is_ok_and(|metadata| metadata.is_file())
            }
            None => false,
        };
        Ok(session)
    }

    /// Transcript entries after `cursor`, or a first page.
    pub(crate) async fn transcript(
        &self,
        terminal_id: &str,
        cursor: Option<u64>,
        session_id: Option<String>,
    ) -> Result<ApiClaudeTranscriptPage, ClaudeRemoteError> {
        let source = match self.hooked_transcript_source(terminal_id) {
            Some(source) => source,
            None => TranscriptSource::from_terminal_info(&self.terminal_info(terminal_id).await?),
        };
        let Some(path) = source.path else {
            return Err(ClaudeRemoteError::NoTranscript);
        };
        // A cursor is only meaningful together with the session it came from.
        let same_session = session_id.is_some() && session_id == source.session_id;
        let resume_from = cursor.filter(|_| same_session);

        let chunk =
            tokio::task::spawn_blocking(move || read_transcript(&PathBuf::from(path), resume_from))
                .await
                .map_err(|error| ClaudeRemoteError::Io(std::io::Error::other(error)))?
                .map_err(|error| match error.kind() {
                    std::io::ErrorKind::NotFound => ClaudeRemoteError::NoTranscript,
                    _ => ClaudeRemoteError::Io(error),
                })?;

        Ok(ApiClaudeTranscriptPage {
            session_id: source.session_id,
            entries: chunk.entries,
            cursor: chunk.cursor,
            reset: chunk.reset,
        })
    }

    /// Deliver a prompt from a remote client: through the channel when it is
    /// attached, else typed into an idle terminal, else queued for the next
    /// `Stop` hook.
    pub(crate) async fn send_prompt(
        self: &Arc<Self>,
        terminal_id: &str,
        text: String,
    ) -> Result<ApiClaudePromptDelivery, ClaudeRemoteError> {
        if text.trim().is_empty() {
            return Err(ClaudeRemoteError::EmptyPrompt);
        }
        if text.len() > MAX_PROMPT_BYTES {
            return Err(ClaudeRemoteError::PromptTooLong);
        }
        let info = self.terminal_info(terminal_id).await?;

        let generation = self.next_id();
        {
            let mut terminals = self.terminals.lock();
            let terminal = terminals.entry(terminal_id.to_string()).or_default();
            let now = Instant::now();
            if terminal.channel.is_attached(now) {
                if terminal.channel.queued() >= MAX_QUEUED_PROMPTS {
                    return Err(ClaudeRemoteError::QueueFull);
                }
                terminal.channel.push(text, now);
                return Ok(ApiClaudePromptDelivery::Channel);
            }
            // Typing into a pane without Claude would run the prompt in a shell.
            if !terminal.hooks_attached && info.lifecycle.is_none() {
                return Err(ClaudeRemoteError::NoClaudeSession);
            }
            if terminal.busy(now, info.lifecycle) == Some(true) {
                if !terminal.hooks_attached {
                    return Err(ClaudeRemoteError::Busy);
                }
                if terminal.pty_queue.len() >= MAX_QUEUED_PROMPTS {
                    return Err(ClaudeRemoteError::QueueFull);
                }
                terminal.pty_queue.push_back(text);
                return Ok(ApiClaudePromptDelivery::Queued);
            }
            terminal.typed_prompt = Some(TypedPrompt {
                generation,
                at: now,
            });
        }

        self.spawn_typing(terminal_id.to_string(), text, generation)
            .await
            .map_err(|error| ClaudeRemoteError::Io(std::io::Error::other(error)))??;
        Ok(ApiClaudePromptDelivery::Pty)
    }

    /// Answer an open dialog with a remote client's decision.
    pub(crate) fn decide(
        &self,
        terminal_id: &str,
        dialog_id: &str,
        decision: &ApiClaudeDecision,
    ) -> Result<(), ClaudeRemoteError> {
        let dialog = self
            .terminals
            .lock()
            .get_mut(terminal_id)
            .and_then(|terminal| terminal.take_dialog(dialog_id))
            .ok_or(ClaudeRemoteError::DialogNotFound)?;
        self.bump_state_version();

        let output = decided_output(decision, &dialog.tool_input, &dialog.permission_suggestions);
        dialog
            .respond
            .send(output)
            .map_err(|_| ClaudeRemoteError::DialogNotFound)
    }

    fn hooked_transcript_source(&self, terminal_id: &str) -> Option<TranscriptSource> {
        let terminals = self.terminals.lock();
        let terminal = terminals.get(terminal_id)?;
        let path = terminal.transcript_path.clone()?;
        Some(TranscriptSource {
            session_id: terminal.session_id.clone(),
            path: Some(path),
        })
    }

    async fn terminal_info(
        &self,
        terminal_id: &str,
    ) -> Result<ClaudeTerminalInfo, ClaudeRemoteError> {
        let result = self
            .run(RemoteCommand::ClaudeTerminalInfo {
                terminal_id: terminal_id.to_string(),
            })
            .await?;
        match result {
            CommandResult::Ok(Some(value)) => serde_json::from_value(value).map_err(|error| {
                ClaudeRemoteError::Daemon(format!("invalid terminal info: {error}"))
            }),
            // The daemon refuses this only for a terminal it does not know.
            CommandResult::Err(message) => Err(ClaudeRemoteError::TerminalNotFound(message)),
            _ => Err(ClaudeRemoteError::Daemon(
                "unexpected terminal info reply".to_string(),
            )),
        }
    }

    /// Paste and submit `text` in a task of its own, so a client that goes
    /// away mid-way cannot leave a paste without its Enter.
    fn spawn_typing(
        self: &Arc<Self>,
        terminal_id: String,
        text: String,
        generation: u64,
    ) -> tokio::task::JoinHandle<Result<(), ClaudeRemoteError>> {
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let result = hub.type_into_pty(&terminal_id, &text).await;
            if let Err(error) = &result {
                log::warn!("claude remote: typing a prompt into {terminal_id} failed: {error:?}");
                hub.clear_typed_prompt(&terminal_id, generation);
            }
            result
        })
    }

    fn clear_typed_prompt(&self, terminal_id: &str, generation: u64) {
        if let Some(terminal) = self.terminals.lock().get_mut(terminal_id)
            && terminal
                .typed_prompt
                .as_ref()
                .is_some_and(|typed| typed.generation == generation)
        {
            terminal.typed_prompt = None;
        }
    }

    async fn type_into_pty(&self, terminal_id: &str, text: &str) -> Result<(), ClaudeRemoteError> {
        expect_ok(
            self.run(RemoteCommand::PastePath {
                terminal_id: terminal_id.to_string(),
                text: text.to_string(),
            })
            .await?,
        )?;
        tokio::time::sleep(ENTER_DELAY).await;
        expect_ok(
            self.run(RemoteCommand::Action(ActionRequest::SendSpecialKey {
                terminal_id: terminal_id.to_string(),
                key: SpecialKey::Enter,
            }))
            .await?,
        )
    }

    async fn run(&self, command: RemoteCommand) -> Result<CommandResult, ClaudeRemoteError> {
        let (reply, result) = oneshot::channel();
        self.bridge_tx
            .send(BridgeMessage {
                command,
                reply: Some(reply),
            })
            .await
            .map_err(|_| ClaudeRemoteError::BridgeUnavailable)?;
        result
            .await
            .map_err(|_| ClaudeRemoteError::BridgeUnavailable)
    }

    fn remove_dialog(&self, terminal_id: &str, dialog_id: &str) {
        let removed = self
            .terminals
            .lock()
            .get_mut(terminal_id)
            .and_then(|terminal| terminal.take_dialog(dialog_id));
        if removed.is_some() {
            self.bump_state_version();
        }
    }

    fn end_channel_poll(&self, terminal_id: &str, poll_id: u64) {
        if let Some(terminal) = self.terminals.lock().get_mut(terminal_id) {
            terminal.channel.end_poll(poll_id, Instant::now());
        }
    }

    fn bump_state_version(&self) {
        self.state_version.send_modify(|version| *version += 1);
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

impl TranscriptSource {
    fn from_terminal_info(info: &ClaudeTerminalInfo) -> Self {
        Self {
            session_id: info
                .session
                .as_ref()
                .map(|session| session.session_id.clone()),
            path: info
                .session
                .as_ref()
                .and_then(|session| session.transcript_path.clone())
                .filter(|path| is_valid_transcript_path(path)),
        }
    }
}

/// Removes a dialog whose hook stopped waiting (request dropped).
struct DialogGuard<'a> {
    hub: &'a ClaudeRemoteHub,
    terminal_id: String,
    dialog_id: String,
}

impl Drop for DialogGuard<'_> {
    fn drop(&mut self) {
        self.hub.remove_dialog(&self.terminal_id, &self.dialog_id);
    }
}

/// Detaches a channel poll that returned or was dropped.
struct PollGuard<'a> {
    hub: &'a ClaudeRemoteHub,
    terminal_id: &'a str,
    poll_id: u64,
}

impl Drop for PollGuard<'_> {
    fn drop(&mut self) {
        self.hub.end_channel_poll(self.terminal_id, self.poll_id);
    }
}

fn lifecycle_is_busy(lifecycle: AgentLifecycle) -> bool {
    match lifecycle {
        AgentLifecycle::Working | AgentLifecycle::Blocked => true,
        AgentLifecycle::Done | AgentLifecycle::Idle => false,
    }
}

fn expect_ok(result: CommandResult) -> Result<(), ClaudeRemoteError> {
    match result {
        CommandResult::Err(message) => Err(ClaudeRemoteError::Daemon(message)),
        CommandResult::Ok(_) | CommandResult::OkBytes(_) | CommandResult::OkSnapshot { .. } => {
            Ok(())
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::bridge_channel;
    use serde_json::json;
    use std::collections::BTreeMap;
    use tokio::sync::mpsc;

    /// A hub whose daemon answers `ClaudeTerminalInfo` with `info` and records
    /// every PTY write as `"paste <terminal> <text>"` / `"enter <terminal>"`.
    /// It does not know terminal `gone`, and pasting into `broken` fails.
    fn hub_with_daemon(
        info: ClaudeTerminalInfo,
    ) -> (
        Arc<ClaudeRemoteHub>,
        watch::Receiver<u64>,
        mpsc::UnboundedReceiver<String>,
    ) {
        let (bridge_tx, bridge_rx) = bridge_channel();
        let (version_tx, version_rx) = watch::channel(0);
        let hub = ClaudeRemoteHub::new(bridge_tx, Arc::new(version_tx));
        let (pty_tx, pty_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(message) = bridge_rx.recv().await {
                let result = match message.command {
                    RemoteCommand::ClaudeTerminalInfo { terminal_id } if terminal_id == "gone" => {
                        CommandResult::Err("terminal not found: gone".to_string())
                    }
                    RemoteCommand::ClaudeTerminalInfo { .. } => CommandResult::Ok(Some(
                        serde_json::to_value(&info).expect("serializable info"),
                    )),
                    RemoteCommand::PastePath { terminal_id, .. } if terminal_id == "broken" => {
                        CommandResult::Err("terminal not found: broken".to_string())
                    }
                    RemoteCommand::PastePath { terminal_id, text } => {
                        let _ = pty_tx.send(format!("paste {terminal_id} {text}"));
                        CommandResult::Ok(None)
                    }
                    RemoteCommand::Action(ActionRequest::SendSpecialKey {
                        terminal_id,
                        key: SpecialKey::Enter,
                    }) => {
                        let _ = pty_tx.send(format!("enter {terminal_id}"));
                        CommandResult::Ok(None)
                    }
                    other => CommandResult::Err(format!("unexpected command {other:?}")),
                };
                if let Some(reply) = message.reply {
                    let _ = reply.send(result);
                }
            }
        });
        (hub, version_rx, pty_rx)
    }

    fn idle_daemon() -> ClaudeTerminalInfo {
        ClaudeTerminalInfo::default()
    }

    fn hook(terminal_id: &str, input: serde_json::Value) -> ClaudeHookRequest {
        ClaudeHookRequest {
            terminal_id: terminal_id.to_string(),
            input,
        }
    }

    fn event(name: &str) -> serde_json::Value {
        json!({
            "hook_event_name": name,
            "session_id": "s1",
            "transcript_path": "/home/u/.claude/projects/p/s1.jsonl"
        })
    }

    fn tool_event(name: &str, tool_name: &str, tool_input: serde_json::Value) -> serde_json::Value {
        let mut input = event(name);
        input["tool_name"] = json!(tool_name);
        input["tool_input"] = tool_input;
        input
    }

    fn permission(tool_name: &str, tool_input: serde_json::Value) -> serde_json::Value {
        let mut input = tool_event("PermissionRequest", tool_name, tool_input);
        input["permission_suggestions"] = json!([{"type": "addRules", "rules": [{"toolName": tool_name}], "behavior": "allow", "destination": "localSettings"}]);
        input
    }

    /// Start a `PermissionRequest` long-poll and wait until its dialog is open.
    async fn open_dialog(
        hub: &Arc<ClaudeRemoteHub>,
        terminal_id: &str,
        input: serde_json::Value,
    ) -> (
        String,
        tokio::task::JoinHandle<Result<serde_json::Value, ClaudeRemoteError>>,
    ) {
        let known = dialog_ids(hub, terminal_id);
        let waiting = tokio::spawn({
            let hub = Arc::clone(hub);
            let request = hook(terminal_id, input);
            async move { hub.permission_request(request).await }
        });
        for _ in 0..200 {
            if let Some(id) = dialog_ids(hub, terminal_id)
                .into_iter()
                .find(|id| !known.contains(id))
            {
                return (id, waiting);
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("dialog never opened");
    }

    fn dialog_ids(hub: &ClaudeRemoteHub, terminal_id: &str) -> Vec<String> {
        hub.terminals
            .lock()
            .get(terminal_id)
            .map(|terminal| {
                terminal
                    .dialogs
                    .iter()
                    .map(|dialog| dialog.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn answer_of(
        waiting: tokio::task::JoinHandle<Result<serde_json::Value, ClaudeRemoteError>>,
    ) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("hook answered")
            .expect("task finished")
            .expect("valid request")
    }

    #[tokio::test]
    async fn decision_answers_the_hook_long_poll() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        let ask_input = json!({"questions": [{"question": "Which color?", "options": [{"label": "Red"}, {"label": "Blue"}]}]});
        let cases = [
            (
                ApiClaudeDecision::Allow { always: false },
                json!({"behavior": "allow", "updatedInput": ask_input}),
            ),
            (
                ApiClaudeDecision::Allow { always: true },
                json!({"behavior": "allow", "updatedInput": ask_input, "updatedPermissions": [
                    {"type": "addRules", "rules": [{"toolName": "AskUserQuestion"}], "behavior": "allow", "destination": "session"}
                ]}),
            ),
            (
                ApiClaudeDecision::Deny { message: None },
                json!({"behavior": "deny", "message": "Denied from Okena remote"}),
            ),
            (
                ApiClaudeDecision::Answer {
                    answers: BTreeMap::from([("Which color?".to_string(), "Blue".to_string())]),
                },
                json!({"behavior": "allow", "updatedInput": {
                    "questions": [{"question": "Which color?", "options": [{"label": "Red"}, {"label": "Blue"}]}],
                    "answers": {"Which color?": "Blue"}
                }}),
            ),
        ];

        for (decision, expected) in cases {
            let (dialog_id, waiting) =
                open_dialog(&hub, "t1", permission("AskUserQuestion", ask_input.clone())).await;
            hub.decide("t1", &dialog_id, &decision)
                .expect("dialog open");

            assert_eq!(
                answer_of(waiting).await,
                json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": expected}})
            );
            assert!(hub.dialog_counts().is_empty());
        }
    }

    #[tokio::test]
    async fn deciding_an_unknown_dialog_is_not_found() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        assert!(matches!(
            hub.decide("t1", "42", &ApiClaudeDecision::Allow { always: false }),
            Err(ClaudeRemoteError::DialogNotFound)
        ));
    }

    #[tokio::test]
    async fn post_tool_use_cancels_only_the_matching_dialog() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        let (_ls_id, ls) =
            open_dialog(&hub, "t1", permission("Bash", json!({"command": "ls"}))).await;
        let (_rm_id, _rm) =
            open_dialog(&hub, "t1", permission("Bash", json!({"command": "rm x"}))).await;

        hub.hook_event(hook(
            "t1",
            tool_event("PostToolUse", "Bash", json!({"command": "pwd"})),
        ))
        .expect("valid event");
        assert_eq!(hub.dialog_counts(), HashMap::from([("t1".to_string(), 2)]));

        hub.hook_event(hook(
            "t1",
            tool_event("PostToolUse", "Bash", json!({"command": "ls"})),
        ))
        .expect("valid event");
        assert_eq!(answer_of(ls).await, json!({}));
        assert_eq!(hub.dialog_counts(), HashMap::from([("t1".to_string(), 1)]));
    }

    #[tokio::test]
    async fn stop_cancels_every_dialog_of_the_terminal() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        let (_, first) =
            open_dialog(&hub, "t1", permission("Bash", json!({"command": "ls"}))).await;
        let (_, second) =
            open_dialog(&hub, "t1", permission("Write", json!({"file_path": "/a"}))).await;
        let (_, other) =
            open_dialog(&hub, "t2", permission("Bash", json!({"command": "ls"}))).await;
        assert_eq!(
            hub.dialog_counts(),
            HashMap::from([("t1".to_string(), 2), ("t2".to_string(), 1)])
        );

        hub.hook_event(hook("t1", event("Stop")))
            .expect("valid event");

        assert_eq!(answer_of(first).await, json!({}));
        assert_eq!(answer_of(second).await, json!({}));
        assert_eq!(hub.dialog_counts(), HashMap::from([("t2".to_string(), 1)]));
        other.abort();
    }

    #[tokio::test]
    async fn dropping_the_hook_request_removes_its_dialog() {
        let (hub, version, _pty) = hub_with_daemon(idle_daemon());
        let (_, waiting) =
            open_dialog(&hub, "t1", permission("Bash", json!({"command": "ls"}))).await;
        assert_eq!(hub.dialog_counts(), HashMap::from([("t1".to_string(), 1)]));
        let opened_version = *version.borrow();

        waiting.abort();
        let _ = waiting.await;

        assert!(hub.dialog_counts().is_empty());
        assert!(*version.borrow() > opened_version);
    }

    #[tokio::test]
    async fn permission_request_records_the_session() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        let (_, _waiting) =
            open_dialog(&hub, "t1", permission("Bash", json!({"command": "ls"}))).await;

        let session = hub.session("t1").await.expect("session");
        assert_eq!(session.session_id.as_deref(), Some("s1"));
        assert!(session.hooks_attached);
        assert_eq!(session.busy, Some(true));
        assert_eq!(session.dialogs.len(), 1);
        assert_eq!(session.dialogs[0].tool_name, "Bash");
        assert_eq!(session.dialogs[0].permission_suggestions.len(), 1);
    }

    #[tokio::test]
    async fn prompt_goes_to_the_attached_channel() {
        let (hub, _version, mut pty) = hub_with_daemon(idle_daemon());
        let poll = tokio::spawn({
            let hub = Arc::clone(&hub);
            async move { hub.poll_channel_for("t1", Duration::from_secs(5)).await }
        });
        for _ in 0..200 {
            let attached = hub
                .terminals
                .lock()
                .get("t1")
                .is_some_and(|terminal| terminal.channel.is_attached(Instant::now()));
            if attached {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let delivery = hub.send_prompt("t1", "hello".into()).await;

        assert_eq!(
            delivery.expect("delivered"),
            ApiClaudePromptDelivery::Channel
        );
        assert_eq!(
            poll.await.expect("poll finished"),
            vec!["hello".to_string()]
        );
        assert!(pty.try_recv().is_err());
    }

    #[tokio::test]
    async fn timed_out_poll_returns_empty_and_keeps_the_channel_attached() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        let prompts = hub.poll_channel_for("t1", Duration::from_millis(10)).await;
        assert!(prompts.is_empty());

        let delivery = hub.send_prompt("t1", "later".into()).await;
        assert_eq!(
            delivery.expect("delivered"),
            ApiClaudePromptDelivery::Channel
        );
        assert_eq!(hub.poll_channel("t1").await, vec!["later".to_string()]);
    }

    #[tokio::test]
    async fn idle_prompt_is_typed_and_the_next_one_waits_for_stop() {
        let (hub, _version, mut pty) = hub_with_daemon(idle_daemon());
        hub.hook_event(hook("t1", event("SessionStart")))
            .expect("valid event");

        let first = hub.send_prompt("t1", "first".into()).await;
        assert_eq!(first.expect("delivered"), ApiClaudePromptDelivery::Pty);
        assert_eq!(pty.recv().await.as_deref(), Some("paste t1 first"));
        assert_eq!(pty.recv().await.as_deref(), Some("enter t1"));

        let second = hub.send_prompt("t1", "second".into()).await;
        let third = hub.send_prompt("t1", "third".into()).await;
        assert_eq!(second.expect("accepted"), ApiClaudePromptDelivery::Queued);
        assert_eq!(third.expect("accepted"), ApiClaudePromptDelivery::Queued);
        assert_eq!(hub.session("t1").await.expect("session").queued_prompts, 2);

        hub.hook_event(hook("t1", event("UserPromptSubmit")))
            .expect("valid event");
        hub.hook_event(hook("t1", event("Stop")))
            .expect("valid event");
        assert_eq!(pty.recv().await.as_deref(), Some("paste t1 second"));
        assert_eq!(pty.recv().await.as_deref(), Some("enter t1"));
        assert_eq!(hub.session("t1").await.expect("session").queued_prompts, 1);

        hub.hook_event(hook("t1", event("Stop")))
            .expect("valid event");
        assert_eq!(pty.recv().await.as_deref(), Some("paste t1 third"));
        assert_eq!(pty.recv().await.as_deref(), Some("enter t1"));
    }

    #[tokio::test]
    async fn without_hooks_the_daemon_lifecycle_decides() {
        let (idle_hub, _version, mut pty) = hub_with_daemon(ClaudeTerminalInfo {
            session: None,
            lifecycle: Some(AgentLifecycle::Done),
        });
        let delivery = idle_hub.send_prompt("t1", "go".into()).await;
        assert_eq!(delivery.expect("delivered"), ApiClaudePromptDelivery::Pty);
        assert_eq!(pty.recv().await.as_deref(), Some("paste t1 go"));

        let (busy_hub, _version, mut pty) = hub_with_daemon(ClaudeTerminalInfo {
            session: None,
            lifecycle: Some(AgentLifecycle::Working),
        });
        let delivery = busy_hub.send_prompt("t1", "go".into()).await;
        assert!(matches!(delivery, Err(ClaudeRemoteError::Busy)));
        assert!(pty.try_recv().is_err());
    }

    #[tokio::test]
    async fn busy_after_session_end_is_refused() {
        let (hub, _version, _pty) = hub_with_daemon(ClaudeTerminalInfo {
            session: None,
            lifecycle: Some(AgentLifecycle::Blocked),
        });
        hub.hook_event(hook("t1", event("UserPromptSubmit")))
            .expect("valid event");
        hub.hook_event(hook("t1", event("SessionEnd")))
            .expect("valid event");

        let delivery = hub.send_prompt("t1", "go".into()).await;
        assert!(matches!(delivery, Err(ClaudeRemoteError::Busy)));
    }

    #[tokio::test]
    async fn blank_prompt_is_rejected() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        let delivery = hub.send_prompt("t1", " \n".into()).await;
        assert!(matches!(delivery, Err(ClaudeRemoteError::EmptyPrompt)));
    }

    #[tokio::test]
    async fn transcript_follows_the_hook_reported_path() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("s1.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n",
                json!({"type": "assistant", "uuid": "a1", "message": {"content": [{"type": "text", "text": "hi"}]}})
            ),
        )
        .expect("write transcript");
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        let mut start = event("SessionStart");
        start["transcript_path"] = json!(path.to_string_lossy());
        hub.hook_event(hook("t1", start)).expect("valid event");

        let page = hub.transcript("t1", None, None).await.expect("page");
        assert!(page.reset);
        assert_eq!(page.session_id.as_deref(), Some("s1"));
        assert_eq!(page.entries.len(), 1);

        let next = hub
            .transcript("t1", Some(page.cursor), Some("s1".into()))
            .await
            .expect("page");
        assert!(!next.reset);
        assert!(next.entries.is_empty());

        let without_session = hub
            .transcript("t1", Some(page.cursor), None)
            .await
            .expect("page");
        assert!(without_session.reset);
        assert_eq!(without_session.entries.len(), 1);

        let other_session = hub
            .transcript("t1", Some(page.cursor), Some("s0".into()))
            .await
            .expect("page");
        assert!(other_session.reset);
        assert_eq!(other_session.entries.len(), 1);

        assert!(hub.session("t1").await.expect("session").has_transcript);
    }

    #[tokio::test]
    async fn transcript_without_a_source_is_not_found() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        assert!(matches!(
            hub.transcript("t1", None, None).await,
            Err(ClaudeRemoteError::NoTranscript)
        ));
    }

    fn claude_running() -> ClaudeTerminalInfo {
        ClaudeTerminalInfo {
            session: None,
            lifecycle: Some(AgentLifecycle::Idle),
        }
    }

    fn queued_prompts(hub: &ClaudeRemoteHub, terminal_id: &str) -> (usize, usize) {
        hub.terminals
            .lock()
            .get(terminal_id)
            .map_or((0, 0), |terminal| {
                (terminal.pty_queue.len(), terminal.channel.queued())
            })
    }

    #[tokio::test]
    async fn turn_and_session_boundaries_cancel_every_dialog() {
        for name in ["UserPromptSubmit", "SessionStart", "SessionEnd"] {
            let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
            let (_, waiting) =
                open_dialog(&hub, "t1", permission("Bash", json!({"command": "ls"}))).await;

            hub.hook_event(hook("t1", event(name)))
                .expect("valid event");

            assert_eq!(answer_of(waiting).await, json!({}), "{name}");
            assert!(hub.dialog_counts().is_empty(), "{name}");
        }
    }

    #[tokio::test]
    async fn compaction_keeps_dialogs_busy_state_and_queue() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        hub.hook_event(hook("t1", event("UserPromptSubmit")))
            .expect("valid event");
        let delivery = hub.send_prompt("t1", "later".into()).await;
        assert_eq!(delivery.expect("accepted"), ApiClaudePromptDelivery::Queued);
        let (_, _waiting) =
            open_dialog(&hub, "t1", permission("Bash", json!({"command": "ls"}))).await;

        let mut compact = event("SessionStart");
        compact["source"] = json!("compact");
        hub.hook_event(hook("t1", compact)).expect("valid event");

        assert_eq!(hub.dialog_counts(), HashMap::from([("t1".to_string(), 1)]));
        assert_eq!(hub.session("t1").await.expect("session").busy, Some(true));
        assert_eq!(queued_prompts(&hub, "t1"), (1, 0));
    }

    #[tokio::test]
    async fn state_version_follows_open_decide_and_cancel() {
        let (hub, version, _pty) = hub_with_daemon(idle_daemon());
        let start = *version.borrow();

        let (dialog_id, waiting) =
            open_dialog(&hub, "t1", permission("Bash", json!({"command": "ls"}))).await;
        let opened = *version.borrow();
        assert!(opened > start);

        hub.decide(
            "t1",
            &dialog_id,
            &ApiClaudeDecision::Allow { always: false },
        )
        .expect("dialog open");
        answer_of(waiting).await;
        let decided = *version.borrow();
        assert!(decided > opened);

        let (_, waiting) =
            open_dialog(&hub, "t1", permission("Bash", json!({"command": "ls"}))).await;
        let reopened = *version.borrow();
        hub.hook_event(hook("t1", event("Stop")))
            .expect("valid event");
        answer_of(waiting).await;
        assert!(*version.borrow() > reopened);

        let unchanged = *version.borrow();
        hub.hook_event(hook("t1", event("Stop")))
            .expect("valid event");
        assert_eq!(*version.borrow(), unchanged);
    }

    #[tokio::test]
    async fn sessions_follow_hooks_and_the_channel() {
        let (hub, version, _pty) = hub_with_daemon(idle_daemon());
        assert!(hub.session_terminals().is_empty());

        let start = *version.borrow();
        hub.hook_event(hook("t1", event("SessionStart")))
            .expect("valid event");
        assert_eq!(hub.session_terminals(), vec!["t1".to_string()]);
        let attached = *version.borrow();
        assert!(attached > start);

        let polling = tokio::spawn({
            let hub = Arc::clone(&hub);
            async move { hub.poll_channel_for("t2", Duration::from_secs(60)).await }
        });
        for _ in 0..200 {
            if hub.session_terminals().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(
            hub.session_terminals(),
            vec!["t1".to_string(), "t2".to_string()]
        );
        assert!(*version.borrow() > attached);

        let before_end = *version.borrow();
        hub.hook_event(hook("t2", event("SessionEnd")))
            .expect("valid event");
        assert_eq!(hub.session_terminals(), vec!["t1".to_string()]);
        assert!(*version.borrow() > before_end);
        assert!(polling.await.expect("poll finished").is_empty());
    }

    #[tokio::test]
    async fn a_pane_without_claude_is_never_typed_into() {
        let (hub, _version, mut pty) = hub_with_daemon(idle_daemon());
        let delivery = hub.send_prompt("t1", "rm -rf ~".into()).await;
        assert!(matches!(delivery, Err(ClaudeRemoteError::NoClaudeSession)));
        assert!(pty.try_recv().is_err());
    }

    #[tokio::test]
    async fn an_unconfirmed_typed_prompt_stops_counting_as_busy() {
        let (hub, _version, mut pty) = hub_with_daemon(idle_daemon());
        hub.hook_event(hook("t1", event("SessionStart")))
            .expect("valid event");
        let first = hub.send_prompt("t1", "/clear".into()).await;
        assert_eq!(first.expect("delivered"), ApiClaudePromptDelivery::Pty);
        assert_eq!(hub.session("t1").await.expect("session").busy, Some(true));

        let expired = Instant::now()
            .checked_sub(TYPED_PROMPT_GRACE + Duration::from_secs(1))
            .expect("clock is past the grace period");
        if let Some(typed) = hub
            .terminals
            .lock()
            .get_mut("t1")
            .and_then(|terminal| terminal.typed_prompt.as_mut())
        {
            typed.at = expired;
        }

        assert_eq!(hub.session("t1").await.expect("session").busy, Some(false));
        let second = hub.send_prompt("t1", "hello".into()).await;
        assert_eq!(second.expect("delivered"), ApiClaudePromptDelivery::Pty);
        let typed: Vec<String> = std::iter::from_fn(|| pty.try_recv().ok()).collect();
        assert!(typed.contains(&"paste t1 hello".to_string()), "{typed:?}");
    }

    #[tokio::test]
    async fn a_failed_typing_clears_only_its_own_marker() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        hub.hook_event(hook("broken", event("SessionStart")))
            .expect("valid event");

        let delivery = hub.send_prompt("broken", "hello".into()).await;

        assert!(matches!(delivery, Err(ClaudeRemoteError::Daemon(_))));
        assert_eq!(
            hub.session("broken").await.expect("session").busy,
            Some(false)
        );

        hub.terminals
            .lock()
            .get_mut("broken")
            .expect("terminal state")
            .typed_prompt = Some(TypedPrompt {
            generation: 7,
            at: Instant::now(),
        });
        hub.clear_typed_prompt("broken", 6);
        assert_eq!(
            hub.session("broken").await.expect("session").busy,
            Some(true)
        );
    }

    #[tokio::test]
    async fn a_dropped_prompt_request_still_presses_enter() {
        let (hub, _version, mut pty) = hub_with_daemon(idle_daemon());
        hub.hook_event(hook("t1", event("SessionStart")))
            .expect("valid event");
        let request = tokio::spawn({
            let hub = Arc::clone(&hub);
            async move { hub.send_prompt("t1", "hello".into()).await }
        });
        assert_eq!(pty.recv().await.as_deref(), Some("paste t1 hello"));

        request.abort();

        let enter = tokio::time::timeout(Duration::from_secs(1), pty.recv()).await;
        assert_eq!(enter.expect("enter pressed").as_deref(), Some("enter t1"));
    }

    #[tokio::test]
    async fn a_new_session_drops_prompts_of_the_old_one() {
        for name in ["SessionStart", "SessionEnd"] {
            let (hub, _version, _pty) = hub_with_daemon(claude_running());
            hub.hook_event(hook("t1", event("UserPromptSubmit")))
                .expect("valid event");
            let queued = hub.send_prompt("t1", "pty".into()).await;
            assert_eq!(queued.expect("accepted"), ApiClaudePromptDelivery::Queued);
            assert!(
                hub.poll_channel_for("t1", Duration::from_millis(1))
                    .await
                    .is_empty()
            );
            let channel = hub.send_prompt("t1", "channel".into()).await;
            assert_eq!(channel.expect("accepted"), ApiClaudePromptDelivery::Channel);
            assert_eq!(queued_prompts(&hub, "t1"), (1, 1), "{name}");

            hub.hook_event(hook("t1", event(name)))
                .expect("valid event");

            assert_eq!(queued_prompts(&hub, "t1"), (0, 0), "{name}");
        }
    }

    #[tokio::test]
    async fn queues_and_prompt_size_are_capped() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        hub.hook_event(hook("t1", event("UserPromptSubmit")))
            .expect("valid event");
        for n in 0..MAX_QUEUED_PROMPTS {
            let delivery = hub.send_prompt("t1", format!("prompt {n}")).await;
            assert_eq!(delivery.expect("accepted"), ApiClaudePromptDelivery::Queued);
        }
        let overflow = hub.send_prompt("t1", "one too many".into()).await;
        assert!(matches!(overflow, Err(ClaudeRemoteError::QueueFull)));

        assert!(
            hub.poll_channel_for("t2", Duration::from_millis(1))
                .await
                .is_empty()
        );
        for n in 0..MAX_QUEUED_PROMPTS {
            let delivery = hub.send_prompt("t2", format!("prompt {n}")).await;
            assert_eq!(
                delivery.expect("accepted"),
                ApiClaudePromptDelivery::Channel
            );
        }
        let overflow = hub.send_prompt("t2", "one too many".into()).await;
        assert!(matches!(overflow, Err(ClaudeRemoteError::QueueFull)));

        let too_long = hub
            .send_prompt("t1", "x".repeat(MAX_PROMPT_BYTES + 1))
            .await;
        assert!(matches!(too_long, Err(ClaudeRemoteError::PromptTooLong)));
    }

    #[tokio::test]
    async fn unknown_terminal_is_not_found() {
        let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
        assert!(matches!(
            hub.session("gone").await,
            Err(ClaudeRemoteError::TerminalNotFound(_))
        ));
        assert!(matches!(
            hub.send_prompt("gone", "hi".into()).await,
            Err(ClaudeRemoteError::TerminalNotFound(_))
        ));
    }

    #[tokio::test]
    async fn transcript_must_be_a_jsonl_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let not_jsonl = directory.path().join("s1.txt");
        std::fs::write(&not_jsonl, "").expect("write");
        let not_a_file = directory.path().join("dir.jsonl");
        std::fs::create_dir(&not_a_file).expect("create dir");

        for path in [not_jsonl, not_a_file] {
            let (hub, _version, _pty) = hub_with_daemon(idle_daemon());
            let mut start = event("SessionStart");
            start["transcript_path"] = json!(path.to_string_lossy());
            hub.hook_event(hook("t1", start)).expect("valid event");

            assert!(matches!(
                hub.transcript("t1", None, None).await,
                Err(ClaudeRemoteError::NoTranscript)
            ));
            assert!(!hub.session("t1").await.expect("session").has_transcript);
        }
    }
}
