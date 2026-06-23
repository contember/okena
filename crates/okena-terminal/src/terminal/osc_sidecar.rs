use alacritty_terminal::vte::Perform;
use base64::Engine as _;
use okena_core::agent_session::AgentSession;
use okena_core::agent_status::{AgentLifecycle, AgentStatus};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::app_version::app_version;
use super::transport::TerminalTransport;
use super::types::{TerminalProgress, TerminalProgressState};

/// A desktop notification requested by the shell via an OSC escape.
///
/// Three source sequences map onto this type:
/// - iTerm2-style `OSC 9 ; <body>` — a single message with no title.
/// - `OSC 777 ; notify ; <title> ; <body>` (urxvt / foot / wezterm) — a
///   proper title + body pair.
/// - `OSC 99` — the kitty notification protocol; title and body arrive across
///   one or more chunks (see [`Osc99Accumulator`]).
///
/// The GPUI thread drains these via [`super::Terminal::take_pending_notifications`]
/// and turns them into native desktop notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalNotification {
    /// `None` for `OSC 9` (and title-less `OSC 99`), where the consumer picks
    /// a title (e.g. the project name); `Some` for `OSC 777` and `OSC 99`
    /// notifications that carry a distinct title and body.
    pub title: Option<String>,
    pub body: String,
}

/// Cap on accumulated `OSC 99` title/body length and number of in-flight
/// (incomplete, `d=0`) notifications, to bound memory against a misbehaving
/// stream that never sends a final chunk.
const OSC99_MAX_FIELD_LEN: usize = 4096;
const OSC99_MAX_PENDING: usize = 32;

/// Reassembly buffer for a chunked `OSC 99` notification keyed by its `i=` id.
#[derive(Default)]
struct Osc99Accumulator {
    title: String,
    body: String,
}

/// Side-channel VTE parser for sequences that alacritty_terminal either
/// ignores or answers in a way Okena wants to override. Runs on the same
/// byte stream as the main `Processor` so we can observe shell-reported
/// state (OSC 7 cwd, later OSC 133) and answer terminal-identification
/// queries (XTVERSION) without patching upstream.
pub(crate) struct OscSidecar {
    parser: alacritty_terminal::vte::Parser,
    perform: SidecarPerform,
}

impl OscSidecar {
    pub(super) fn new(
        reported_cwd: Arc<Mutex<Option<String>>>,
        pending_notifications: Arc<Mutex<Vec<TerminalNotification>>>,
        progress: Arc<Mutex<Option<TerminalProgress>>>,
        agent_status: Arc<Mutex<Option<AgentStatus>>>,
        remote_dirty: Arc<AtomicBool>,
        agent_session: Arc<Mutex<Option<AgentSession>>>,
        agent_session_dirty: Arc<AtomicBool>,
        transport: Arc<dyn TerminalTransport>,
        terminal_id: String,
    ) -> Self {
        Self {
            parser: alacritty_terminal::vte::Parser::new(),
            perform: SidecarPerform {
                reported_cwd,
                pending_notifications,
                progress,
                agent_status,
                remote_dirty,
                agent_session,
                agent_session_dirty,
                transport,
                terminal_id,
                osc99_pending: HashMap::new(),
            },
        }
    }

    pub(super) fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.perform, bytes);
    }
}

