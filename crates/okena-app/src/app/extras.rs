//! Extra window observer + spawn-side OS window creation.
//!
//! Slice 05 keystone. The `Workspace::spawn_extra_window` data-layer mutation
//! pushes a fresh `WindowState` onto `WorkspaceData.extra_windows`; this module
//! is the consumer side that detects new entries and opens an OS window per
//! entry, instantiating a fresh `WindowView` keyed by the new
//! `WindowId::Extra(uuid)` so each window gets its own per-window UI state
//! (sidebar, focus, overlays). PRD ref: `plans/multi-window.md`'s "Okena
//! coordinator" + "Lifecycle and runtime -> Spawn flow" sections.
//!
//! Cascade-offset bounds are seeded by `Workspace::spawn_extra_window` at
//! action-handler time (the handler reads `gpui::Window::window_bounds()` and
//! passes them to the wrapper); the observer here just reads the entry's
//! persisted `os_bounds` and threads it into `cx.open_window`'s
//! `WindowOptions::window_bounds`. When `os_bounds` is `None` (e.g. a future
//! caller spawns without bounds, or a slice 07 restore loads an entry with no
//! recorded bounds), the OS picks a default position.

#[cfg(target_os = "linux")]
use crate::simple_root::SimpleRoot as Root;
use crate::views::window::{WindowView, WindowViewEvent};
use crate::workspace::state::{WindowBounds as PersistedWindowBounds, WindowId, WorkspaceData};
use gpui::*;
#[cfg(not(target_os = "linux"))]
use gpui_component::Root;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::Okena;

/// Grace before an OS-level extra-window close is committed as a "forget"
/// (removed from the persisted layout). Quit flows deliver a close event to
/// every window (GNOME dock Quit, logout, Alt+F4-ing each window to exit);
/// committing the forget immediately made those quits wipe the multi-window
/// layout before the final quit-time save. Deliberate single-window closes
/// survive the delay: the app keeps running and the forget lands after it.
const EXTRA_FORGET_GRACE: Duration = Duration::from_secs(5);

/// Deferred forgets for OS-closed extra windows. Every OS close re-arms the
/// whole pending set onto a fresh generation: a burst of closes is what
/// "quitting by closing every window" looks like, so earlier timers go stale
/// and only the last close's timer commits — after ITS full grace. Pure
/// bookkeeping (no GPUI) so the re-arm semantics are unit-testable.
#[derive(Default)]
pub(super) struct PendingExtraForgets {
    pending: HashSet<WindowId>,
    generation: u64,
}

impl PendingExtraForgets {
    /// Record an OS close for `id`, re-arming every pending forget. Returns
    /// the generation the caller's grace timer must present to
    /// [`take_due`](Self::take_due).
    pub(super) fn note_close(&mut self, id: WindowId) -> u64 {
        self.generation += 1;
        self.pending.insert(id);
        self.generation
    }

    /// Drain every pending forget if `generation` is still the latest; a
    /// stale generation (some later close re-armed the set) drains nothing —
    /// the later close's timer owns the set now. Sorted for deterministic
    /// commit order.
    pub(super) fn take_due(&mut self, generation: u64) -> Vec<WindowId> {
        if generation != self.generation {
            return Vec::new();
        }
        let mut due: Vec<WindowId> = self.pending.drain().collect();
        due.sort_by_key(window_sort_key);
        due
    }
}

/// Stable sort key for window ids (uuid bytes; Main first). Ordering matters
/// when two pending closes touch shared state — see `extras_to_close`.
fn window_sort_key(id: &WindowId) -> [u8; 16] {
    match id {
        WindowId::Extra(uuid) => *uuid.as_bytes(),
        WindowId::Main => [0u8; 16],
    }
}

