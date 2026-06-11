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
                    this.jump_to_terminal(&jump.project_id, &jump.terminal_id, cx);
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

        // A bell or OSC alert is "activity" for the owning project regardless
        // of the notification settings below — stamp it before any settings
        // bail so the activity-sorted sidebar surfaces the project even when
        // desktop notifications are disabled.
        self.bump_activity_for_terminals(drained.iter().map(|(tid, _, _)| tid.as_str()), cx);

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

    /// Drain the one-shot command-finished (OSC 133 ;D) edge for each dirty
    /// terminal and stamp activity on the owning project. Called from the PTY
    /// event loop alongside notification draining so a finished command floats
    /// its project up in the activity-sorted sidebar — even with no bell.
    pub(super) fn process_command_finished_activity(
        &mut self,
        dirty_terminal_ids: &[String],
        cx: &mut Context<Self>,
    ) {
        // Drain edges first (cheap atomic swap); collect the terminals that
        // actually saw a command finish. Almost every batch drains nothing.
        let finished: Vec<String> = {
            let reg = self.terminals.lock();
            dirty_terminal_ids
                .iter()
                .filter(|tid| {
                    reg.get(*tid)
                        .is_some_and(|t| t.take_pending_command_finished())
                })
                .cloned()
                .collect()
        };
        if finished.is_empty() {
            return;
        }
        self.bump_activity_for_terminals(finished.iter().map(|s| s.as_str()), cx);
    }

    /// Resolve each terminal id to its owning project and bump that project's
    /// activity timestamp once. Deduplicates projects so a batch touching
    /// several terminals of the same project only notifies/persists once.
    fn bump_activity_for_terminals<'a>(
        &mut self,
        terminal_ids: impl Iterator<Item = &'a str>,
        cx: &mut Context<Self>,
    ) {
        let project_ids: std::collections::HashSet<String> = {
            let ws = self.workspace.read(cx);
            terminal_ids
                .filter_map(|tid| ws.find_project_for_terminal(tid).map(|p| p.id.clone()))
                .collect()
        };
        if project_ids.is_empty() {
            return;
        }
        self.workspace.update(cx, |ws, cx| {
            for pid in project_ids {
                ws.bump_activity(&pid, cx);
            }
        });
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

    /// Focus the specific terminal that raised a notification, raising the
    /// window it lives in. Mirrors `jump_to_project_terminal` but targets an
    /// exact terminal id (activating its tab) rather than the first visible one.
    ///
    /// Two tiers: if the project is currently visible in some window, focus it
    /// there without disturbing the view. If it's visible *nowhere* — hidden
    /// column, folder filter, or a window zoomed into another project — fall
    /// back to zooming into it in the active (or main) window, which pierces
    /// all three so the click always lands on the terminal.
    fn jump_to_terminal(&mut self, project_id: &str, terminal_id: &str, cx: &mut Context<Self>) {
        // Tier 1: a window where the project is actually visible right now.
        let mut order = vec![WindowId::Main];
        order.extend(self.extra_window_handles.keys().copied());
        let mut visible_in: Option<WindowId> = None;
        for wid in order {
            if let Some((view, _)) = self.window_view_and_handle(wid)
                && self.project_visible_in(wid, &view, project_id, cx)
            {
                visible_in = Some(wid);
                break;
            }
        }

        // Tier 2: visible nowhere → reveal (zoom) in the active or main window.
        let (target, reveal) = match visible_in {
            Some(wid) => (wid, false),
            None => {
                let active = cx.active_window();
                let mut handles = vec![(WindowId::Main, self.main_window_handle)];
                handles.extend(self.extra_window_handles.iter().map(|(id, h)| (*id, *h)));
                let wid = handles
                    .into_iter()
                    .find(|(_, h)| Some(*h) == active)
                    .map(|(id, _)| id)
                    .unwrap_or(WindowId::Main);
                (wid, true)
            }
        };

        let Some((view, handle)) = self.window_view_and_handle(target) else {
            return;
        };

        let workspace = self.workspace.clone();
        let focus_manager = view.read(cx).focus_manager();
        let pid = project_id.to_string();
        let tid = terminal_id.to_string();
        focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                if reveal {
                    // Zoom into the project. The focus override is shown even
                    // when the project is in the window's hidden set or behind
                    // a folder filter, and supersedes a zoom into another
                    // project — so the terminal below becomes reachable.
                    ws.set_focused_project(fm, Some(pid.clone()), cx);
                }
                ws.focus_terminal_by_id(fm, &pid, &tid, cx);
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
            id => match (self.extra_windows.get(&id), self.extra_window_handles.get(&id)) {
                (Some(v), Some(h)) => Some((v.clone(), *h)),
                _ => None,
            },
        }
    }

    /// Whether `project_id` is in `window`'s currently visible set — accounting
    /// for the hidden set, folder filter, and that window's own zoom/focus
    /// state (read from its `FocusManager`).
    fn project_visible_in(
        &self,
        window_id: WindowId,
        view: &Entity<WindowView>,
        project_id: &str,
        cx: &App,
    ) -> bool {
        let (focused, individual) = {
            let fm = view.read(cx).focus_manager();
            let fm = fm.read(cx);
            (fm.focused_project_id().cloned(), fm.is_focus_individual())
        };
        self.workspace
            .read(cx)
            .visible_projects(window_id, focused.as_ref(), individual)
            .iter()
            .any(|p| p.id == project_id)
    }
}
