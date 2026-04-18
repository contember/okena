use alacritty_terminal::vte::Perform;
use parking_lot::Mutex;
use std::sync::Arc;

use super::app_version::app_version;
use super::transport::TerminalTransport;

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
        pending_notifications: Arc<Mutex<Vec<String>>>,
        transport: Arc<dyn TerminalTransport>,
        terminal_id: String,
    ) -> Self {
        Self {
            parser: alacritty_terminal::vte::Parser::new(),
            perform: SidecarPerform {
                reported_cwd,
                pending_notifications,
                transport,
                terminal_id,
            },
        }
    }

    pub(super) fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.perform, bytes);
    }
}

struct SidecarPerform {
    reported_cwd: Arc<Mutex<Option<String>>>,
    /// iTerm2-style `OSC 9 ; <message>` notifications, drained by the GPUI
    /// thread on each render (same model as `pending_clipboard`).
    pending_notifications: Arc<Mutex<Vec<String>>>,
    transport: Arc<dyn TerminalTransport>,
    terminal_id: String,
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
            b"9" => {
                // iTerm2-style notification: `OSC 9 ; <message>`. ConEmu's
                // `OSC 9 ; 4 ; state ; progress` progress-bar subtype is
                // treated as a plain-text message for now — we can split
                // off subtypes when there's a UI for them.
                let message: String = params[1..]
                    .iter()
                    .filter_map(|p| std::str::from_utf8(p).ok())
                    .collect::<Vec<_>>()
                    .join(";");
                let message = message.trim();
                if !message.is_empty() {
                    self.pending_notifications.lock().push(message.to_string());
                }
            }
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
