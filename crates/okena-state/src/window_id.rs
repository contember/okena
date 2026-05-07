//! Stable identity for a window onto the shared workspace.
//!
//! `WindowId::Main` addresses the always-present main window slot
//! (`WorkspaceData.main_window`); `WindowId::Extra(_)` addresses an entry in
//! `WorkspaceData.extra_windows`. The `Main` variant carries no payload because
//! the main slot is a compile-time invariant — there is exactly one main window
//! and closing it quits the app. See PRD `plans/multi-window.md`.

use uuid::Uuid;

/// Identifies a single window. Used by the upcoming window-scoped mutation API
/// so callers can target a specific window's state inside `WorkspaceData`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowId {
    /// The main window. Always present in persistence; closing it quits.
    Main,
    /// An extra window, identified by a UUID assigned at spawn time.
    Extra(Uuid),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn main_equals_main() {
        assert_eq!(WindowId::Main, WindowId::Main);
    }

    #[test]
    fn main_differs_from_any_extra() {
        let extra = WindowId::Extra(Uuid::new_v4());
        assert_ne!(WindowId::Main, extra);
    }

    #[test]
    fn distinct_extras_are_not_equal() {
        let a = WindowId::Extra(Uuid::new_v4());
        let b = WindowId::Extra(Uuid::new_v4());
        assert_ne!(a, b);
    }

    #[test]
    fn same_uuid_extras_are_equal() {
        let id = Uuid::new_v4();
        assert_eq!(WindowId::Extra(id), WindowId::Extra(id));
    }

    #[test]
    fn usable_as_hashmap_key() {
        let mut map: HashMap<WindowId, &'static str> = HashMap::new();
        let extra_id = Uuid::new_v4();
        map.insert(WindowId::Main, "main");
        map.insert(WindowId::Extra(extra_id), "extra");

        assert_eq!(map.get(&WindowId::Main).copied(), Some("main"));
        assert_eq!(map.get(&WindowId::Extra(extra_id)).copied(), Some("extra"));
        assert!(map.get(&WindowId::Extra(Uuid::new_v4())).is_none());
    }

    #[test]
    fn copy_semantics() {
        // WindowId is Copy so callers can pass it around without clone() noise.
        // Pin this here so a future struct-with-non-Copy-payload variant has to
        // own the breakage.
        let original = WindowId::Extra(Uuid::new_v4());
        let copy = original;
        assert_eq!(original, copy);
    }
}
