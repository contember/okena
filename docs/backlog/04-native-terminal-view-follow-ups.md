---
id: 04
title: Native terminal view follow-ups
blocked-by: []
---

# 04 — Native terminal view follow-ups

**Summary.** Gaps found in the WU7 parity pass of the native terminal view
(iOS + Android) that are bigger than a parity tweak. None blocks the rollout;
the first one is the most visible.

## Problem

1. **Resize ownership with two clients.** The daemon has one resize owner for
   all terminals ("last to interact wins"). When another client owns it, a
   phone's resize (font change, keyboard, rotation) is denied and the server
   puts the old grid back. Both views then show a grid that does not fit: right
   columns cut off, or blank rows at the top, until the user types (input claims
   ownership and the view re-sends its size). Reproduced on the Android
   emulator with the iOS sim typing into the same daemon; once the whole
   Android view looked blank except for the cursor. Skia behaves the same.
2. **Emoji graphemes split into cells.** Skin-tone modifiers (`👍🏽`),
   regional-indicator flags (`🇨🇿`) and ZWJ families (`👨‍👩‍👧`) arrive as
   separate cells from the Rust core, so both views draw the parts side by side
   (iOS: small boxed letters for the flag, Android: `cz`).
3. **Underline styles.** Curly, dotted and dashed underlines are drawn as a
   single line on both platforms (double underline is drawn).
4. **Unfocused cursor.** `focused` does not change the cursor (e.g. a hollow
   block in an unfocused pane), on either platform or in Skia.
5. **Scroll feel constants differ.** The same swipe moves `less` by about 9
   lines on iOS and 11 on Android; selection edge auto-scroll tops out at 1.5
   lines per frame on iOS and 0.6 on Android.
6. **Hidden text after session restore.** Text with SGR 8 shows up after a
   restore (both renderers): the restored screen seems to lose the attribute.

## Approach / acceptance

1. Decide between per-terminal resize ownership in the daemon, or the view
   fitting a foreign grid (scale down or letterbox, plus a hint). Witness: two
   clients on one daemon, font change on the non-owner, the view still shows
   the whole grid.
2. Fix grapheme widths in the Rust snapshot (one WIDE cell with an extra
   grapheme for the whole cluster). Witness: a `render.rs` unit test for each
   of the three cases, plus screenshots on both platforms.
3. Draw the styles in `TerminalRowPainter.swift` and `RowPainter.kt` from the
   existing cell flags.
4. Pick a cursor style for unfocused panes and apply it on both platforms.
5. Share one px-to-notch and auto-scroll rule between the two views.
6. Check what the session snapshot keeps of cell flags on the daemon side.

## Touch points

- `crates/okena-mobile-ffi/src/render.rs`, the daemon's resize authority
- `mobile/rn/modules/okena-terminal-view/ios/*.swift`
- `mobile/rn/modules/okena-terminal-view/android/**/*.kt`

<!-- Origin: ../sprints/sprint-2026-10-17-native-terminal-view.md, WU7 run log. -->
