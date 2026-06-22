use alacritty_terminal::vte::Perform;
use base64::Engine as _;
use parking_lot::Mutex;
use std::collections::HashMap;
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
        transport: Arc<dyn TerminalTransport>,
        terminal_id: String,
    ) -> Self {
        Self {
            parser: alacritty_terminal::vte::Parser::new(),
            perform: SidecarPerform {
                reported_cwd,
                pending_notifications,
                progress,
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
