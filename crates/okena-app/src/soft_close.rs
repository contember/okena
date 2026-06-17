//! Desktop orchestration for the grace-period "soft close".
//!
//! When the user closes a *busy* terminal (one with a running foreground
//! process), instead of killing it immediately we:
//!   1. remove the pane from the layout but keep the PTY alive,
//!   2. show a toast with "Undo" / "Close now" and a countdown,
//!   3. schedule the real teardown after the grace period.
//!
//! The layout bookkeeping + restore live on `Workspace` (`begin_soft_close`,
//! `undo_soft_close`, `finalize_soft_close`); this module wires the busy check,
//! the toast, and the timer. Only the interactive desktop close path goes
//! through here — the remote API keeps immediate-close semantics.

use crate::terminal::backend::TerminalBackend;
use crate::views::window::TerminalsRegistry;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::Workspace;
use crate::workspace::toast::{Toast, ToastAction, ToastActionStyle, ToastManager};
use gpui::*;
use std::time::Duration;

/// Prefix for the "undo" toast action id; payload is `:<project_id>:<terminal_id>`.
pub const UNDO_PREFIX: &str = "soft_close_undo";
/// Prefix for the "close now" toast action id; payload is `:<project_id>:<terminal_id>`.
pub const KILL_PREFIX: &str = "soft_close_kill";

/// Decode a toast action id of the form `<prefix>:<project_id>:<terminal_id>`.
/// Returns `(project_id, terminal_id)` when the prefix matches.
pub fn decode_action(id: &str, prefix: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix(prefix)?.strip_prefix(':')?;
    let (project_id, terminal_id) = rest.split_once(':')?;
    Some((project_id.to_string(), terminal_id.to_string()))
}

/// Cap a terminal label so the toast stays tidy (TOAST_WIDTH is ~320px). OSC
/// titles can be arbitrarily long; truncate on a char boundary with an ellipsis.
fn truncate_label(label: &str) -> String {
    const MAX_CHARS: usize = 42;
    if label.chars().count() <= MAX_CHARS {
        return label.to_string();
    }
    let mut out: String = label.chars().take(MAX_CHARS - 1).collect();
    out.push('\u{2026}');
    out
}

/// Home-relative, tail-preserving working directory for the toast detail line.
/// `~`-collapses the home dir and keeps the *end* of long paths (the directory
/// the user is actually in), since that's the useful part.
fn shorten_cwd(path: &str) -> String {
    let shown = match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path == home => return "~".to_string(),
        Ok(home) if !home.is_empty() && path.starts_with(&format!("{home}/")) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    };
    const MAX_CHARS: usize = 30;
    if shown.chars().count() <= MAX_CHARS {
        return shown;
    }
    let tail: String = shown
        .chars()
        .rev()
        .take(MAX_CHARS - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("\u{2026}{tail}")
}

