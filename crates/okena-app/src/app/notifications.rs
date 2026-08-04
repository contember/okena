//! Desktop notifications for `OSC 9` / `OSC 777` terminal alerts.
//!
//! The terminal's OSC sidecar queues notifications (`OSC 9 ; body` or
//! `OSC 777 ; notify ; title ; body`); the centralized PTY event loop drains
//! those queues here via [`Okena::process_terminal_notifications`] for every
//! terminal that produced output in the batch — visible *or* background.
//!
//! A notification fires unless the emitting pane is the one the user is
//! actively looking at (the focused pane in a window that currently holds OS
//! focus). So background tabs, inactive detached windows, and "the whole app
//! isn't focused" all notify, matching the issue's intent.
//!
//! Click-to-focus is best-effort and platform-dependent. On XDG (Linux/BSD)
//! clicking the bubble invokes its `default` action, which routes a
//! [`NotificationJump`] back to the GPUI thread to focus the originating pane
//! (and raise its window). `notify-rust` can't surface a click callback on
//! macOS/Windows, so there the bubble is shown without a jump.

use crate::terminal::terminal::TerminalNotification;
use crate::views::window::WindowView;
use crate::workspace::state::WindowId;
use gpui::*;
use notify_rust::Notification;

use super::Okena;

/// Where a clicked desktop notification should send the user: the exact
/// terminal that raised the alert.
#[derive(Clone, Debug)]
pub(crate) struct NotificationJump {
    pub project_id: String,
    pub terminal_id: String,
}

/// Fire a single native OS notification on a dedicated thread.
///
/// `notify-rust`'s `show()` (and, on XDG, `wait_for_action`) block, so each
/// notification owns a short-lived thread that ends when the OS closes the
/// bubble. On XDG a click invokes the `default` action and sends `jump` back
/// through `tx`; elsewhere the bubble is shown without click handling.
pub(crate) fn show_notification(
    title: String,
    body: String,
    jump: Option<NotificationJump>,
    tx: async_channel::Sender<NotificationJump>,
) {
    std::thread::spawn(move || {
        let mut builder = Notification::new();
        builder.summary(&title).body(&body).appname("Okena");

        // A "default" action makes the whole bubble clickable on XDG; only add
        // it when we actually have somewhere to jump.
        #[cfg(all(unix, not(target_os = "macos")))]
        if jump.is_some() {
            builder.action("default", "Open");
        }

        match builder.show() {
            Ok(_handle) => {
                #[cfg(all(unix, not(target_os = "macos")))]
                if let Some(jump) = jump.as_ref() {
                    // Blocks until the bubble is actioned or closed by the daemon.
                    _handle.wait_for_action(|action| {
                        if action == "default" {
                            let _ = tx.send_blocking(jump.clone());
                        }
                    });
                }
                // Click-to-focus is unsupported here; keep params "used".
                #[cfg(not(all(unix, not(target_os = "macos"))))]
                let _ = (&jump, &tx);
            }
            Err(e) => log::warn!("desktop notification failed: {e}"),
        }
    });
}

