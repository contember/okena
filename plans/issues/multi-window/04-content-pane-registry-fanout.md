---
title: Content pane registry fan-out for shared terminals
status: done
type: AFK
blocked-by: [03-windowview-rename-and-per-window-focus]
user-stories: [8, 32]
---

## What to build

The content-pane registry maps `terminal_id` to a weak handle for the `TerminalContent` entity that should be notified when PTY data arrives. Today it stores a single `WeakEntity` per terminal — fine when only one window can render a terminal at a time. With multiple windows, the same terminal can render in N project-column instances simultaneously (any window whose visible set includes the project hosting that terminal). Notifying only one of them stalls the others' visual updates.

Change the registry value type from `WeakEntity<TerminalContent>` to `Vec<WeakEntity<TerminalContent>>`. PTY event delivery iterates the vec and notifies each live entry. Dead weak references are dropped lazily on iteration so the vec doesn't grow unbounded as windows close.

Registration helpers update accordingly:

- Register: append to the vec for that `terminal_id`. If the vec was empty, the terminal becomes "alive" in the registry.
- Unregister (on `TerminalContent` drop): remove the matching weak by pointer-equality. When the vec is empty, remove the key.
- Iteration during PTY notify: walk the vec, for each weak attempt `update`; if the weak is dead, mark for removal. After iteration, prune dead entries.

Detached-terminal popups already use this same registry for their PTY routing — they piggyback the fix automatically and continue to work without modification.

After this slice the app is still single-window in the user's experience, but the registry is now safe for the multi-window slice to land on.

## Acceptance criteria

- [x] `content_pane_registry` value type is `Vec<WeakEntity<TerminalContent>>`.
- [x] PTY event loop's per-terminal notify path iterates the vec, attempts `update` on each, prunes dead entries.
- [x] Registration appends; unregistration removes the matching entry; empty vecs cleaned up.
- [x] Existing single-window terminal behavior unchanged: typing in a terminal updates that pane, no double-fires, no missed updates.
- [ ] Detached-terminal popups continue to work: opening a terminal in a popup, typing in the source pane, both reflect the same content.
- [x] Unit/integration test: register two `WeakEntity`s for the same `terminal_id`, fire a notification, both targets receive the call. Drop one entity, fire again, only the live one is called and the dead entry is gone afterward.
- [x] `cargo build` and `cargo test` both green.

## Notes

- The registry lives in the views module today; keep it there.
- PTY event delivery happens on the GPUI thread inside `Okena::start_pty_event_loop`. The fan-out happens inside the existing `cx.update` block — no thread-safety changes needed beyond what already exists.
- This slice is small but earns its own file: it's load-bearing for slice 05's correctness and is straightforward to test in isolation.