struct SidecarPerform {
    reported_cwd: Arc<Mutex<Option<String>>>,
    /// `OSC 9` / `OSC 777` / `OSC 99` notifications, drained by the GPUI thread
    /// in the PTY event loop (same model as `pending_clipboard`).
    pending_notifications: Arc<Mutex<Vec<TerminalNotification>>>,
    /// Active `OSC 9 ; 4` (ConEmu / Windows Terminal) progress report, or
    /// `None` when cleared (`st=0`). Overwritten on each progress sequence and
    /// read by the GPUI thread via `Terminal::progress`.
    progress: Arc<Mutex<Option<TerminalProgress>>>,
    /// Latest agent status reported via the agent-status OSC, or `None` when
    /// never set / cleared (`st=clear`). Overwritten on each sequence and read
    /// by the GPUI thread via `Terminal::agent_status`.
    agent_status: Arc<Mutex<Option<AgentStatus>>>,
    /// One-shot "remote-visible state changed since last drain" edge, shared
    /// with `Terminal`. Set whenever a runtime-only signal here changes state
    /// remote clients should see (currently agent status), consumed by the PTY
    /// event loop (`take_remote_dirty`) to bump the remote `state_version` so
    /// remote / mobile clients re-fetch. Generic by design — not agent-specific.
    /// Mirrors `bell_pending`.
    remote_dirty: Arc<AtomicBool>,
    /// Sticky agent session identity (`agent` + `session_id` + `transcript_path`)
    /// captured from the `lbl=` of an agent-status OSC. Unlike `agent_status`
    /// it survives `st=clear`; the app layer persists it for resume + stats.
    agent_session: Arc<Mutex<Option<AgentSession>>>,
    /// One-shot edge set when `agent_session` changes; drained by the PTY event
    /// loop to persist the session. Mirrors `remote_dirty`.
    agent_session_dirty: Arc<AtomicBool>,
    transport: Arc<dyn TerminalTransport>,
    terminal_id: String,
    /// In-progress `OSC 99` notifications keyed by `i=` id, awaiting their
    /// final (`d=1`) chunk. GPUI-thread only (process_output is serialized).
    osc99_pending: HashMap<String, Osc99Accumulator>,
}

impl SidecarPerform {
    /// Handle the kitty notification protocol: `OSC 99 ; metadata ; payload`.
    ///
    /// `metadata` is colon-separated `key=value` pairs (ASCII). We care about:
    /// `i` (id, groups chunks), `d` (0 = more chunks, default 1 = complete),
    /// `p` (`title` default / `body` / others we ignore), `e` (1 = payload is
    /// base64), and `c=1` (close). Title/body payloads accumulate per id until
    /// a final chunk, then emit one [`TerminalNotification`]. Non-text payload
    /// types (queries, icons, buttons, …) are ignored.
    fn handle_osc99(&mut self, params: &[&[u8]]) {
        let metadata: &[u8] = params.get(1).copied().unwrap_or(b"");
        // Payload may contain ';' when unencoded → rejoin the split tail.
        let payload_raw: String = params
            .get(2..)
            .unwrap_or(&[])
            .iter()
            .filter_map(|p| std::str::from_utf8(p).ok())
            .collect::<Vec<_>>()
            .join(";");

        let mut id = String::new();
        let mut done = true; // `d` defaults to 1 (complete)
        let mut ptype = "title"; // `p` defaults to title
        let mut base64 = false;
        let mut close = false;
        for field in metadata.split(|&b| b == b':') {
            let mut kv = field.splitn(2, |&b| b == b'=');
            let key = kv.next().unwrap_or(b"");
            let val = kv.next().unwrap_or(b"");
            match key {
                b"i" => id = String::from_utf8_lossy(val).into_owned(),
                b"d" => done = val != b"0",
                b"p" => ptype = std::str::from_utf8(val).unwrap_or("title"),
                b"e" => base64 = val == b"1",
                b"c" => close = val == b"1",
                _ => {}
            }
        }

        // Close request: drop any in-progress accumulation, display nothing.
        if close || ptype == "close" {
            self.osc99_pending.remove(&id);
            return;
        }

        // Only title/body payloads build a displayed notification. Other types
        // (query `?`, icon, buttons, alive, …) don't contribute text, but a
        // final chunk still completes whatever was accumulated.
        if ptype != "title" && ptype != "body" {
            if done {
                self.finish_osc99(&id);
            }
            return;
        }

        let payload = if base64 {
            // kitty uses standard base64; tolerate a missing-pad variant too.
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(payload_raw.as_bytes())
                .or_else(|_| {
                    base64::engine::general_purpose::STANDARD_NO_PAD
                        .decode(payload_raw.as_bytes())
                });
            match decoded.ok().and_then(|b| String::from_utf8(b).ok()) {
                Some(s) => s,
                None => return, // undecodable chunk — drop it
            }
        } else {
            payload_raw
        };

        // Bound memory: ignore a brand-new id once too many are in flight.
        if !self.osc99_pending.contains_key(&id)
            && self.osc99_pending.len() >= OSC99_MAX_PENDING
        {
            return;
        }
        let acc = self.osc99_pending.entry(id.clone()).or_default();
        let target = if ptype == "body" { &mut acc.body } else { &mut acc.title };
        if target.len() + payload.len() <= OSC99_MAX_FIELD_LEN {
            target.push_str(&payload);
        }

        if done {
            self.finish_osc99(&id);
        }
    }