/// Compute which `WindowId::Extra` entries in `data.extra_windows` are NOT yet
/// present in `opened`. Returned in `extra_windows` Vec order so the caller
/// spawns OS windows in persistence order.
///
/// Pure function — separated from the observer body so the diff contract can
/// be exercised without standing up the full `Okena` entity (whose
/// construction pulls in PtyManager, settings, theme, remote, services, etc.).
pub(super) fn extras_to_open(data: &WorkspaceData, opened: &HashSet<WindowId>) -> Vec<WindowId> {
    data.extra_windows
        .iter()
        .map(|w| WindowId::Extra(w.id))
        .filter(|id| !opened.contains(id))
        .collect()
}

/// Compute which opened extra windows no longer have a matching persisted
/// `WorkspaceData.extra_windows` entry.
///
/// Result is sorted by the extra's uuid bytes so close order is deterministic
/// even though `opened` is a `HashSet`. Iteration order matters when two
/// pending closes touch shared state (e.g. the same terminal rendered in both
/// windows); a flaky order would surface as intermittent test failures.
pub(super) fn extras_to_close(data: &WorkspaceData, opened: &HashSet<WindowId>) -> Vec<WindowId> {
    let desired: HashSet<WindowId> = data
        .extra_windows
        .iter()
        .map(|w| WindowId::Extra(w.id))
        .collect();

    let mut result: Vec<WindowId> = opened
        .iter()
        .copied()
        .filter(|id| matches!(id, WindowId::Extra(_)) && !desired.contains(id))
        .collect();
    result.sort_by_key(window_sort_key);
    result
}

/// Resolve the OS bounds an extra window should open at: prefer the entry's
/// persisted `os_bounds`; fall back to a cascade-offset (+30,+30 origin,
/// preserved size) from main's live bounds when the entry has none recorded.
/// Returns `None` if both are absent so the OS picks a default position.
///
/// Pure function — slice 07 cri 2. The persisted-wins precedence covers the
/// common case (user resized an extra, quit, relaunched: bounds round-trip
/// exactly). The cascade fallback covers entries that pre-date slice 05's
/// cascade-offset spawn snapshot, or any future caller that pushes onto
/// `extra_windows` without pre-seeding `os_bounds`. The arithmetic mirrors
/// the spawn-time cascade in `WorkspaceData::spawn_extra_window` (also
/// +30,+30 origin shift, size preserved) so a freshly-minted extra and a
/// fallback-restored extra land at visually equivalent offsets.
pub(super) fn resolve_extra_window_bounds(
    persisted: Option<PersistedWindowBounds>,
    main_bounds: Option<PersistedWindowBounds>,
) -> Option<PersistedWindowBounds> {
    persisted.or_else(|| {
        main_bounds.map(|b| PersistedWindowBounds {
            origin_x: b.origin_x + 30.0,
            origin_y: b.origin_y + 30.0,
            width: b.width,
            height: b.height,
        })
    })
}

impl Okena {
    /// Workspace observer body: walk `extra_windows`, open an OS window for
    /// each entry not yet tracked in `Okena.extra_windows`. Idempotent —
    /// re-firing the observer with no new entries is a no-op (the diff is
    /// empty).
    pub(super) fn handle_extra_windows_changed(&mut self, cx: &mut Context<Self>) {
        let data = self.workspace.read(cx).data().clone();
        let opened: HashSet<WindowId> = self.extra_windows.keys().copied().collect();
        for window_id in extras_to_close(&data, &opened) {
            if let Some(handle) = self.extra_window_handles.remove(&window_id) {
                let _ = handle.update(cx, |_, window, _| {
                    window.remove_window();
                });
            }
            self.extra_windows.remove(&window_id);
        }

        let to_open = extras_to_open(&data, &opened);
        if !to_open.is_empty() {
            log::info!("Opening {} extra window(s)", to_open.len());
        }
        for window_id in to_open {
            self.open_extra_window(window_id, cx);
        }
    }

