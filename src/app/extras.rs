//! Extra window observer + spawn-side OS window creation + CLI/remote
//! focused-window routing.
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
//!
//! Beyond spawn, this module also hosts the focused-window routing helper
//! `resolve_focused_window_id` used by the remote/CLI bridge to send actions
//! to the per-window `FocusManager` of whichever Okena window currently has
//! OS focus (PRD user story 27 / acceptance criterion 13). When no Okena
//! window is focused (another app is in front, or focus is unknown), the
//! helper falls back to `WindowId::Main`.

use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowBounds as PersistedWindowBounds, WindowId, WorkspaceData};
use crate::views::window::WindowView;
use gpui::*;
#[cfg(not(target_os = "linux"))]
use gpui_component::Root;
#[cfg(target_os = "linux")]
use crate::simple_root::SimpleRoot as Root;
use std::collections::HashSet;

use super::Okena;

/// Compute which `WindowId::Extra` entries in `data.extra_windows` are NOT yet
/// present in `opened`. Returned in `extra_windows` Vec order so the caller
/// spawns OS windows in persistence order.
///
/// Pure function — separated from the observer body so the diff contract can
/// be exercised without standing up the full `Okena` entity (whose
/// construction pulls in PtyManager, settings, theme, remote, services, etc.).
pub(super) fn extras_to_open(
    data: &WorkspaceData,
    opened: &HashSet<WindowId>,
) -> Vec<WindowId> {
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
pub(super) fn extras_to_close(
    data: &WorkspaceData,
    opened: &HashSet<WindowId>,
) -> Vec<WindowId> {
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
    result.sort_by_key(|id| match id {
        WindowId::Extra(uuid) => *uuid.as_bytes(),
        // Filter above guarantees Extra(_) only; the arm is unreachable in
        // practice but keeps the match total without an unwrap.
        WindowId::Main => [0u8; 16],
    });
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

/// Resolve the `WindowId` that the currently focused OS window corresponds to,
/// or fall back to `WindowId::Main` if no Okena window is focused (e.g. another
/// application has focus, or the active window isn't tracked).
///
/// Pure function — generic over the handle type so the routing rule can be
/// exercised without standing up real `gpui::AnyWindowHandle` values (which
/// have private fields and can only be constructed via `cx.open_window`). The
/// production caller passes `gpui::AnyWindowHandle`; tests use a trivial
/// stand-in.
///
/// PRD ref: `plans/multi-window.md` user story 27 ("CLI lands its action in
/// the focused window if any, falling back to main otherwise") +
/// `plans/issues/multi-window/05-spawn-extra-window.md` acceptance criterion
/// 13 (CLI fallback). Used by `Okena::focus_manager_for_active_window` to
/// route remote-bridge actions (the existing `okena action` CLI verb +
/// future `okena open <path>`-style verbs) to the correct per-window
/// `FocusManager`.
pub(super) fn resolve_focused_window_id<H: PartialEq + Copy>(
    active: Option<H>,
    window_handles: &[(WindowId, H)],
) -> WindowId {
    match active {
        Some(a) => window_handles
            .iter()
            .find(|(_, h)| *h == a)
            .map(|(id, _)| *id)
            .unwrap_or(WindowId::Main),
        None => WindowId::Main,
    }
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

        for window_id in extras_to_open(&data, &opened) {
            self.open_extra_window(window_id, cx);
        }
    }

    /// Resolve the `(WindowId, Entity<FocusManager>)` of whichever Okena
    /// window currently has OS focus, falling back to
    /// `(WindowId::Main, main_window.focus_manager())` if no Okena window is
    /// focused (e.g. another app is in front, or the active window isn't
    /// tracked). Used by the remote-bridge command loop so CLI/remote-driven
    /// actions land in the focused window's per-window state per PRD user
    /// story 27 + slice 05 cri 13. The `WindowId` flows into `execute_action`
    /// so per-window data mutations (e.g. `SetProjectShowInOverview`)
    /// also target the focused window, not just focus state.
    pub(super) fn focus_manager_for_active_window(
        &self,
        cx: &App,
    ) -> (WindowId, Entity<FocusManager>) {
        let active = cx.active_window();
        let mut handles: Vec<(WindowId, AnyWindowHandle)> = Vec::with_capacity(1 + self.extra_window_handles.len());
        handles.push((WindowId::Main, self.main_window_handle));
        handles.extend(self.extra_window_handles.iter().map(|(id, h)| (*id, *h)));
        let resolved = resolve_focused_window_id(active, &handles);
        match resolved {
            WindowId::Main => (WindowId::Main, self.main_window.read(cx).focus_manager()),
            extra_id @ WindowId::Extra(_) => match self.extra_windows.get(&extra_id) {
                // Drop-race fallback: the resolver matched on a tracked extra
                // handle but the corresponding `WindowView` entity has been
                // dropped between handle-tracking and resolution. Fall back
                // to main's `(WindowId, FocusManager)` so per-window data
                // mutations target a slot that exists.
                Some(view) => (extra_id, view.read(cx).focus_manager()),
                None => (WindowId::Main, self.main_window.read(cx).focus_manager()),
            },
        }
    }

    /// Open an OS window for the given extra `WindowId::Extra(uuid)` and
    /// register the resulting `Entity<WindowView>` in `Okena.extra_windows`.
    /// Wires an `on_window_should_close` hook so closing the OS window drops
    /// the entry from `WorkspaceData.extra_windows` (slice 07 cri 3 — close
    /// forgets) plus the `Okena`-side `extra_windows` and
    /// `extra_window_handles` maps.
    fn open_extra_window(&mut self, window_id: WindowId, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let pty_manager = self.pty_manager.clone();
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
        let window_bounds = resolve_extra_window_bounds(persisted, main_bounds).map(
            |b: PersistedWindowBounds| {
                WindowBounds::Windowed(Bounds {
                    origin: point(px(b.origin_x), px(b.origin_y)),
                    size: size(px(b.width), px(b.height)),
                })
            },
        );

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
            move |window, cx| {
                let view = cx.new(|cx| {
                    WindowView::new(window_id, workspace.clone(), pty_manager.clone(), terminals.clone(), window, cx)
                });
                let view_for_okena = view.clone();
                let handle = window.window_handle();
                // Defer the registration: this build closure runs while Okena
                // is already leased (the workspace observer that triggered
                // `open_extra_window` holds the Okena update lease for the
                // duration of `flush_effects`). Calling `okena.update` here
                // synchronously would double-lease and panic. `cx.defer`
                // schedules the registration to run after the current effect
                // flush completes, when the lease has released.
                let okena_for_register = okena.clone();
                cx.defer(move |cx| {
                    okena_for_register.update(cx, |this, cx| {
                        this.extra_windows.insert(window_id, view_for_okena.clone());
                        // Track the OS window handle so the remote-bridge
                        // command loop can resolve actions to whichever window
                        // is focused (PRD cri 13). The handle is removed on
                        // close below.
                        this.extra_window_handles.insert(window_id, handle);

                        // Wire the per-window UI to the shared singletons
                        // main was wired with at startup. Without this, the
                        // extra's ProjectColumns have no git_watcher (no +/-
                        // diff badges), no service_manager (service panel
                        // dead), and no remote_manager (remote actions don't
                        // route).
                        let git_watcher = this.git_watcher.clone();
                        let service_manager = this.service_manager.clone();
                        let remote_manager = this.remote_manager.clone();
                        view_for_okena.update(cx, |rv, cx| {
                            rv.set_git_watcher(git_watcher, cx);
                            rv.set_service_manager(service_manager, cx);
                            rv.set_remote_manager(remote_manager, cx);
                        });
                    });
                });

                // Slice 07 cri 3 close-flow: when the user closes this OS
                // window, drop the entry from `WorkspaceData.extra_windows`
                // (so persistence forgets it -- PRD user story 22) and from
                // `Okena.extra_windows` + `extra_window_handles` (so the
                // remote-bridge resolver and the spawn-side observer stop
                // seeing it). Order matters: the workspace mutation runs
                // FIRST so save/observers fire on a still-alive
                // `Entity<WindowView>` (the strong handle in
                // `Okena.extra_windows` keeps it alive until the second
                // step). The Okena-side removes drop the strong handle so
                // the entity then drops with the OS window. Returning
                // `true` allows the OS close to proceed.
                let workspace_for_close = workspace.clone();
                let okena_for_close = okena.clone();
                window.on_window_should_close(cx, move |_window, cx| {
                    workspace_for_close.update(cx, |ws, cx| {
                        ws.close_extra_window(window_id, cx);
                    });
                    okena_for_close.update(cx, |this, _cx| {
                        this.extra_windows.remove(&window_id);
                        this.extra_window_handles.remove(&window_id);
                    });
                    true
                });

                cx.new(|cx| Root::new(view, window, cx))
            },
        );

        if let Err(e) = result {
            log::error!("Failed to open extra window: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{extras_to_close, extras_to_open, resolve_extra_window_bounds, resolve_focused_window_id};
    use crate::workspace::state::{WindowBounds as PersistedWindowBounds, WindowId, WindowState, WorkspaceData};
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

    #[test]
    fn empty_extras_returns_empty() {
        let data = empty_workspace();
        let opened = HashSet::new();
        assert!(extras_to_open(&data, &opened).is_empty());
    }

    #[test]
    fn returns_every_extra_when_nothing_opened() {
        let mut data = empty_workspace();
        let id1 = data.spawn_extra_window(None);
        let id2 = data.spawn_extra_window(None);
        let opened = HashSet::new();
        let pending = extras_to_open(&data, &opened);
        assert_eq!(pending, vec![id1, id2]);
    }

    #[test]
    fn skips_extras_already_opened() {
        let mut data = empty_workspace();
        let id1 = data.spawn_extra_window(None);
        let id2 = data.spawn_extra_window(None);
        let mut opened = HashSet::new();
        opened.insert(id1);
        let pending = extras_to_open(&data, &opened);
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
        assert!(extras_to_open(&data, &opened).is_empty());
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
        let pending = extras_to_open(&data, &opened);
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
        assert_eq!(extras_to_open(&data, &opened), ids);
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
            extras_to_open(&data, &opened).is_empty(),
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
        assert_eq!(data.extra_windows.len(), 25, "data layer must accept every spawn");
        let opened = HashSet::new();
        assert_eq!(extras_to_open(&data, &opened), ids, "helper must surface every pending entry");
    }

    // ── Focused-window routing ───────────────────────────────────────────

    #[test]
    fn no_active_window_falls_back_to_main() {
        // Another OS app is in front (or focus is unknown). The CLI/remote
        // action must still land somewhere — main is the fallback per PRD
        // user story 27 ("falling back to main otherwise").
        let main_handle: u32 = 1;
        let extra_id = WindowId::Extra(Uuid::new_v4());
        let handles = vec![(WindowId::Main, main_handle), (extra_id, 2)];
        assert_eq!(resolve_focused_window_id::<u32>(None, &handles), WindowId::Main);
    }

    #[test]
    fn active_main_resolves_to_main() {
        let main_handle: u32 = 1;
        let extra_id = WindowId::Extra(Uuid::new_v4());
        let handles = vec![(WindowId::Main, main_handle), (extra_id, 2)];
        assert_eq!(
            resolve_focused_window_id(Some(main_handle), &handles),
            WindowId::Main,
        );
    }

    #[test]
    fn active_extra_resolves_to_that_extra() {
        // PRD cri 13's W2-focused branch: the focused window is an extra;
        // routing must land on that extra's WindowId so the remote bridge
        // mutates that extra's per-window FocusManager.
        let main_handle: u32 = 1;
        let extra_a = WindowId::Extra(Uuid::new_v4());
        let extra_b = WindowId::Extra(Uuid::new_v4());
        let handles = vec![
            (WindowId::Main, main_handle),
            (extra_a, 2),
            (extra_b, 3),
        ];
        assert_eq!(resolve_focused_window_id(Some(2), &handles), extra_a);
        assert_eq!(resolve_focused_window_id(Some(3), &handles), extra_b);
    }

    #[test]
    fn unknown_active_window_falls_back_to_main() {
        // The active window isn't tracked (e.g. detached terminal popup, or
        // a window opened by a future feature that doesn't register here).
        // Fall back to main rather than dropping the action.
        let main_handle: u32 = 1;
        let handles = vec![(WindowId::Main, main_handle)];
        assert_eq!(resolve_focused_window_id(Some(99), &handles), WindowId::Main);
    }

    #[test]
    fn empty_handles_falls_back_to_main() {
        // Defensive — should never happen in practice (main is always tracked)
        // but the helper stays total: any input shape produces a valid
        // WindowId, never panics.
        let handles: Vec<(WindowId, u32)> = Vec::new();
        assert_eq!(resolve_focused_window_id(Some(1), &handles), WindowId::Main);
        assert_eq!(resolve_focused_window_id::<u32>(None, &handles), WindowId::Main);
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

    #[test]
    fn first_match_wins_on_duplicate_handles() {
        // Pathological input — two entries point at the same handle. The
        // helper picks the first match (Vec order). In production, handles
        // are unique per OS window, but pinning the rule keeps the helper
        // deterministic if a future bug duplicates an entry.
        let extra_a = WindowId::Extra(Uuid::new_v4());
        let extra_b = WindowId::Extra(Uuid::new_v4());
        let handles = vec![(WindowId::Main, 1u32), (extra_a, 2), (extra_b, 2)];
        assert_eq!(resolve_focused_window_id(Some(2), &handles), extra_a);
    }
}
