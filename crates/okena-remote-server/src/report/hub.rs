//! The report hub: links, the poll that notices a child's turn ending or its
//! terminal closing, waits, and delivery into the parent's input.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use okena_core::claude_remote::{ApiClaudeEntry, ApiClaudePromptDelivery, ClaudeTerminalInfo};
use okena_core::keys::SpecialKey;
use okena_core::report::{
    ApiReportChildrenResponse, ApiReportEvent, ApiReportLinkRequest, ApiReportRequest,
    ApiReportWaitRequest, ApiReportWaitResponse, ReportDelivery, ReportEventKind,
    ReportProbeEntry,
};
use parking_lot::Mutex;
use tokio::sync::{Notify, oneshot};

use super::message::{MAX_BODY_BYTES, SCREEN_TAIL_LINES, sanitize_body, screen_tail};
use super::state::{Links, Observed};
use crate::bridge::{BridgeMessage, BridgeSender, CommandResult, RemoteCommand};
use crate::claude_remote::{ClaudeRemoteError, ClaudeRemoteHub};
use crate::types::ActionRequest;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// The longest one wait request blocks; the CLI asks again.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(25);
const ENTER_DELAY: Duration = Duration::from_millis(150);
/// What `okena spawn` appends to the command it types; screen tails skip the
/// line that shows it.
pub const EXIT_REPORT_MARK: &str = "report --exit-code";

/// Foreground programs that take a typed report: coding agents with a prompt.
/// Anything else (a shell, an editor, a pager) only gets a report through
/// `okena wait`. A program that reports its own agent status counts too.
const AGENT_COMMANDS: &[&str] = &[
    "claude",
    "codex",
    "gemini",
    "opencode",
    "aider",
    "amp",
    "goose",
    "crush",
    "cursor-agent",
    "grok",
    "qwen",
    "droid",
];
const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "mksh", "tcsh", "csh", "nu", "pwsh", "login",
];

#[derive(Debug)]
pub(crate) enum ReportError {
    NotLinked,
    TerminalNotFound(String),
    Invalid(String),
    BridgeUnavailable,
}

pub struct ReportHub {
    bridge_tx: BridgeSender,
    claude_hub: Arc<ClaudeRemoteHub>,
    state: Mutex<HubState>,
    /// Woken on every recorded report, for waits.
    reported: Notify,
}

#[derive(Default)]
struct HubState {
    links: Links,
    polling: bool,
}

impl ReportHub {
    pub fn new(bridge_tx: BridgeSender, claude_hub: Arc<ClaudeRemoteHub>) -> Arc<Self> {
        Arc::new(Self {
            bridge_tx,
            claude_hub,
            state: Mutex::new(HubState::default()),
            reported: Notify::new(),
        })
    }

    pub(crate) async fn link(
        self: &Arc<Self>,
        request: &ApiReportLinkRequest,
    ) -> Result<(), ReportError> {
        let child = &request.child_terminal_id;
        let parent = &request.parent_terminal_id;
        let entries = self.probe(vec![child.clone(), parent.clone()], Vec::new()).await?;
        let find = |id: &str| entries.iter().find(|entry| entry.terminal_id == id);
        let child_entry = find(child).ok_or_else(|| ReportError::TerminalNotFound(child.clone()))?;
        find(parent).ok_or_else(|| ReportError::TerminalNotFound(parent.clone()))?;
        self.state
            .lock()
            .links
            .link(child, parent, request.owned, child_entry)
            .map_err(|error| ReportError::Invalid(error.to_string()))?;
        self.ensure_polling();
        Ok(())
    }

    pub(crate) fn unlink(&self, child: &str) -> Result<(), ReportError> {
        if self.state.lock().links.unlink(child) {
            Ok(())
        } else {
            Err(ReportError::NotLinked)
        }
    }