    /// Finalize the accumulated `OSC 99` notification for `id` and queue it.
    /// kitty's "title" is the headline, so a title-only notification reads like
    /// `OSC 9` (the consumer supplies the title, e.g. the project name).
    fn finish_osc99(&mut self, id: &str) {
        let Some(acc) = self.osc99_pending.remove(id) else {
            return;
        };
        let title = acc.title.trim();
        let body = acc.body.trim();
        let notification = if !body.is_empty() {
            TerminalNotification {
                title: (!title.is_empty()).then(|| title.to_string()),
                body: body.to_string(),
            }
        } else if !title.is_empty() {
            TerminalNotification { title: None, body: title.to_string() }
        } else {
            return; // nothing to show
        };
        self.pending_notifications.lock().push(notification);
    }

    /// Handle the ConEmu / Windows Terminal progress protocol
    /// `OSC 9 ; 4 ; st ; pr` (also spoken by WezTerm, Ghostty, Kitty, …).
    ///
    /// `st` selects the state and `pr` an optional 0..=100 percentage:
    /// - `st=0` → clear progress (`pr` ignored).
    /// - `st=1` → [`TerminalProgressState::Normal`] at `pr` percent
    ///   (clamped to `0..=100`).
    /// - `st=2` → [`TerminalProgressState::Error`]; `pr` optional, keeping the
    ///   previous percent when absent.
    /// - `st=3` → [`TerminalProgressState::Indeterminate`] (`pr` ignored).
    /// - `st=4` → [`TerminalProgressState::Paused`]; `pr` optional like `st=2`.
    ///
    /// A missing or unparseable `st` (or any value outside `0..=4`) leaves the
    /// current progress untouched, so a malformed sequence can never panic or
    /// spuriously clear an active bar. `params[2]` is `st`, `params[3]` is `pr`.
    fn handle_osc9_progress(&mut self, params: &[&[u8]]) {
        let Some(st) = params
            .get(2)
            .and_then(|p| std::str::from_utf8(p).ok())
            .and_then(|s| s.trim().parse::<u8>().ok())
        else {
            return;
        };
        // `pr` is optional and only meaningful for some states.
        let pr = params
            .get(3)
            .and_then(|p| std::str::from_utf8(p).ok())
            .and_then(|s| s.trim().parse::<u8>().ok());

        let mut slot = self.progress.lock();
        let previous = slot.map(|p| p.value).unwrap_or(0);
        *slot = match st {
            // `st=0` removes the progress entirely.
            0 => None,
            1 => Some(TerminalProgress {
                state: TerminalProgressState::Normal,
                value: pr.unwrap_or(0).min(100),
            }),
            // Error / paused keep the previous percent when `pr` is omitted.
            2 => Some(TerminalProgress {
                state: TerminalProgressState::Error,
                value: pr.unwrap_or(previous).min(100),
            }),
            3 => Some(TerminalProgress {
                state: TerminalProgressState::Indeterminate,
                value: 0,
            }),
            4 => Some(TerminalProgress {
                state: TerminalProgressState::Paused,
                value: pr.unwrap_or(previous).min(100),
            }),
            // Unknown state — ignore gracefully, leaving the bar as-is.
            _ => return,
        };
    }