/// Attempt to soft-close a terminal. Returns `true` if the close was handled as
/// a soft close (caller must NOT also run the immediate close); `false` if the
/// feature is off / the terminal is idle / not found, in which case the caller
/// should fall through to the normal immediate close.
pub fn try_begin(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    project_id: &str,
    terminal_id: &str,
    cx: &mut Context<Workspace>,
) -> bool {
    let grace_secs = crate::settings::settings(cx).terminal_close_grace_secs;
    if grace_secs == 0 {
        return false; // feature disabled
    }

    // Only soft-close *busy* terminals. The foreground shell pid resolves
    // through session backends (dtach / tmux), so "has a child" means the user
    // actually has a command running — not just the persistence wrapper.
    let fg_pid = backend.get_foreground_shell_pid(terminal_id);
    let busy = fg_pid
        .map(okena_terminal::terminal::has_child_processes)
        .unwrap_or(false);
    if !busy {
        return false;
    }

    let path = match ws
        .project(project_id)
        .and_then(|p| p.layout.as_ref())
        .and_then(|l| l.find_terminal_path(terminal_id))
    {
        Some(p) => p,
        None => return false,
    };

    // Build an informative two-line toast:
    //
    //   title:  Closed “make”             — what's closing
    //   detail: okena · ~/projects/okena  — project · working directory (muted)
    //
    // Read the live OSC title + cwd under a single registry lock.
    let (osc_title, cwd) = {
        let reg = terminals.lock();
        let term = reg.get(terminal_id);
        (term.and_then(|t| t.title()), term.map(|t| t.current_cwd()))
    };
    let command = fg_pid.and_then(okena_terminal::terminal::foreground_command);

    let (title, detail) = ws
        .project(project_id)
        .map(|p| {
            // Title label precedence: a meaningful display name (user-set custom
            // name or non-prompt OSC title) wins; else the live foreground
            // command; else a generic "Terminal closed".
            let display = p.terminal_display_name(terminal_id, osc_title);
            let label = if display == p.directory_name() { command } else { Some(display) };
            let title = match label {
                Some(l) => format!("Closed \u{201c}{}\u{201d}", truncate_label(&l)),
                None => "Terminal closed".to_string(),
            };
            // Detail line: project name, plus the cwd when we have one.
            let mut detail = p.name.clone();
            if let Some(cwd) = &cwd {
                detail.push_str(" \u{00b7} ");
                detail.push_str(&shorten_cwd(cwd));
            }
            (title, detail)
        })
        .unwrap_or_else(|| ("Terminal closed".to_string(), String::new()));

    let grace = Duration::from_secs(grace_secs as u64);
    let actions = vec![
        ToastAction::new(
            format!("{UNDO_PREFIX}:{project_id}:{terminal_id}"),
            "Undo",
            ToastActionStyle::Primary,
        ),
        ToastAction::new(
            format!("{KILL_PREFIX}:{project_id}:{terminal_id}"),
            "Close now",
            ToastActionStyle::Danger,
        ),
    ];
    let toast = {
        let base = Toast::info(title).with_ttl(grace).with_actions(actions);
        if detail.is_empty() { base } else { base.with_detail(detail) }
    };
    let toast_id = toast.id.clone();

    // Remove from the layout (PTY stays alive) and record the pending close.
    ws.begin_soft_close(focus_manager, project_id, &path, terminal_id, &toast_id, cx);
    ToastManager::post(toast, cx);

    // Schedule the real teardown once the grace period elapses. If the user
    // already undid or force-closed, `finalize_soft_close` returns false and
    // the toast was already dismissed by the action handler.
    let tid = terminal_id.to_string();
    let toast_id_timer = toast_id;
    cx.spawn(async move |ws_weak, cx| {
        smol::Timer::after(grace).await;
        let _ = ws_weak.update(cx, |ws, cx| {
            if ws.finalize_soft_close(&tid, cx) {
                ToastManager::dismiss(&toast_id_timer, cx);
            }
        });
    })
    .detach();

    true
}

#[cfg(test)]
mod tests {
    use super::{decode_action, shorten_cwd, truncate_label, KILL_PREFIX, UNDO_PREFIX};

    #[test]
    fn decode_action_round_trips() {
        let id = format!("{UNDO_PREFIX}:proj-1:term-9");
        assert_eq!(
            decode_action(&id, UNDO_PREFIX),
            Some(("proj-1".to_string(), "term-9".to_string()))
        );
    }

    #[test]
    fn decode_action_rejects_wrong_prefix() {
        let id = format!("{KILL_PREFIX}:proj-1:term-9");
        assert_eq!(decode_action(&id, UNDO_PREFIX), None);
    }

    #[test]
    fn decode_action_rejects_malformed() {
        assert_eq!(decode_action("soft_close_undo:onlyone", UNDO_PREFIX), None);
        assert_eq!(decode_action("garbage", UNDO_PREFIX), None);
    }

    #[test]
    fn truncate_label_leaves_short_labels_untouched() {
        assert_eq!(truncate_label("vim main.rs"), "vim main.rs");
    }

    #[test]
    fn truncate_label_caps_long_labels_with_ellipsis() {
        let long = "a".repeat(60);
        let out = truncate_label(&long);
        assert_eq!(out.chars().count(), 42);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn truncate_label_respects_char_boundaries() {
        // Multi-byte chars must not be split mid-codepoint.
        let long = "é".repeat(60);
        let out = truncate_label(&long);
        assert_eq!(out.chars().count(), 42);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn shorten_cwd_passes_through_short_paths() {
        // A path not under $HOME stays as-is when short enough.
        assert_eq!(shorten_cwd("/opt/app"), "/opt/app");
    }

    #[test]
    fn shorten_cwd_keeps_tail_of_long_paths() {
        let long = format!("/opt/{}/leaf", "deep/".repeat(20));
        let out = shorten_cwd(&long);
        assert_eq!(out.chars().count(), 30);
        assert!(out.starts_with('\u{2026}'), "leading ellipsis");
        assert!(out.ends_with("leaf"), "tail preserved");
    }
}