    /// React to an OS-level close of an extra window: arm a deferred forget
    /// instead of committing it now. If the app is quitting, do nothing at all
    /// — the final quit-time save must keep the window so the next launch
    /// restores it. While the forget is pending the window stays in
    /// `WorkspaceData.extra_windows` AND in the Okena-side maps: dead handles
    /// are tolerated by every consumer, and keeping `extra_windows` populated
    /// stops `handle_extra_windows_changed` from reopening the entry.
    pub(super) fn handle_extra_window_os_close(
        &mut self,
        window_id: WindowId,
        cx: &mut Context<Self>,
    ) {
        if self.quitting.load(Ordering::SeqCst) {
            return;
        }
        let generation = self.pending_extra_forgets.note_close(window_id);
        cx.spawn(async move |this, cx| {
            smol::Timer::after(EXTRA_FORGET_GRACE).await;
            let _ = this.update(cx, |this, cx| {
                this.apply_due_extra_forgets(generation, cx);
            });
        })
        .detach();
    }

    /// Commit the deferred forgets armed on `generation`. Bails when the app
    /// is quitting (quit-time closes must survive into the final save) or when
    /// a later close re-armed the entries (that close's timer owns them). The
    /// workspace mutation fires the extras observer, which sweeps the
    /// forgotten ids out of `extra_windows` / `extra_window_handles`.
    fn apply_due_extra_forgets(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.quitting.load(Ordering::SeqCst) {
            return;
        }
        let due = self.pending_extra_forgets.take_due(generation);
        if due.is_empty() {
            return;
        }
        self.workspace.update(cx, |ws, cx| {
            for window_id in due {
                ws.close_extra_window(window_id, cx);
            }
        });
    }