    /// Handle Okena's agent-status protocol
    /// `OSC 9001 ; st=<state> [; msg=<b64>] [; lbl=<b64-json>]`.
    ///
    /// An AI coding agent (or a thin hook) pushes its own lifecycle here:
    /// - `st=working|blocked|done|idle` sets the lifecycle.
    /// - `st=clear` removes the status entirely.
    /// - `msg=` carries base64(UTF-8) free-form text (e.g. "running tests 3/5").
    /// - `lbl=` carries base64 of a flat JSON `{"k":"v"}` object of extras.
    ///
    /// `msg`/`lbl` are base64-encoded so their values stay `;`/ST-safe (the VTE
    /// parser splits OSC params on `;`). A missing or unknown `st` leaves the
    /// current status untouched — a malformed sequence can never panic or
    /// spuriously clear an active status.
    ///
    /// On a transition *into* a notifying state (`blocked` / `done`) a
    /// [`TerminalNotification`] is queued so the GPUI layer raises a desktop
    /// notification — reusing the same drain + focused-pane suppression as
    /// `OSC 9` notifications.
    fn handle_agent_status(&mut self, params: &[&[u8]]) {
        // params[0] is the OSC number; the rest are `key=value` pairs.
        let mut st: Option<&str> = None;
        let mut msg_b64: Option<&str> = None;
        let mut lbl_b64: Option<&str> = None;
        for kv in &params[1..] {
            let Ok(s) = std::str::from_utf8(kv) else {
                continue;
            };
            let mut it = s.splitn(2, '=');
            let key = it.next().unwrap_or("").trim();
            let val = it.next().unwrap_or("").trim();
            match key {
                "st" => st = Some(val),
                "msg" => msg_b64 = Some(val),
                "lbl" => lbl_b64 = Some(val),
                _ => {}
            }
        }

        let Some(st) = st else {
            log::debug!(
                "agent-status[{}]: OSC 9001 with no st= field — ignored",
                self.terminal_id
            );
            return; // no state field — nothing to do
        };

        if st == "clear" {
            let mut slot = self.agent_status.lock();
            let changed = slot.is_some();
            *slot = None;
            drop(slot);
            if changed {
                self.remote_dirty.store(true, Ordering::Relaxed);
            }
            log::debug!(
                "agent-status[{}]: clear (changed={changed})",
                self.terminal_id
            );
            return;
        }
        let Some(lifecycle) = AgentLifecycle::from_token(st) else {
            log::debug!(
                "agent-status[{}]: unknown st={st:?} — ignored, status left as-is",
                self.terminal_id
            );
            return; // unknown state — ignore, leave current status as-is
        };

        // Decode the agent-supplied fields BEFORE taking the lock — base64 of a
        // hostile multi-MB payload must not run while holding `agent_status`.
        // `new_clamped` bounds the decoded sizes (a pane could otherwise pin
        // unbounded memory we then re-serialize to every remote client) and
        // maps an empty `msg=` to `None` so a notifying state keeps its default
        // body instead of an empty string.
        let custom = msg_b64.and_then(decode_osc_base64);
        let labels = lbl_b64
            .and_then(decode_osc_base64)
            .map(|json| okena_core::agent_status::parse_labels_json(&json))
            .unwrap_or_default();
        let new_status = AgentStatus::new_clamped(lifecycle, custom, labels);

        // Durable session capture (resume + transcript stats). The `agent` +
        // `session_id` labels are the pane's *sticky* session identity, NOT part
        // of the ephemeral status — record them on a separate field that
        // survives `st=clear`, so the pane can later offer to resume or show
        // stats. Require both (without an `agent` id we don't know which harness
        // could resume it) and validate the id looks like a UUID: it's
        // untrusted in-band data that may reach a resume command.
        if let (Some(agent), Some(sid)) = (
            new_status.labels.get("agent"),
            new_status.labels.get("session_id"),
        ) {
            if okena_core::agent_session::is_uuid_like(sid) {
                let session = AgentSession {
                    agent: agent.clone(),
                    session_id: sid.clone(),
                    transcript_path: new_status.labels.get("transcript_path").cloned(),
                };
                let mut s = self.agent_session.lock();
                if s.as_ref() != Some(&session) {
                    *s = Some(session);
                    self.agent_session_dirty.store(true, Ordering::Relaxed);
                }
            }
        }

        // Notify only on a *transition* into a notifying state, so repeated
        // identical reports don't ping. Decide while holding the slot (we need
        // the previous value), then release it before touching the notification
        // queue to keep lock scopes from overlapping.
        let mut slot = self.agent_status.lock();
        let previous_lifecycle = slot.as_ref().map(|s| s.lifecycle);
        let notify_body = (previous_lifecycle != Some(lifecycle) && lifecycle.notifies())
            .then(|| new_status.custom.clone().unwrap_or_else(|| agent_default_body(lifecycle)));
        // Any change to the stored status (lifecycle, custom text, or labels)
        // is worth pushing to remote clients.
        let changed = slot.as_ref() != Some(&new_status);
        *slot = Some(new_status);
        drop(slot);

        log::debug!(
            "agent-status[{}]: {:?} -> {:?} (changed={changed}, notify={})",
            self.terminal_id,
            previous_lifecycle,
            lifecycle,
            notify_body.is_some()
        );

        if changed {
            self.remote_dirty.store(true, Ordering::Relaxed);
        }
        if let Some(body) = notify_body {
            self.pending_notifications
                .lock()
                .push(TerminalNotification { title: None, body });
        }
    }
}

