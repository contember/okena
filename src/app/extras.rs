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
    fn open_extra_window(&mut self, window_id: WindowId, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let pty_manager = self.pty_manager.clone();
        let okena = cx.entity().clone();

        // Read the persisted cascade-offset bounds seeded by
        // `Workspace::spawn_extra_window` and lift them into the gpui
        // `WindowBounds::Windowed` shape that `cx.open_window` expects.
        // `PersistedWindowBounds` is the persisted f32 origin/size struct
        // from `okena-state`; aliased on import to disambiguate from the
        // gpui `WindowBounds` enum brought in by `use gpui::*;`.
        let window_bounds = self
            .workspace
            .read(cx)
            .data()
            .window(window_id)
            .and_then(|w| w.os_bounds)
            .map(|b: PersistedWindowBounds| {
                WindowBounds::Windowed(Bounds {
                    origin: point(px(b.origin_x), px(b.origin_y)),
                    size: size(px(b.width), px(b.height)),
                })
            });

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
                    WindowView::new(window_id, workspace.clone(), pty_manager.clone(), cx)
                });
                let view_for_okena = view.clone();
                let handle = window.window_handle();
                okena.update(cx, |this, _| {
                    this.extra_windows.insert(window_id, view_for_okena);
                    // Track the OS window handle so the remote-bridge command
                    // loop can resolve actions to whichever window is focused
                    // (PRD cri 13). The handle is removed on close in the
                    // upcoming slice 07 close-flow.
                    this.extra_window_handles.insert(window_id, handle);
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
    use super::{extras_to_open, resolve_focused_window_id};
    use crate::workspace::state::{WindowId, WindowState, WorkspaceData};
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
