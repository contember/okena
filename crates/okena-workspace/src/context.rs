//! Reactor abstraction for the workspace action/state layer.
//!
//! `Workspace` mutations need exactly two reactor capabilities from their
//! context: mark the entity dirty (so the autosave + `state_version` observers
//! and any cached views re-evaluate), and invalidate cached views so structural
//! changes repaint. Today the only implementer is GPUI's [`Context`]; the
//! headless daemon will add a second, GPUI-free implementer backed by a plain
//! tokio reactor (notify = fire registered observers, refresh_views = no-op).
//!
//! Action methods take `cx: &mut impl WorkspaceCx` instead of
//! `&mut Context<Self>`. This is a non-breaking change for existing callers:
//! they pass `&mut Context<Workspace>`, which satisfies the trait via the impl
//! below. Once every action is generic, the daemon can drive the same
//! `execute_action` code path with no GPUI in scope — the seam that makes the
//! GPUI-free daemon a swap behind the protocol rather than a rewrite.

use crate::state::Workspace;
use gpui::Context;

/// The reactor capabilities the workspace action/state layer needs.
///
/// Deliberately minimal: anything heavier (spawning tasks, reading other
/// entities) lives in the app/observer layer, not in the action layer, so it
/// does not belong here.
pub trait WorkspaceCx {
    /// Mark the workspace dirty. Fires the change observers (autosave debounce,
    /// `state_version` bump) and flags cached views for re-evaluation.
    ///
    /// GPUI: `Context::notify`. Daemon: invoke registered change callbacks.
    fn notify(&mut self);

    /// Invalidate cached views so a structural data change actually repaints.
    ///
    /// GPUI: `App::refresh_windows` (bypasses `.cached()` view wrappers).
    /// Daemon: no-op — there are no local views to refresh.
    fn refresh_views(&mut self);

    /// The hook runner service (creates PTY-backed hook terminals), if present.
    ///
    /// Returns a cheap owned clone (the runner is `Arc`-backed) so callers can
    /// fetch it without holding a borrow on `self` across the subsequent
    /// `notify`/`refresh_views`.
    ///
    /// GPUI: the `HookRunner` global. Daemon: the daemon's runner, or `None`.
    fn hook_runner(&self) -> Option<okena_hooks::HookRunner>;

    /// The hook monitor service (tracks in-flight/completed hook runs), if
    /// present. Returns a cheap owned clone (`Arc`-backed).
    ///
    /// GPUI: the `HookMonitor` global. Daemon: the daemon's monitor, or `None`.
    fn hook_monitor(&self) -> Option<okena_hooks::HookMonitor>;
}

impl WorkspaceCx for Context<'_, Workspace> {
    fn notify(&mut self) {
        // The inherent `Context::notify` shadows this trait method during
        // method resolution (inherent methods win), so this is a direct call
        // into GPUI — not recursion back into the trait impl.
        self.notify();
    }

    fn refresh_views(&mut self) {
        // Reaches `App::refresh_windows` through `Context`'s `DerefMut<App>`.
        self.refresh_windows();
    }

    fn hook_runner(&self) -> Option<okena_hooks::HookRunner> {
        // Reaches `App::try_global` through `Context`'s `Deref<App>`.
        self.try_global::<okena_hooks::HookRunner>().cloned()
    }

    fn hook_monitor(&self) -> Option<okena_hooks::HookMonitor> {
        self.try_global::<okena_hooks::HookMonitor>().cloned()
    }
}