impl Okena {
    /// Spawn the loop that turns clicked XDG notifications into pane jumps.
    /// The notification threads (see [`show_notification`]) send a
    /// [`NotificationJump`] here; on other platforms nothing is ever sent.
    pub(super) fn start_notification_click_loop(
        &mut self,
        rx: async_channel::Receiver<NotificationJump>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this: WeakEntity<Okena>, cx| {
            while let Ok(jump) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.jump_to_terminal(&jump.project_id, &jump.terminal_id, None, cx);
                });
            }
        })
        .detach();
    }

    /// Drain `OSC 9` / `OSC 777` notifications for the terminals that produced
    /// output this PTY batch and fire OS notifications for the ones the user
    /// isn't already watching. Always drains (even when disabled) so the
    /// per-terminal queues can't grow unbounded.
    pub(super) fn process_terminal_notifications(
        &mut self,
        dirty_terminal_ids: &[String],
        cx: &mut Context<Self>,
    ) {
        // Drain first — almost every batch has nothing queued, and this is the
        // PTY hot path. Both drains are a quick lock + take/swap, so we drain
        // unconditionally to keep the queues bounded (and clear a stale bell
        // edge), then bail before touching settings when there's nothing.
        let mut drained: Vec<(String, Vec<TerminalNotification>, bool)> = Vec::new();
        {
            let reg = self.terminals.lock();
            for tid in dirty_terminal_ids {
                if let Some(term) = reg.get(tid) {
                    let osc = term.take_pending_notifications();
                    let bell = term.take_pending_bell();
                    if !osc.is_empty() || bell {
                        drained.push((tid.clone(), osc, bell));
                    }
                }
            }
        }
        if drained.is_empty() {
            return;
        }

        // Activity stamping for bell/OSC alerts now happens on the DAEMON
        // (`pty_loop::process_activity_edges`), which owns the authoritative
        // `last_activity_at` and persists it. Bumping the client mirror here would
        // only be overwritten on the next state sync, so we don't — we keep
        // draining the edges above purely to fire OS notification bubbles below.

        // Read the (small) notification settings; bail if the feature is off.
        // Draining above already cleared the queues, so nothing accumulates
        // while disabled.
        let n = crate::settings::settings_entity(cx)
            .read(cx)
            .get()
            .notifications
            .clone();
        if !n.enabled || (!n.osc && !n.bell) {
            return;
        }

        for (tid, osc, bell) in drained {
            // Resolve the owning project + pane for focus suppression and
            // click-to-focus. Unmapped terminals (e.g. some service/hook PTYs)
            // still notify — just without suppression or a jump target.
            let resolved: Option<(String, String, Vec<usize>)> = {
                let ws = self.workspace.read(cx);
                ws.find_project_for_terminal(&tid).and_then(|p| {
                    p.layout
                        .as_ref()
                        .and_then(|l| l.find_terminal_path(&tid))
                        .map(|path| (p.id.clone(), p.name.clone(), path))
                })
            };

            if let Some((project_id, _, path)) = &resolved
                && self.pane_focused_in_active_window(project_id, path, cx)
            {
                continue;
            }

            let jump = resolved.as_ref().map(|(pid, _, _)| NotificationJump {
                project_id: pid.clone(),
                terminal_id: tid.clone(),
            });
            // OSC 9 / bell carry no title; fall back to the project name.
            let fallback_title = resolved
                .as_ref()
                .map(|(_, name, _)| name.clone())
                .unwrap_or_else(|| "Okena".to_string());

            if n.osc && !osc.is_empty() {
                for notification in osc {
                    let title = notification.title.unwrap_or_else(|| fallback_title.clone());
                    show_notification(
                        title,
                        notification.body,
                        jump.clone(),
                        self.notification_jump_tx.clone(),
                    );
                }
                // Light the pane's attention border (mirrors the bell), cleared
                // when the user focuses it. Only set on the fire path, so it
                // already respects the settings + focused-pane suppression.
                if let Some(term) = self.terminals.lock().get(&tid) {
                    term.mark_notification();
                }
            }

            if bell && n.bell {
                show_notification(
                    fallback_title,
                    "🔔 Terminal bell".to_string(),
                    jump,
                    self.notification_jump_tx.clone(),
                );
            }
        }
    }

    /// Apply OSC 52 clipboard *writes* queued by terminals that produced output.
    ///
    /// This side effect is intentionally drained from the immediate activity
    /// handler rather than relying on `TerminalContent::render`: background-only
    /// panes may remain inactive indefinitely, while clipboard semantics must not.
    /// The render path keeps a fallback drain for non-remote terminal producers.
    pub(super) fn process_clipboard_writes(
        &mut self,
        dirty_terminal_ids: &[String],
        cx: &mut Context<Self>,
    ) {
        let writes = {
            let reg = self.terminals.lock();
            let mut writes = Vec::new();
            for terminal_id in dirty_terminal_ids {
                if let Some(terminal) = reg.get(terminal_id) {
                    writes.extend(terminal.take_pending_clipboard_writes());
                }
            }
            writes
        };

        for text in writes {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Answer (or silently deny) OSC 52 clipboard *read* requests
    /// (`OSC 52 ; c ; ?`) queued by terminals that produced output this batch.
    /// Runs here, in the PTY event loop, because this is where the opt-in
    /// setting and the system clipboard are reachable — the event listener
    /// that enqueues the requests has neither.
    ///
    /// Clipboard read is gated behind `allow_clipboard_read` (off by default):
    /// a program reading the clipboard can exfiltrate whatever the user has
    /// copied. When allowed, every requesting terminal gets the current
    /// clipboard contents; when not, the requests are dropped without a reply
    /// so the per-terminal queues stay bounded.
    pub(super) fn process_clipboard_reads(
        &mut self,
        dirty_terminal_ids: &[String],
        cx: &mut Context<Self>,
    ) {
        // Collect the terminals that actually have a queued read. Almost every
        // batch has none, so we bail before touching settings or the clipboard.
        let pending: Vec<String> = {
            let reg = self.terminals.lock();
            dirty_terminal_ids
                .iter()
                .filter(|tid| {
                    reg.get(*tid)
                        .is_some_and(|t| t.has_pending_clipboard_reads())
                })
                .cloned()
                .collect()
        };
        if pending.is_empty() {
            return;
        }

        // Read the opt-in setting once for the whole batch.
        let allow = crate::settings::settings_entity(cx)
            .read(cx)
            .get()
            .allow_clipboard_read;

        if allow {
            // Read the system clipboard once; hand the same contents to every
            // requesting terminal. An empty/imageless clipboard answers "".
            let content = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default();
            let reg = self.terminals.lock();
            for tid in &pending {
                if let Some(term) = reg.get(tid) {
                    term.answer_clipboard_reads(&content);
                }
            }
        } else {
            // Silently deny: drop the queued requests without replying so the
            // queue stays bounded while the feature is off.
            let reg = self.terminals.lock();
            for tid in &pending {
                if let Some(term) = reg.get(tid) {
                    term.drop_clipboard_reads();
                }
            }
        }
    }

    /// True when `(project_id, path)` is the focused pane in a window that
    /// currently holds OS focus. Background tabs, inactive detached windows,
    /// and "no Okena window focused" all return false, so they notify.
    fn pane_focused_in_active_window(
        &self,
        project_id: &str,
        path: &[usize],
        cx: &mut Context<Self>,
    ) -> bool {
        let mut windows: Vec<(Entity<WindowView>, AnyWindowHandle)> =
            vec![(self.main_window.clone(), self.main_window_handle)];
        for (wid, view) in &self.extra_windows {
            if let Some(handle) = self.extra_window_handles.get(wid) {
                windows.push((view.clone(), *handle));
            }
        }
        for (view, handle) in windows {
            let active = handle
                .update(cx, |_, window, _| window.is_window_active())
                .unwrap_or(false);
            if !active {
                continue;
            }
            if view
                .read(cx)
                .focus_manager()
                .read(cx)
                .is_focused(project_id, path)
            {
                return true;
            }
        }
        false
    }

    /// Focus an exact terminal in the requested window, or the active window.
    pub(super) fn jump_to_terminal(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        requested_window: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let requested_window = match requested_window {
            None => None,
            Some("main") => Some(WindowId::Main),
            Some(id) => {
                let Ok(id) = uuid::Uuid::parse_str(id) else {
                    return;
                };
                Some(WindowId::Extra(id))
            }
        };
        let target = requested_window.unwrap_or_else(|| {
            let active = cx.active_window();
            self.extra_window_handles
                .iter()
                .find(|(_, handle)| Some(**handle) == active)
                .map(|(id, _)| *id)
                .unwrap_or(WindowId::Main)
        });

        let Some((view, handle)) = self.window_view_and_handle(target) else {
            return;
        };
        let workspace = self.workspace.clone();
        let focus_manager = view.read(cx).focus_manager();
        let pid = project_id.to_string();
        let tid = terminal_id.to_string();
        focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                // Revealing an off-screen project is `focus_terminal_by_id`'s
                // job now. Zooming here first cancelled fullscreen (changing the
                // focused project drops that context) before the reveal could
                // retarget it, so a jump out of a fullscreened pane dumped the
                // user back into the overview.
                ws.focus_terminal_by_id(fm, target, &pid, &tid, cx);
            });
            cx.notify();
        });

        // Best-effort raise — see `jump_to_project_terminal` for the platform
        // caveats (X11 raises; Wayland only flags "demands attention").
        let _ = handle.update(cx, |_, window, _| {
            window.activate_window();
            window.refresh();
        });
    }

    /// Resolve a window's view entity + OS handle, or `None` if the id names an
    /// extra that has been dropped (close race).
    fn window_view_and_handle(
        &self,
        window_id: WindowId,
    ) -> Option<(Entity<WindowView>, AnyWindowHandle)> {
        match window_id {
            WindowId::Main => Some((self.main_window.clone(), self.main_window_handle)),
            id => match (
                self.extra_windows.get(&id),
                self.extra_window_handles.get(&id),
            ) {
                (Some(v), Some(h)) => Some((v.clone(), *h)),
                _ => None,
            },
        }
    }

}