    /// Route a [`WindowViewEvent`] raised by any window to the right handler.
    pub(super) fn handle_window_view_event(
        &mut self,
        _view: Entity<WindowView>,
        event: &WindowViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            WindowViewEvent::JumpToProject { origin, project_id } => {
                self.jump_to_project_terminal(*origin, project_id, cx);
            }
            WindowViewEvent::RebuildLocal => self.rebuild_local(cx),
            WindowViewEvent::RestartLocalBuild => self.restart_local_build(cx),
        }
    }

    /// Focus the first visible terminal of an already-open project, activating
    /// the window it lives in. Prefers `origin` (the window the request came
    /// from), then main, then any extra. No-op if the project is open nowhere —
    /// the layout is never changed, this is purely a "click into the first
    /// terminal" across windows.
    ///
    /// A project with no terminals is still a valid destination: focus lands on
    /// the project itself (an empty path names no pane), which is what makes it
    /// the current project for everything that resolves one. Refusing to enter
    /// it left a bookmark project unreachable by keyboard — the only ways in
    /// were a click or a project that already had a terminal.
    fn jump_to_project_terminal(
        &mut self,
        origin: WindowId,
        project_id: &str,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.clone();

        let path = match workspace.read(cx).project(project_id) {
            Some(project) => project
                .layout
                .as_ref()
                .map_or_else(Vec::new, |layout| layout.find_visible_terminal_path()),
            None => return,
        };

        // Choose the target window: the first one (origin-first) where the
        // project is currently visible.
        let mut order = vec![origin, WindowId::Main];
        order.extend(self.extra_window_handles.keys().copied());
        let target = order.into_iter().find(|wid| {
            workspace
                .read(cx)
                .data()
                .window(*wid)
                .is_some_and(|w| !w.hidden_project_ids.contains(project_id))
        });
        let Some(target) = target else { return };

        // Resolve the target window's view + OS handle.
        let (view, handle) = match target {
            WindowId::Main => (self.main_window.clone(), self.main_window_handle),
            id => match (
                self.extra_windows.get(&id),
                self.extra_window_handles.get(&id),
            ) {
                (Some(v), Some(h)) => (v.clone(), *h),
                _ => return,
            },
        };

        // Mark this as an explicit jump so the target window centers the
        // project (arrow/click switches only ensure visibility).
        view.update(cx, |v, _| v.request_center_on_next_navigation());

        // Point that window's FocusManager at the first visible terminal.
        let focus_manager = view.read(cx).focus_manager();
        let pid = project_id.to_string();
        focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.set_focused_terminal(fm, pid, path, cx);
            });
            cx.notify();
        });

        // Bring the target OS window to the front and force it to re-render, so
        // the target terminal pane self-focuses on its next render (it focuses
        // when the FocusManager points at it). `refresh` bypasses the pane's
        // element cache, which a bare FocusManager notify would not.
        //
        // Caveat — raising the *window* is best-effort and platform-dependent:
        //   - X11: `activate_window` sends `_NET_ACTIVE_WINDOW`, so the window
        //     actually comes to the front.
        //   - Wayland (esp. GNOME/Mutter): the compositor blocks programmatic
        //     focus stealing. gpui's `activate()` requests an xdg-activation
        //     token on the *target* (unfocused) surface, which Mutter rejects —
        //     it only flags "demands attention" (a dock/taskbar blink). The
        //     terminal still gets focused, so once the user switches to that
        //     window they land in the right pane. A real cross-window raise on
        //     Wayland would need xdg-activation token handoff from the focused
        //     window, which gpui does not expose; users who want auto-raise can
        //     use a GNOME "focus stealing" extension or run on X11.
        // (App-level `cx.activate` is a no-op on Linux, so it doesn't help here.)
        let _ = handle.update(cx, |_, window, _| {
            window.activate_window();
            window.refresh();
        });
    }

    /// Open an OS window for the given extra `WindowId::Extra(uuid)` and
    /// register the resulting `Entity<WindowView>` in `Okena.extra_windows`.
    /// Wires an `on_window_should_close` hook so closing the OS window drops
    /// the entry from `WorkspaceData.extra_windows` (slice 07 cri 3 — close
    /// forgets) plus the `Okena`-side `extra_windows` and
    /// `extra_window_handles` maps.
    fn open_extra_window(&mut self, window_id: WindowId, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let terminals = self.terminals.clone();
        let okena = cx.entity().clone();

        // Resolve the OS bounds: prefer persisted `os_bounds` (seeded by
        // `Workspace::spawn_extra_window` at action-handler time, or
        // round-tripped through persistence from a prior session); else
        // cascade-offset from main's live bounds (slice 07 cri 2 fallback for
        // entries with no recorded bounds — pre-slice-05 persisted entries,
        // or future callers that spawn without pre-seeding). The conversion
        // chain is f32 → `PersistedWindowBounds` → gpui `WindowBounds` so the
        // cascade arithmetic stays in the pure helper, free of GPUI types.
        // `PersistedWindowBounds` is the persisted f32 struct from
        // `okena-state`; aliased on import to disambiguate from the gpui
        // `WindowBounds` enum brought in by `use gpui::*;`.
        let persisted = self
            .workspace
            .read(cx)
            .data()
            .window(window_id)
            .and_then(|w| w.os_bounds);
        let main_bounds = self
            .main_window_handle
            .update(cx, |_, window, _| window.window_bounds().get_bounds())
            .ok()
            .map(|b| PersistedWindowBounds {
                origin_x: f32::from(b.origin.x),
                origin_y: f32::from(b.origin.y),
                width: f32::from(b.size.width),
                height: f32::from(b.size.height),
            });
        let window_bounds =
            resolve_extra_window_bounds(persisted, main_bounds).map(|b: PersistedWindowBounds| {
                WindowBounds::Windowed(Bounds {
                    origin: point(px(b.origin_x), px(b.origin_y)),
                    size: size(px(b.width), px(b.height)),
                })
            });

        let mut opened_view = None;
        let result = cx.open_window(
            WindowOptions {
                titlebar: if cfg!(target_os = "windows") {
                    None
                } else {
                    Some(TitlebarOptions {
                        title: Some("Okena".into()),
                        appears_transparent: true,
                        ..Default::default()
                    })
                },
                window_bounds,
                is_resizable: true,
                window_decorations: Some(if cfg!(target_os = "windows") {
                    WindowDecorations::Client
                } else {
                    WindowDecorations::Server
                }),
                window_min_size: Some(Size {
                    width: px(400.0),
                    height: px(300.0),
                }),
                app_id: Some("okena".to_string()),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| {
                    WindowView::new(window_id, workspace.clone(), terminals.clone(), window, cx)
                });
                opened_view = Some(view.clone());

                window.on_next_frame(move |_window, _cx| {
                    log::info!("Extra window {window_id:?} received its first platform frame");
                });

                // Slice 07 cri 3 close-flow, quit-aware: an OS-level close
                // (X button, Alt+F4, compositor quit-all) does NOT commit
                // the "forget" (PRD user story 22) immediately — quit flows
                // deliver a close to every window and an eager forget wiped
                // the multi-window layout right before the quit-time save
                // (the recurring restore-only-one-window bug). Instead the
                // forget is deferred via `handle_extra_window_os_close`;
                // the Okena-side maps stay populated until it commits so
                // the extras observer can't mistake the still-persisted
                // entry for a fresh one and reopen a zombie window. The
                // explicit in-app CloseWindow action keeps its immediate
                // forget (see views/window/render.rs). Returning `true`
                // allows the OS close to proceed.
                let okena_for_close = okena.clone();
                window.on_window_should_close(cx, move |_window, cx| {
                    okena_for_close.update(cx, |this, cx| {
                        this.handle_extra_window_os_close(window_id, cx);
                    });
                    true
                });

                cx.new(|cx| Root::new(view, window, cx))
            },
        );

        // open_window is synchronous, so register before observers can run again.
        match result {
            Ok(handle) => {
                let Some(view) = opened_view else {
                    log::error!("Extra window {window_id:?} opened without a WindowView");
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                    return;
                };
                let handle: AnyWindowHandle = handle.into();
                self.extra_windows.insert(window_id, view.clone());
                self.extra_window_handles.insert(window_id, handle);
                cx.subscribe(&view, Okena::handle_window_view_event)
                    .detach();

                let remote_manager = self.remote_manager.clone();
                view.update(cx, |view, cx| {
                    view.set_remote_manager(remote_manager, cx);
                });
                log::info!(
                    "Registered extra window {window_id:?} as GPUI window {}",
                    handle.window_id().as_u64()
                );
            }
            Err(error) => {
                log::error!("Failed to open extra window {window_id:?}: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PendingExtraForgets, extras_to_close, extras_to_open, resolve_extra_window_bounds,
    };
    use crate::workspace::state::{
        WindowBounds as PersistedWindowBounds, WindowId, WindowState, WorkspaceData,
    };
    use std::collections::{HashMap, HashSet};
    use uuid::Uuid;

    fn empty_workspace() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: Vec::new(),
            project_order: Vec::new(),
            folders: Vec::new(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    fn extras_not_opened(data: &WorkspaceData, opened: &HashSet<WindowId>) -> Vec<WindowId> {
        extras_to_open(data, opened)
    }

    #[test]
    fn empty_extras_returns_empty() {
        let data = empty_workspace();
        let opened = HashSet::new();
        assert!(extras_not_opened(&data, &opened).is_empty());
    }

    #[test]
    fn returns_every_extra_when_nothing_opened() {
        let mut data = empty_workspace();
        let id1 = data.spawn_extra_window(None);
        let id2 = data.spawn_extra_window(None);
        let opened = HashSet::new();
        let pending = extras_not_opened(&data, &opened);
        assert_eq!(pending, vec![id1, id2]);
    }

    #[test]
    fn skips_extras_already_opened() {
        let mut data = empty_workspace();
        let id1 = data.spawn_extra_window(None);
        let id2 = data.spawn_extra_window(None);
        let mut opened = HashSet::new();
        opened.insert(id1);
        let pending = extras_not_opened(&data, &opened);
        assert_eq!(pending, vec![id2]);
    }

    #[test]
    fn idempotent_when_all_extras_opened() {
        let mut data = empty_workspace();
        let id1 = data.spawn_extra_window(None);
        let id2 = data.spawn_extra_window(None);
        let mut opened = HashSet::new();
        opened.insert(id1);
        opened.insert(id2);
        assert!(extras_not_opened(&data, &opened).is_empty());
    }

    #[test]
    fn no_removed_extras_returns_empty_close_set() {
        let mut data = empty_workspace();
        let id1 = data.spawn_extra_window(None);
        let id2 = data.spawn_extra_window(None);
        let opened = HashSet::from([id1, id2]);

        assert!(extras_to_close(&data, &opened).is_empty());
    }

    #[test]
    fn returns_opened_extras_missing_from_workspace_data() {
        let mut data = empty_workspace();
        let surviving = data.spawn_extra_window(None);
        let removed = WindowId::Extra(Uuid::new_v4());
        let opened = HashSet::from([surviving, removed]);

        let pending: HashSet<WindowId> = extras_to_close(&data, &opened).into_iter().collect();

        assert_eq!(pending, HashSet::from([removed]));
    }

    #[test]
    fn close_diff_ignores_main_window_id() {
        let data = empty_workspace();
        let opened = HashSet::from([WindowId::Main]);

        assert!(extras_to_close(&data, &opened).is_empty());
    }

    #[test]
    fn ignores_main_window_id() {
        // Main is addressed separately on Okena (Okena.main_window field), not
        // through extra_windows. The diff helper must never return Main even
        // if the caller mistakenly passes an empty `opened` set.
        let data = empty_workspace();
        let opened = HashSet::new();
        let pending = extras_not_opened(&data, &opened);
        assert!(!pending.contains(&WindowId::Main));
    }

    #[test]
    fn returns_in_persistence_order() {
        // Extras are pushed onto extra_windows in spawn order; the observer
        // must open them in the same order so the OS-window stacking order at
        // restore time (slice 07) matches the persisted Vec.
        let mut data = empty_workspace();
        let ids: Vec<WindowId> = (0..5).map(|_| data.spawn_extra_window(None)).collect();
        let opened = HashSet::new();
        assert_eq!(extras_not_opened(&data, &opened), ids);
    }

    #[test]
    fn default_workspace_yields_no_extras_to_open() {
        // Slice 07 cri 8: first launch on a system with no `workspace.json`
        // opens exactly one window (main), `extra_windows` empty. The
        // structural chain: `main.rs::265+520` calls
        // `persistence::load_workspace(...).unwrap_or_else(|_|
        // default_workspace())`, so a missing file (no Err — line 330 in
        // persistence.rs returns `Ok(default_workspace())` directly) AND a
        // corrupt-file Err (cri 9 path) both bottom out at
        // `default_workspace()`. Pin the contract: the fallback shape has
        // empty `extra_windows`, so the slice 07 cri 1 restore-at-launch
        // kickoff (`handle_extra_windows_changed` -> `extras_to_open`)
        // surfaces nothing to open. Main always opens unconditionally per
        // `main.rs::cx.open_window` for the Okena view, so the visible
        // window count is exactly one.
        //
        // Defends against a regression where a future refactor pre-populates
        // `default_workspace().extra_windows` (e.g. with a synthesized "tip"
        // window for new users), which would silently break the
        // single-window-on-fresh-install contract.
        let data = crate::workspace::persistence::default_workspace();
        assert!(
            data.extra_windows.is_empty(),
            "default_workspace must produce zero extras so first-launch opens main only"
        );
        let opened = HashSet::new();
        assert!(
            extras_not_opened(&data, &opened).is_empty(),
            "restore-at-launch kickoff must find no extras to open on the default workspace"
        );
    }

    #[test]
    fn corrupt_workspace_json_errors_without_panic_and_fallback_has_no_extras() {
        // Slice 07 cri 9: corrupt/missing `windows` section: app falls back
        // to one fresh main window — no panics. The structural chain:
        // `persistence::load_workspace` calls `serde_json::from_str` on the
        // file content; on parse error, the loader backs the file up as
        // `workspace.json.bak`, sets `LOADED_FROM_DEFAULT` (blocks save),
        // and returns Err. `main.rs::265+520`'s `unwrap_or_else` substitutes
        // `default_workspace()` (already pinned empty by the cri 8 test
        // above). The "no panic" half of the contract reduces to: serde
        // returns Result::Err on malformed input rather than panicking.
        //
        // Pin both halves:
        //   (a) `serde_json::from_str::<WorkspaceData>` on malformed JSON
        //       returns Err (no panic). The loader's backup + Err-return
        //       path is then exercised by the unwrap_or_else in main.rs.
        //   (b) the fallback `default_workspace()` has empty
        //       `extra_windows` — the same structural contract as cri 8,
        //       reasserted here because cri 9 names the no-panic +
        //       single-window guarantee independently.
        //
        // The "missing windows section" branch of cri 9 (legacy JSON without
        // `main_window` / `extra_windows` fields) is already pinned by the
        // existing `workspace_data_old_shape_loads_with_default_main_window`
        // test in `crates/okena-state/src/workspace_data.rs::tests` (line
        // 365): legacy JSON parses successfully and yields a default
        // `main_window` + empty `extra_windows`. This test covers the
        // "corrupt/wrong-type" branch (e.g. `extra_windows: "not an array"`)
        // where serde must err rather than panic.
        let corrupt_json = r#"{
            "version": 1,
            "projects": [],
            "project_order": [],
            "extra_windows": "this should be an array, not a string"
        }"#;
        let parse_result: Result<crate::workspace::state::WorkspaceData, _> =
            serde_json::from_str(corrupt_json);
        assert!(
            parse_result.is_err(),
            "corrupt workspace.json must produce serde Err (no panic) so loader's backup + fallback path runs"
        );

        // Fallback path produces the single-main-window shape.
        let fallback = crate::workspace::persistence::default_workspace();
        assert!(
            fallback.extra_windows.is_empty(),
            "default_workspace fallback (used by main.rs's unwrap_or_else on load Err) must produce zero extras so corrupt-file recovery opens main only"
        );
    }

    #[test]
    fn no_artificial_cap_on_extras() {
        // Slice 05 cri 5: triggering NewWindow again opens a third window (no
        // artificial cap). The full chain — `WorkspaceData::spawn_extra_window`
        // (just `extra_windows.push`), `Workspace::spawn_extra_window` (delegate
        // + notify), the action handler in `WindowView::render` (delegate to
        // wrapper), and this helper — has no upper bound. Pin the structural
        // contract by spawning well above any reasonable "third window"
        // threshold and asserting every entry surfaces in the helper output.
        // Defends against a future refactor that introduces a cap (e.g. a
        // resource-budget guard) without surfacing the cap in the helper's
        // contract: such a regression would either reject the spawn at the
        // data layer (visible as a shorter `data.extra_windows`) or skip
        // entries in the helper (visible as a shorter return Vec). Either
        // direction fails this test.
        let mut data = empty_workspace();
        let ids: Vec<WindowId> = (0..25).map(|_| data.spawn_extra_window(None)).collect();
        assert_eq!(
            data.extra_windows.len(),
            25,
            "data layer must accept every spawn"
        );
        let opened = HashSet::new();
        assert_eq!(
            extras_not_opened(&data, &opened),
            ids,
            "helper must surface every pending entry"
        );
    }

    // ── Deferred OS-close forgets ────────────────────────────────────────

    #[test]
    fn single_close_commits_on_its_own_generation() {
        // The plain case: one extra is OS-closed, nothing else happens, its
        // grace timer fires with the generation note_close returned — the
        // forget commits.
        let mut forgets = PendingExtraForgets::default();
        let id = WindowId::Extra(Uuid::new_v4());
        let generation = forgets.note_close(id);
        assert_eq!(forgets.take_due(generation), vec![id]);
        // Drained: a duplicate timer fire (or a retry) finds nothing.
        assert!(forgets.take_due(generation).is_empty());
    }

    #[test]
    fn later_close_rearms_earlier_pending_forgets() {
        // Quit-by-closing-windows: extra A closes, then extra B closes before
        // A's grace expires. A's timer must find nothing (its generation went
        // stale) and B's timer must commit BOTH — so a close burst always gets
        // the full grace of its last close, during which the quit signal can
        // arrive and cancel everything.
        let mut forgets = PendingExtraForgets::default();
        let a = WindowId::Extra(Uuid::new_v4());
        let b = WindowId::Extra(Uuid::new_v4());
        let gen_a = forgets.note_close(a);
        let gen_b = forgets.note_close(b);

        assert!(
            forgets.take_due(gen_a).is_empty(),
            "stale timer must not commit"
        );
        let due: HashSet<WindowId> = forgets.take_due(gen_b).into_iter().collect();
        assert_eq!(
            due,
            HashSet::from([a, b]),
            "last close owns every pending forget"
        );
    }

    #[test]
    fn commit_order_is_deterministic_by_uuid() {
        // Mirrors extras_to_close's contract: shared-state teardown order must
        // not depend on HashMap iteration order.
        let mut forgets = PendingExtraForgets::default();
        let ids: Vec<WindowId> = (0..5).map(|_| WindowId::Extra(Uuid::new_v4())).collect();
        let mut generation = 0;
        for id in &ids {
            generation = forgets.note_close(*id);
        }
        let mut expected = ids.clone();
        expected.sort_by_key(super::window_sort_key);
        assert_eq!(forgets.take_due(generation), expected);
    }

    // ── Restore-bounds resolver ──────────────────────────────────────────

    #[test]
    fn resolve_extra_window_bounds_persisted_wins_over_main() {
        // Slice 07 cri 2: when the persisted entry already has `os_bounds`
        // (the common case after a quit/relaunch round-trip, or after a
        // fresh spawn that seeded bounds via the slice 05 cascade), the
        // resolver returns the persisted value verbatim. Main's live bounds
        // are ignored — without this precedence, restoring a manually-resized
        // extra would silently snap it back to a cascade offset.
        let persisted = PersistedWindowBounds {
            origin_x: 500.0,
            origin_y: 300.0,
            width: 1920.0,
            height: 1080.0,
        };
        let main_bounds = PersistedWindowBounds {
            origin_x: 0.0,
            origin_y: 0.0,
            width: 1280.0,
            height: 800.0,
        };
        let resolved = resolve_extra_window_bounds(Some(persisted), Some(main_bounds))
            .expect("persisted bounds resolve to Some");
        assert_eq!(resolved, persisted);
    }

    #[test]
    fn resolve_extra_window_bounds_no_persisted_cascades_from_main() {
        // The fallback path: persisted `os_bounds` is None (e.g. a
        // pre-slice-05 entry, or a future caller that spawns without
        // bounds). Resolver computes +30,+30 origin shift, preserves size.
        // Mirrors the data-layer cascade rule from
        // `WorkspaceData::spawn_extra_window` so a fresh spawn and a
        // fallback restore land at visually equivalent offsets.
        let main_bounds = PersistedWindowBounds {
            origin_x: 100.0,
            origin_y: 200.0,
            width: 1280.0,
            height: 800.0,
        };
        let resolved = resolve_extra_window_bounds(None, Some(main_bounds))
            .expect("cascade fallback resolves to Some");
        assert_eq!(resolved.origin_x, 130.0);
        assert_eq!(resolved.origin_y, 230.0);
        assert_eq!(resolved.width, 1280.0);
        assert_eq!(resolved.height, 800.0);
    }

    #[test]
    fn resolve_extra_window_bounds_no_persisted_no_main_returns_none() {
        // Both inputs absent — the OS picks a default position. Defends
        // against a future regression that synthesised a "default" bounds
        // (e.g. fixed 0,0,1280,800) when both inputs are None: such a
        // change would silently override the user's OS default.
        assert!(resolve_extra_window_bounds(None, None).is_none());
    }

    #[test]
    fn resolve_extra_window_bounds_persisted_some_main_none_passes_through() {
        // Symmetric case: main's bounds couldn't be read (e.g. drop race on
        // shutdown), but the persisted entry has bounds. The persisted
        // path doesn't need main, so the absent main_bounds is irrelevant.
        let persisted = PersistedWindowBounds {
            origin_x: 50.0,
            origin_y: 75.0,
            width: 800.0,
            height: 600.0,
        };
        let resolved = resolve_extra_window_bounds(Some(persisted), None)
            .expect("persisted alone is sufficient");
        assert_eq!(resolved, persisted);
    }
}