/// Decode a base64(UTF-8) OSC field, tolerating a missing-pad variant (same
/// leniency as the `OSC 99` payload decoder). Returns `None` on undecodable or
/// non-UTF-8 input so a malformed field is simply dropped.
fn decode_osc_base64(s: &str) -> Option<String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(s.as_bytes()))
        .ok()?;
    String::from_utf8(decoded).ok()
}

/// Default notification body when an agent reports a notifying state without
/// its own `msg=` text.
fn agent_default_body(lifecycle: AgentLifecycle) -> String {
    match lifecycle {
        AgentLifecycle::Blocked => "Agent needs your input".to_string(),
        AgentLifecycle::Done => "Agent finished".to_string(),
        // Non-notifying states never reach here (guarded by `notifies()`).
        AgentLifecycle::Working | AgentLifecycle::Idle => String::new(),
    }
}

impl Perform for SidecarPerform {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.len() < 2 {
            return;
        }
        match params[0] {
            b"7" => {
                // Rejoin with `;` in case an unencoded semicolon in the URI
                // caused the parser to split the value across multiple
                // params. Well-behaved shell scripts percent-encode `;`,
                // but be forgiving.
                let uri: String = params[1..]
                    .iter()
                    .filter_map(|p| std::str::from_utf8(p).ok())
                    .collect::<Vec<_>>()
                    .join(";");
                if let Some(path) = parse_osc7_file_uri(&uri) {
                    *self.reported_cwd.lock() = Some(path);
                }
            }
            b"1337" => {
                // iTerm2's proprietary `OSC 1337 ; key=value` channel. We only
                // honour `CurrentDir=<path>`, which some shell integrations emit
                // instead of `OSC 7`; it feeds the *same* `reported_cwd` so the
                // sidebar / "new tab here" / cwd tracking work regardless of
                // which sequence the shell speaks. All other 1337 subcommands
                // (RemoteHost, SetUserVar, File, …) have no consumer here and
                // are deliberately ignored rather than parsed into dead code.
                //
                // Unlike OSC 7, the value is a raw filesystem path — not a
                // `file://` URI and not percent-encoded. A path may legitimately
                // contain `;`, so rejoin the split tail like the OSC 7 / 777
                // arms in case the parser broke the value apart.
                let payload: String = params[1..]
                    .iter()
                    .filter_map(|p| std::str::from_utf8(p).ok())
                    .collect::<Vec<_>>()
                    .join(";");
                if let Some(value) = payload.strip_prefix("CurrentDir=") {
                    let path = value.trim();
                    if !path.is_empty() {
                        *self.reported_cwd.lock() = Some(path.to_string());
                    }
                }
            }
            b"9" => {
                // `OSC 9` is overloaded. The `OSC 9 ; 4 ; st ; pr` subtype is
                // ConEmu's / Windows Terminal's progress-bar protocol; route it
                // to the progress handler. Everything else is an iTerm2-style
                // notification: `OSC 9 ; <message>`.
                if params.get(1).copied() == Some(b"4".as_slice()) {
                    self.handle_osc9_progress(params);
                    return;
                }
                let message: String = params[1..]
                    .iter()
                    .filter_map(|p| std::str::from_utf8(p).ok())
                    .collect::<Vec<_>>()
                    .join(";");
                let message = message.trim();
                if !message.is_empty() {
                    self.pending_notifications.lock().push(TerminalNotification {
                        title: None,
                        body: message.to_string(),
                    });
                }
            }
            b"777" => {
                // urxvt-style rich notification: `OSC 777 ; notify ; title ; body`.
                // 777 also carries unrelated subcommands (e.g. precmd/preexec
                // from some prompt frameworks) — only `notify` is ours.
                if params.get(1).copied() != Some(b"notify".as_slice()) {
                    return;
                }
                let title = params
                    .get(2)
                    .and_then(|p| std::str::from_utf8(p).ok())
                    .unwrap_or("")
                    .trim();
                // The body may legitimately contain semicolons, so rejoin the
                // tail. `get(3..)` avoids a panic when no body field is present.
                let body: String = params
                    .get(3..)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(|p| std::str::from_utf8(p).ok())
                    .collect::<Vec<_>>()
                    .join(";");
                let body = body.trim();
                if !body.is_empty() {
                    self.pending_notifications.lock().push(TerminalNotification {
                        title: (!title.is_empty()).then(|| title.to_string()),
                        body: body.to_string(),
                    });
                }
            }
            b"99" => self.handle_osc99(params),
            // Okena's private agent-status protocol; see `handle_agent_status`.
            b"9001" => self.handle_agent_status(params),
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &alacritty_terminal::vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // XTVERSION query: `CSI > Ps q`. Per xterm ctlseqs, only Ps=0 (or
        // omitted) asks for the terminal name+version; other Ps values
        // belong to unrelated private CSI sequences we must not answer.
        if action != 'q' || intermediates != [b'>'] {
            return;
        }
        let ps = params
            .iter()
            .next()
            .and_then(|p| p.first().copied())
            .unwrap_or(0);
        if ps != 0 {
            return;
        }
        let response = format!("\x1bP>|okena({})\x1b\\", app_version());
        self.transport.send_input(&self.terminal_id, response.as_bytes());
    }
}

/// Extract the local path from an `OSC 7` `file://host/path` URI.
///
/// Host component is accepted but ignored — Okena's remote terminals already
/// know which host a session belongs to, so the path alone is what callers
/// care about. Returns `None` if the scheme is missing, the URI has no path
/// component, or percent-decoding yields invalid UTF-8.
pub(super) fn parse_osc7_file_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path_start = rest.find('/')?;
    percent_decode(&rest[path_start..])
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