    /// A child reports itself (`okena report`).
    pub(crate) async fn report(
        self: &Arc<Self>,
        request: &ApiReportRequest,
    ) -> Result<ApiReportEvent, ReportError> {
        let child = &request.terminal_id;
        if !self.state.lock().links.is_linked(child) {
            return Err(ReportError::NotLinked);
        }
        let body = match request.event {
            ReportEventKind::Exit => {
                let screen = self.screen(child).await?;
                screen_tail(&screen, SCREEN_TAIL_LINES, Some(EXIT_REPORT_MARK))
            }
            ReportEventKind::Report
            | ReportEventKind::Ask
            | ReportEventKind::Done
            | ReportEventKind::Error => request.text.clone(),
            ReportEventKind::Blocked | ReportEventKind::Closed => {
                return Err(ReportError::Invalid(format!(
                    "a child cannot report `{}` itself",
                    request.event.as_str()
                )));
            }
        };
        let exit_code = request
            .exit_code
            .filter(|_| request.event == ReportEventKind::Exit);
        let event = self
            .record(child, request.event, exit_code, &body, true)
            .ok_or(ReportError::NotLinked)?;
        self.ensure_polling();
        self.flush_soon();
        Ok(event)
    }

    /// Block until a child of the parent has a report (all of them with
    /// `all`), at most `timeout_ms`, capped at [`MAX_WAIT`].
    pub(crate) async fn wait(
        &self,
        request: &ApiReportWaitRequest,
    ) -> Result<ApiReportWaitResponse, ReportError> {
        let parent = &request.parent_terminal_id;
        let awaited = self
            .state
            .lock()
            .links
            .awaited(parent, &request.children)
            .map_err(ReportError::Invalid)?;
        let deadline =
            Instant::now() + Duration::from_millis(request.timeout_ms).min(MAX_WAIT);
        self.state.lock().links.begin_wait(parent);
        let _end = EndWait { hub: self, parent };
        loop {
            // Registered before the check, so a report recorded in between
            // still wakes this wait.
            let reported = self.reported.notified();
            let taken = self.state.lock().links.take_for_wait(&awaited, request.all);
            match taken {
                Ok(events) => {
                    return Ok(ApiReportWaitResponse {
                        events,
                        waiting: Vec::new(),
                    });
                }
                Err(waiting) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero()
                        || tokio::time::timeout(left, reported).await.is_err()
                    {
                        return Ok(ApiReportWaitResponse {
                            events: Vec::new(),
                            waiting,
                        });
                    }
                }
            }
        }
    }

    pub(crate) fn children(&self, parent: Option<&str>) -> ApiReportChildrenResponse {
        ApiReportChildrenResponse {
            children: self.state.lock().links.children(parent),
        }
    }

    /// Input reached `terminal_id` through the API: if it is a child, it runs
    /// again.
    pub(crate) fn note_input(&self, terminal_id: &str) {
        self.state.lock().links.steer(terminal_id);
    }

    fn record(
        &self,
        child: &str,
        event: ReportEventKind,
        exit_code: Option<i32>,
        body: &str,
        explicit: bool,
    ) -> Option<ApiReportEvent> {
        let body = sanitize_body(body, MAX_BODY_BYTES);
        let recorded =
            self.state
                .lock()
                .links
                .record(child, event, exit_code, body, explicit, now_ms());
        if recorded.is_some() {
            self.reported.notify_waiters();
        }
        recorded
    }

    /// Poll every second while any link exists.
    fn ensure_polling(self: &Arc<Self>) {
        {
            let mut state = self.state.lock();
            if state.polling || state.links.is_empty() {
                return;
            }
            state.polling = true;
        }
        let hub = Arc::clone(self);
        drop(tokio::spawn(async move {
            loop {
                tokio::time::sleep(POLL_INTERVAL).await;
                hub.poll().await;
                let mut state = hub.state.lock();
                if state.links.is_empty() {
                    state.polling = false;
                    break;
                }
            }
        }));
    }

    async fn poll(self: &Arc<Self>) {
        let ids = self.state.lock().links.terminal_ids();
        let entries = match self.probe(ids, Vec::new()).await {
            Ok(entries) => entries,
            Err(error) => {
                log::warn!("report: polling the daemon failed: {error:?}");
                return;
            }
        };
        let observed = self.state.lock().links.observe(&entries, Instant::now());
        self.apply(observed).await;
        self.flush().await;
    }

    async fn apply(&self, observed: Observed) {
        for child in observed.closed {
            self.record(&child, ReportEventKind::Closed, None, "", false);
        }
        for (child, kind) in observed.turns {
            let body = self.turn_body(&child, kind).await;
            self.record(&child, kind, None, &body, false);
        }
        for (child, project_id) in observed.orphans {
            let close = ActionRequest::CloseTerminal {
                project_id,
                terminal_id: child.clone(),
            };
            match self.run(RemoteCommand::Action(close)).await {
                Ok(CommandResult::Err(error)) => {
                    log::warn!("report: closing {child}, whose parent is gone, failed: {error}");
                }
                Err(error) => log::warn!("report: closing {child} failed: {error:?}"),
                Ok(_) => {}
            }
        }
    }

    /// What a turn's end says: the agent's status text, then Claude's last
    /// message, else the end of the screen (a permission dialog shows there).
    async fn turn_body(&self, child: &str, kind: ReportEventKind) -> String {
        let custom = self.state.lock().links.custom_of(child).map(str::to_string);
        let last_message = match kind {
            ReportEventKind::Done | ReportEventKind::Error => self.last_claude_message(child).await,
            _ => None,
        };
        let detail = match last_message {
            Some(message) => message,
            None => self
                .screen(child)
                .await
                .map(|screen| screen_tail(&screen, SCREEN_TAIL_LINES, None))
                .unwrap_or_default(),
        };
        match custom {
            Some(custom) if !detail.contains(custom.as_str()) => format!("{custom}\n\n{detail}"),
            _ => detail,
        }
    }

    async fn last_claude_message(&self, child: &str) -> Option<String> {
        let page = self
            .claude_hub
            .transcript(child, None, None, None, None)
            .await
            .ok()?;
        page.entries.into_iter().rev().find_map(|entry| match entry {
            ApiClaudeEntry::Assistant { text, .. } if !text.trim().is_empty() => Some(text),
            _ => None,
        })
    }

    fn flush_soon(self: &Arc<Self>) {
        let hub = Arc::clone(self);
        drop(tokio::spawn(async move { hub.flush().await }));
    }

    /// Deliver the reports that are due.
    async fn flush(&self) {
        let due = self.state.lock().links.take_due(Instant::now());
        for due in due {
            let message = due
                .events
                .iter()
                .map(|event| event.message.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            match self.deliver(&due.parent, message).await {
                Some(delivery) => self.state.lock().links.delivered(&due.events, delivery),
                None => self.state.lock().links.retry(due.events),
            }
        }
    }

    /// Put `message` into the parent's input: through Okena's Claude remote
    /// when Claude runs there, pasted when another agent does, else nowhere
    /// (`Kept`): typing into a shell would run it. `None`: try again later.
    async fn deliver(&self, parent: &str, message: String) -> Option<ReportDelivery> {
        let info = match self.terminal_info(parent).await {
            Ok(info) => info,
            Err(ReportError::TerminalNotFound(_)) => return Some(ReportDelivery::Kept),
            Err(_) => return None,
        };
        let claude_parent = info.foreground_command.as_deref() == Some("claude")
            || self
                .claude_hub
                .session_terminals()
                .iter()
                .any(|terminal| terminal == parent);
        if claude_parent {
            match self.claude_hub.send_prompt(parent, message.clone(), None).await {
                Ok(ApiClaudePromptDelivery::Channel) => return Some(ReportDelivery::Channel),
                Ok(ApiClaudePromptDelivery::Pty) => return Some(ReportDelivery::Pty),
                Ok(ApiClaudePromptDelivery::Queued) => return Some(ReportDelivery::Queued),
                Err(ClaudeRemoteError::Busy | ClaudeRemoteError::QueueFull) => return None,
                Err(ClaudeRemoteError::TerminalNotFound(_)) => {
                    return Some(ReportDelivery::Kept);
                }
                // Claude Remote is off for the pane or Claude left it.
                Err(_) => {}
            }
        }
        if !takes_typed_reports(&info) {
            return Some(ReportDelivery::Kept);
        }
        match self.paste_and_submit(parent, message).await {
            Ok(()) => Some(ReportDelivery::Typed),
            Err(error) => {
                log::warn!("report: typing into {parent} failed: {error:?}");
                None
            }
        }
    }

    async fn paste_and_submit(&self, terminal_id: &str, text: String) -> Result<(), ReportError> {
        expect_ok(
            self.run(RemoteCommand::PastePath {
                terminal_id: terminal_id.to_string(),
                text,
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

    async fn terminal_info(&self, terminal_id: &str) -> Result<ClaudeTerminalInfo, ReportError> {
        match self
            .run(RemoteCommand::ClaudeTerminalInfo {
                terminal_id: terminal_id.to_string(),
                probe_foreground: true,
                read_screen: false,
            })
            .await?
        {
            CommandResult::Ok(Some(value)) => serde_json::from_value(value)
                .map_err(|error| ReportError::Invalid(format!("invalid terminal info: {error}"))),
            CommandResult::Err(message) => Err(ReportError::TerminalNotFound(message)),
            _ => Err(ReportError::Invalid("unexpected terminal info reply".to_string())),
        }
    }

    async fn screen(&self, terminal_id: &str) -> Result<String, ReportError> {
        let entries = self
            .probe(vec![terminal_id.to_string()], vec![terminal_id.to_string()])
            .await?;
        Ok(entries
            .into_iter()
            .next()
            .and_then(|entry| entry.screen)
            .unwrap_or_default())
    }

    async fn probe(
        &self,
        terminal_ids: Vec<String>,
        screen_of: Vec<String>,
    ) -> Result<Vec<ReportProbeEntry>, ReportError> {
        match self
            .run(RemoteCommand::ReportProbe {
                terminal_ids,
                screen_of,
            })
            .await?
        {
            CommandResult::Ok(Some(value)) => serde_json::from_value(value)
                .map_err(|error| ReportError::Invalid(format!("invalid report probe: {error}"))),
            CommandResult::Err(message) => Err(ReportError::Invalid(message)),
            _ => Err(ReportError::Invalid("unexpected report probe reply".to_string())),
        }
    }

    async fn run(&self, command: RemoteCommand) -> Result<CommandResult, ReportError> {
        let (reply, result) = oneshot::channel();
        self.bridge_tx
            .send(BridgeMessage {
                command,
                reply: Some(reply),
            })
            .await
            .map_err(|_| ReportError::BridgeUnavailable)?;
        result.await.map_err(|_| ReportError::BridgeUnavailable)
    }
}

/// Ends a wait however its request ends (a client that disconnects drops it).
struct EndWait<'a> {
    hub: &'a ReportHub,
    parent: &'a str,
}

impl Drop for EndWait<'_> {
    fn drop(&mut self) {
        self.hub
            .state
            .lock()
            .links
            .end_wait(self.parent, Instant::now());
    }
}

/// A coding agent runs in the foreground: a known one, or any program other
/// than a shell that reports its own agent status.
fn takes_typed_reports(info: &ClaudeTerminalInfo) -> bool {
    let Some(command) = info.foreground_command.as_deref() else {
        return false;
    };
    let command = command.trim_start_matches('-');
    if SHELLS.contains(&command) {
        return false;
    }
    AGENT_COMMANDS.contains(&command) || info.lifecycle.is_some()
}

fn expect_ok(result: CommandResult) -> Result<(), ReportError> {
    match result {
        CommandResult::Err(message) => Err(ReportError::TerminalNotFound(message)),
        _ => Ok(()),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::bridge_channel;
    use okena_core::agent_status::AgentLifecycle;
    use std::collections::HashMap;
    use tokio::sync::{mpsc, watch};

    /// The fake daemon's terminals: probe entries by id, and the foreground
    /// program of each.
    #[derive(Default)]
    struct FakeDaemon {
        entries: Mutex<HashMap<String, ReportProbeEntry>>,
        foreground: Mutex<HashMap<String, String>>,
    }

    impl FakeDaemon {
        fn add(&self, id: &str, foreground: Option<&str>) {
            self.entries.lock().insert(
                id.to_string(),
                ReportProbeEntry {
                    terminal_id: id.to_string(),
                    project_id: "p".to_string(),
                    name: Some(format!("{id}-name")),
                    lifecycle: None,
                    custom: None,
                    lifecycle_changes: 0,
                    screen: Some(format!("$ make; okena {EXIT_REPORT_MARK} $?\nbuilt {id}\n")),
                },
            );
            if let Some(foreground) = foreground {
                self.foreground
                    .lock()
                    .insert(id.to_string(), foreground.to_string());
            }
        }

        fn set_lifecycle(&self, id: &str, lifecycle: AgentLifecycle) {
            let mut entries = self.entries.lock();
            let entry = entries.get_mut(id).unwrap();
            entry.lifecycle = Some(lifecycle);
            entry.lifecycle_changes += 1;
        }
    }

    /// A hub whose daemon is `daemon`; every PTY write and close shows up on
    /// the receiver as `"paste <terminal> <text>"`, `"enter <terminal>"` or
    /// `"close <terminal>"`.
    fn hub_with(daemon: Arc<FakeDaemon>) -> (Arc<ReportHub>, mpsc::UnboundedReceiver<String>) {
        let (bridge_tx, bridge_rx) = bridge_channel();
        let claude_hub = ClaudeRemoteHub::new(bridge_tx.clone(), Arc::new(watch::channel(0).0));
        let hub = ReportHub::new(bridge_tx, claude_hub);
        let (pty_tx, pty_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(message) = bridge_rx.recv().await {
                let result = match message.command {
                    RemoteCommand::ReportProbe {
                        terminal_ids,
                        screen_of,
                    } => {
                        let entries = daemon.entries.lock();
                        let found: Vec<ReportProbeEntry> = terminal_ids
                            .iter()
                            .filter_map(|id| entries.get(id))
                            .map(|entry| ReportProbeEntry {
                                screen: entry
                                    .screen
                                    .clone()
                                    .filter(|_| screen_of.contains(&entry.terminal_id)),
                                ..entry.clone()
                            })
                            .collect();
                        CommandResult::Ok(Some(serde_json::to_value(found).unwrap()))
                    }
                    RemoteCommand::ClaudeTerminalInfo { terminal_id, .. } => {
                        if daemon.entries.lock().contains_key(&terminal_id) {
                            let info = ClaudeTerminalInfo {
                                foreground_command: daemon
                                    .foreground
                                    .lock()
                                    .get(&terminal_id)
                                    .cloned(),
                                ..ClaudeTerminalInfo::default()
                            };
                            CommandResult::Ok(Some(serde_json::to_value(info).unwrap()))
                        } else {
                            CommandResult::Err(format!("terminal not found: {terminal_id}"))
                        }
                    }
                    RemoteCommand::PastePath { terminal_id, text } => {
                        let _ = pty_tx.send(format!("paste {terminal_id} {text}"));
                        CommandResult::Ok(None)
                    }
                    RemoteCommand::Action(ActionRequest::SendSpecialKey {
                        terminal_id, ..
                    }) => {
                        let _ = pty_tx.send(format!("enter {terminal_id}"));
                        CommandResult::Ok(None)
                    }
                    RemoteCommand::Action(ActionRequest::CloseTerminal {
                        terminal_id, ..
                    }) => {
                        daemon.entries.lock().remove(&terminal_id);
                        let _ = pty_tx.send(format!("close {terminal_id}"));
                        CommandResult::Ok(None)
                    }
                    // Not a Claude transcript.
                    _ => CommandResult::Err("unexpected".to_string()),
                };
                if let Some(reply) = message.reply {
                    let _ = reply.send(result);
                }
            }
        });
        (hub, pty_rx)
    }

    fn link_request(child: &str, parent: &str, owned: bool) -> ApiReportLinkRequest {
        ApiReportLinkRequest {
            child_terminal_id: child.to_string(),
            parent_terminal_id: parent.to_string(),
            owned,
        }
    }

    fn exit_report(child: &str, code: i32) -> ApiReportRequest {
        ApiReportRequest {
            terminal_id: child.to_string(),
            event: ReportEventKind::Exit,
            exit_code: Some(code),
            text: String::new(),
        }
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<String>) -> String {
        tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a PTY write")
            .expect("open channel")
    }

    #[tokio::test]
    async fn an_exit_is_typed_into_an_agent_parent_with_the_screen_tail() {
        let daemon = Arc::new(FakeDaemon::default());
        daemon.add("parent", Some("codex"));
        daemon.add("child", None);
        let (hub, mut pty) = hub_with(daemon);
        hub.link(&link_request("child", "parent", false)).await.unwrap();

        let event = hub.report(&exit_report("child", 2)).await.unwrap();
        assert_eq!(event.body, "built child");
        assert_eq!(
            next(&mut pty).await,
            "paste parent <okena-report from=\"child-name\" terminal=\"child\" event=\"exit\" code=\"2\">\nbuilt child\n</okena-report>"
        );
        assert_eq!(next(&mut pty).await, "enter parent");
        let child = &hub.children(Some("parent")).children[0];
        assert_eq!(
            child.last_event.as_ref().unwrap().delivery,
            Some(ReportDelivery::Typed)
        );
    }

    #[tokio::test]
    async fn a_shell_parent_is_never_typed_into() {
        let daemon = Arc::new(FakeDaemon::default());
        daemon.add("parent", Some("zsh"));
        daemon.add("child", None);
        let (hub, mut pty) = hub_with(daemon);
        hub.link(&link_request("child", "parent", false)).await.unwrap();
        hub.report(&exit_report("child", 0)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(pty.try_recv().is_err());
        assert_eq!(
            hub.children(None).children[0]
                .last_event
                .as_ref()
                .unwrap()
                .delivery,
            Some(ReportDelivery::Kept)
        );
        // The kept report still reaches a wait.
        let waited = hub
            .wait(&ApiReportWaitRequest {
                parent_terminal_id: "parent".to_string(),
                children: Vec::new(),
                all: false,
                timeout_ms: 1000,
            })
            .await
            .unwrap();
        assert_eq!(waited.events.len(), 1);
        assert_eq!(waited.events[0].delivery, Some(ReportDelivery::Wait));
    }

    #[tokio::test]
    async fn a_wait_receives_the_report_instead_of_the_input() {
        let daemon = Arc::new(FakeDaemon::default());
        daemon.add("parent", Some("codex"));
        daemon.add("child", None);
        let (hub, mut pty) = hub_with(daemon);
        hub.link(&link_request("child", "parent", false)).await.unwrap();
        hub.note_input("child");

        let waiting = {
            let hub = Arc::clone(&hub);
            tokio::spawn(async move {
                hub.wait(&ApiReportWaitRequest {
                    parent_terminal_id: "parent".to_string(),
                    children: vec!["child".to_string()],
                    all: false,
                    timeout_ms: 5000,
                })
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut report = exit_report("child", 0);
        report.event = ReportEventKind::Report;
        report.text = "PR #12 opened".to_string();
        hub.report(&report).await.unwrap();
        let waited = waiting.await.unwrap().unwrap();
        assert_eq!(waited.events[0].body, "PR #12 opened");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(pty.try_recv().is_err(), "nothing typed into the waiting parent");
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_times_out_naming_the_children_it_waits_for() {
        let daemon = Arc::new(FakeDaemon::default());
        daemon.add("parent", None);
        daemon.add("child", None);
        let (hub, _pty) = hub_with(daemon);
        hub.link(&link_request("child", "parent", false)).await.unwrap();
        hub.note_input("child");
        let waited = hub
            .wait(&ApiReportWaitRequest {
                parent_terminal_id: "parent".to_string(),
                children: Vec::new(),
                all: true,
                timeout_ms: 2000,
            })
            .await
            .unwrap();
        assert!(waited.events.is_empty());
        assert_eq!(waited.waiting, vec!["child".to_string()]);
    }

    #[tokio::test]
    async fn the_poll_reports_a_finished_turn_and_a_closed_child() {
        let daemon = Arc::new(FakeDaemon::default());
        daemon.add("parent", Some("codex"));
        daemon.add("child", None);
        let (hub, mut pty) = hub_with(Arc::clone(&daemon));
        hub.link(&link_request("child", "parent", false)).await.unwrap();

        daemon.set_lifecycle("child", AgentLifecycle::Working);
        hub.poll().await;
        daemon.set_lifecycle("child", AgentLifecycle::Done);
        hub.poll().await;
        let typed = next(&mut pty).await;
        assert!(
            typed.starts_with("paste parent <okena-report from=\"child-name\" terminal=\"child\" event=\"done\">\n"),
            "{typed}"
        );
        assert_eq!(next(&mut pty).await, "enter parent");

        daemon.entries.lock().remove("child");
        hub.poll().await;
        let events = hub.children(Some("parent")).children;
        assert_eq!(
            events[0].last_event.as_ref().unwrap().event,
            ReportEventKind::Closed
        );
    }

    #[tokio::test]
    async fn an_owned_child_closes_with_its_parent() {
        let daemon = Arc::new(FakeDaemon::default());
        daemon.add("parent", None);
        daemon.add("owned", None);
        daemon.add("kept", None);
        let (hub, mut pty) = hub_with(Arc::clone(&daemon));
        hub.link(&link_request("owned", "parent", true)).await.unwrap();
        hub.link(&link_request("kept", "parent", false)).await.unwrap();
        daemon.entries.lock().remove("parent");
        hub.poll().await;
        assert_eq!(next(&mut pty).await, "close owned");
        assert!(hub.children(None).children.is_empty());
        assert!(daemon.entries.lock().contains_key("kept"));
    }

    #[tokio::test]
    async fn only_a_linked_child_reports_and_links_need_live_terminals() {
        let daemon = Arc::new(FakeDaemon::default());
        daemon.add("parent", None);
        daemon.add("child", None);
        let (hub, _pty) = hub_with(daemon);
        assert!(matches!(
            hub.report(&exit_report("child", 0)).await,
            Err(ReportError::NotLinked)
        ));
        assert!(matches!(
            hub.link(&link_request("child", "nowhere", false)).await,
            Err(ReportError::TerminalNotFound(_))
        ));
        assert!(matches!(
            hub.link(&link_request("child", "child", false)).await,
            Err(ReportError::Invalid(_))
        ));
    }

    #[test]
    fn typed_reports_go_only_to_agents() {
        let info = |command: Option<&str>, lifecycle| ClaudeTerminalInfo {
            foreground_command: command.map(str::to_string),
            lifecycle,
            ..ClaudeTerminalInfo::default()
        };
        assert!(takes_typed_reports(&info(Some("codex"), None)));
        assert!(takes_typed_reports(&info(Some("node"), Some(AgentLifecycle::Idle))));
        assert!(!takes_typed_reports(&info(Some("node"), None)));
        assert!(!takes_typed_reports(&info(Some("vim"), None)));
        assert!(!takes_typed_reports(&info(Some("-zsh"), Some(AgentLifecycle::Done))));
        assert!(!takes_typed_reports(&info(None, Some(AgentLifecycle::Done))));
    }
}
