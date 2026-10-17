<!--
On close, prepend an OUTCOME block here, then `git mv` this file to ../archive/:

> **OUTCOME — shipped YYYY-MM-DD.** <one-paragraph result.> Commit map: WU1 → <sha>,
> WU2 → <sha>, … Verification: <the gate command + numbers>. Backlog closed:
> <ids deleted/rescoped>. Deferred: <honest notes>.
-->

# Sprint — native terminal view for iOS and Android (2026-10-17)

**Goal.** Replace the JS + Skia terminal renderer with a native view on each
platform (Swift on iOS, Kotlin on Android) that reads cells straight from the
Rust core, so scrolling and Claude Code output stay smooth at the display's
refresh rate.

**Theme.** Today every frame and every scroll step goes through the JS thread:
the gesture, the dirty check, the FFI copy of the grid, a per-cell paint loop
and the picture build. The fix is to take the terminal off that path. The rest
of the app (drawer, settings, pairing, key toolbar, soft keyboard) stays in
React Native. Success means pixel-smooth scrolling with momentum, no dropped
frames while Claude Code streams, and parity with today's Skia view on both
platforms.

## Refs re-verified at HEAD (2026-10-17, branch `feat/mobile-ux-polish`)

- ✔ Scroll is whole lines only, with no sub-line offset or momentum. Finger px
  are accumulated and released as whole lines in
  `mobile/rn/src/components/terminalScroll.ts:33` and sent through
  `native.scrollWheel` in `mobile/rn/src/components/TerminalPane.tsx:366-376`.
- ✔ Gestures run on the JS thread (RN responder system) in
  `TerminalPane.tsx:262-462`. This covers tap, drag-scroll, long-press
  selection, double-tap word and pinch.
- ✔ The repaint loop runs on the JS thread. A `requestAnimationFrame` calls
  `takeDirty` and then `setFrame` (React re-render), in
  `mobile/rn/src/components/TerminalView.tsx:508`.
- ✔ Each dirty frame copies the whole grid: `getVisibleCellsPacked`
  (`TerminalView.tsx:532`) goes through `get_visible_cells_packed`
  (`crates/okena-mobile-ffi/src/lib.rs:165`). That function first builds a
  `Vec<CellData>` with one `String` per cell
  (`crates/okena-mobile-ffi/src/client/terminal_holder.rs:58`), then packs it at
  13 B per cell.
- ✔ The paint is a JS loop over every cell, making JSI calls for each
  background rect and text run and allocating rects on every call:
  `paintTerminal`, `TerminalView.tsx:226`, then `createPicture` at line 543.
- ✔ Terminal emulation is `alacritty_terminal` 0.25 in Rust. Output is parsed
  on the tokio thread under a `parking_lot::Mutex` (`terminal_holder.rs:50`).
  This is not the bottleneck.
- ✔ The dirty signal is a single `AtomicBool` per terminal
  (`terminal_holder.rs:300`), with no per-row damage.
- ✔ Mouse mode is already split. `scroll_wheel` (`lib.rs:269`) sends wheel
  escapes when the app enabled mouse reporting (`terminal_holder.rs:206`), and
  scrolls the local scrollback otherwise.
- ✔ How the Rust library is linked:
  - iOS: a static library in `OkenaMobileFfiFramework.xcframework`, vendored by
    the generated `OkenaMobileFfi.podspec`, so one copy ends up in the app
    binary.
  - Android: `libokena_mobile_ffi.a`, linked statically into the generated
    `libokena-mobile-ffi.so` (`modules/okena-mobile-ffi/android/CMakeLists.txt`).
  - Generated module contents are gitignored, and fixes are applied by scripts
    after ubrn runs (`mobile/rn/CLAUDE.md`).
- ✔ RN 0.86.3 with the new architecture (Fabric) on. Skia 2.14.0. Android
  `minSdkVersion = 24`.
- ⚠ The Rust core has global state (the `ConnectionManager::get()` singleton).
  A second copy of the static library in another `.so` would create a second,
  empty singleton. The native view must therefore call into the same Rust
  copy the JSI module uses, which is the main technical risk (WU0).

## Work units

### WU0 — Baseline and linking spike (effort S, gate)

- **Problem.** We have no numbers for the current renderer, and we don't yet
  know how Swift and Kotlin can call into the one Rust copy.
- **Verify first.** Check whether `nm` on the xcframework and on the Android
  `.so` lists the uniffi symbols, and which of them are exported dynamically on
  Android.
- **Scope.**
  1. Baseline on real devices (an iPhone with ProMotion and one mid-range
     Android). Measure:
     - hitches during a scroll fling through 5k lines of scrollback,
     - dropped frames while Claude Code streams a long answer,
     - JS-thread frame time inside `paintTerminal` and in the FFI call.
     Use Instruments (Animation Hitches, Time Profiler) on iOS and
     `adb shell dumpsys gfxinfo` plus Perfetto on Android.
  2. Spike: add one `extern "C"` Rust function and call it from Swift and from
     Kotlin (JNI) while the JSI module is connected to a daemon. It must see
     the same connection.
     - iOS: a hand-written C header and Swift calling into the vendored static
       library.
     - Android: export `Java_*` (or a C entry plus a C++ JNI shim) from
       `libokena-mobile-ffi.so`. Use `-Wl,-u,<sym>` or `--whole-archive` in
       the post-ubrn fix script so the linker keeps the symbols.
- **Acceptance / witness.**
  - The baseline numbers are written to the run log.
  - On both platforms, a native call returns the terminal count of the
    connection that JS opened.
- **Touch points.**
  - `crates/okena-mobile-ffi/src/`
  - `mobile/rn/scripts/` (the post-ubrn fixes)
  - a scratch native view

### WU1 — Rust render API over a C ABI (effort M)

- **Problem.** The current snapshot copies the whole grid, allocates one
  `String` per cell, and can only read at alacritty's own `display_offset`.
  Smooth scrolling needs any line window at a scroll offset the view chooses,
  plus cheap change detection.
- **Verify first.** Check whether alacritty's `Term::damage()` is usable with a
  single consumer (one visible view per terminal, as with the chip switcher
  today). If it isn't, use per-row hashes kept next to the last snapshot.
- **Scope.** A new module `crates/okena-mobile-ffi/src/render.rs` with a small,
  stable C ABI (`okena_rv_*`), plus a header checked into the native view
  module.
  - `generation(conn, term) -> u64`: lock-free, bumped by `process_output`,
    `resize` and selection changes. The view polls this once per vsync.
  - `snapshot(conn, term, top_line: i32, rows: u16, out: *mut Frame)`:
    - Fills a Rust-owned, reused buffer. Each row has a row id/generation for
      caching and cells of {codepoint u32, extra-grapheme index, fg u32,
      bg u32, flags u16}.
    - Cell flags include WIDE and WIDE_SPACER.
    - Also fills cursor, selection range, history size, screen lines and mode
      bits (`MOUSE_MODE`, `ALT_SCREEN`, `ALTERNATE_SCROLL`).
    - Inverse and theme are resolved in Rust as today.
    - `top_line` is absolute, so reading does not change `display_offset`.
  - `free`/`release` for the frame. Selection calls in buffer coordinates:
    `start`, `update`, `select_word`, `copy_text`, `clear`. A `wheel` call for
    mouse mode (wrapping `mouse_wheel_input`).
  - `resize_local` and `resize_terminal` go through the same ABI, so the view
    owns sizing.
  - The old uniffi functions stay until WU8.
- **Acceptance / witness.** Rust unit tests in `render.rs` cover:
  - window at an offset into scrollback,
  - wide and combining characters,
  - generation bump on output,
  - changed-row detection,
  - selection text across rows,
  - mode bits after `\x1b[?1000h`.

  A micro-benchmark shows `snapshot` of 60×50 under 0.2 ms on desktop.
- **Touch points.**
  - `crates/okena-mobile-ffi/src/render.rs` (new)
  - `client/terminal_holder.rs`
  - `lib.rs`

### WU2 — Fabric component contract and RN integration (effort S)

- **Problem.** RN needs one component with the same API on both platforms, and
  a way to fall back to Skia while the native view matures.
- **Scope.**
  - New versioned local package `mobile/rn/modules/okena-terminal-view` (not
    generated, not gitignored), with a codegen spec `OkenaTerminalView`:
    - Props: `connId`, `terminalId`, `fontSize`, `theme` (colors), `focused`.
    - Events:
      - `onTap {col,row}`, which focuses the hidden TextInput in JS,
      - `onGridSize {cols,rows,cellW,cellH}`,
      - `onFontSizeChange {size}` (from pinch; JS persists it in
        `settingsStore`),
      - `onSelectionCopied {text}`,
      - `onScrollStateChange {atBottom}`.
    - Commands: `scrollToBottom`, `clearSelection`.
  - `TerminalPane` renders either `OkenaTerminalView` or today's `TerminalView`
    based on an app setting `renderer: 'native' | 'skia'`. The default is
    `native` once WU7 passes, until then `skia`.
  - The hidden TextInput, `softInput.ts` and `KeyToolbar` stay in JS
    unchanged. With the native view, JS no longer handles scroll, pinch or
    selection gestures.
- **Acceptance / witness.**
  - typecheck, lint and jest pass.
  - A jest test for `TerminalPane` checks that the switch picks the right
    renderer, and that `onTap` focuses input and `onFontSizeChange` writes the
    setting.
- **Touch points.**
  - `modules/okena-terminal-view/{package.json,src/,ios/,android/}`
  - `src/components/TerminalPane.tsx`
  - `src/state/settingsStore.ts`
  - `src/screens/SettingsSheet.tsx` (renderer toggle, hidden under "advanced")

### WU3 — iOS renderer (effort L)

- **Scope.**
  - `OkenaTerminalView: UIView` hosting a `UIScrollView`. Its content height is
    `(history + screen) × cellH` and it is used only for physics: momentum,
    rubber band, tap the status bar to scroll to top.
  - Rows are recycled `CALayer`s, one per visible row plus one above and one
    below.
  - A row bitmap is redrawn with Core Text only when that row's id or
    generation changes. Scrolling only moves layers, so Core Animation does it
    on the render server at 120 Hz.
  - A `CADisplayLink` checks `generation` once per vsync. While idle the view
    does no work.
  - Font: bundled JetBrains Mono plus Core Text cascade fallback, with the
    bundled Noto Sans Symbols 2 first in the cascade list. Most tofu goes away
    without our own fallback code. Wide cells come from Rust flags.
  - Cursor (block, beam, underline), selection overlay and scroll indicator.
  - Sizing: cols and rows from glyph advance and line height in `layoutSubviews`,
    then `resize_local`, an `onGridSize` event and a 200 ms debounced
    `resize_terminal` (as today in `TerminalView.tsx`).
- **Acceptance / witness.**
  - On a ProMotion iPhone, a fling through 5k lines shows 0 hitches over 5 ms
    in Instruments Animation Hitches.
  - While Claude Code streams, the main thread stays under 4 ms per frame.
  - The glyph test script from the glyph sprint shows no tofu.
- **Touch points.** `modules/okena-terminal-view/ios/*.swift`, the podspec.

### WU4 — iOS gestures and scroll model (effort M)

- **Scope.**
  - **Live tail.** At the bottom, new output keeps the view pinned. When
    scrolled up, the content stays anchored as history grows (same as
    desktop).
  - **Mouse mode.** When `MOUSE_MODE` is on, or `ALT_SCREEN` with
    `ALTERNATE_SCROLL`, the pan becomes discrete wheel notches through `wheel`.
    Smooth scroll is not possible here because the app scrolls itself.
  - **Pinch.** Live font scaling, with `onFontSizeChange` on end. Range 8–20 pt,
    in steps of 0.5.
  - **Selection.** Long press starts a selection, drag extends it and
    auto-scrolls at the edges, release copies to `UIPasteboard` and emits
    `onSelectionCopied`.
  - **Taps.** Double tap selects a word, single tap emits `onTap`.
  - VoiceOver: expose rows as accessibility elements (basic, line text only).
- **Acceptance / witness.**
  - Maestro flows (scroll, pinch, select-copy, tap-to-focus) pass on the
    iOS 26.5 and iOS 27 simulators.
  - A manual pass on a device with Claude Code covers scrolling during
    streaming and selecting a code block.
- **Touch points.** `modules/okena-terminal-view/ios/`, `.maestro/` flows.

### WU5 — Android renderer (effort L)

- **Scope.**
  - `OkenaTerminalView: View` with `OverScroller`, `VelocityTracker` and
    `EdgeEffect` for fling physics, plus `Choreographer` for frame callbacks
    and the `generation` check.
  - Rows are drawn with hardware-accelerated `Canvas.drawText` and the
    JetBrains Mono `Typeface`.
  - Per-row cache: `RenderNode` on API 29+, direct draw on API 24–28.
  - Font fallback through `Typeface.CustomFallbackBuilder` (API 29+), with the
    bundled Noto Sans Symbols 2 and then the system default.
  - Access to Rust is a C++ JNI shim inside `libokena-mobile-ffi.so`, as proven
    in WU0. The frame buffer is wrapped as a direct `ByteBuffer`, so there is
    no copy into the JVM heap.
  - Cursor, selection overlay, scroll indicator and sizing as on iOS.
- **Acceptance / witness.**
  - On a mid-range Android, `dumpsys gfxinfo` during a 5k-line fling shows
    fewer than 1 % janky frames.
  - While Claude Code streams, the UI thread stays under 8 ms at the 90th
    percentile.
- **Touch points.**
  - `modules/okena-terminal-view/android/**/*.kt`
  - the JNI shim
  - the post-ubrn fix script

### WU6 — Android gestures and scroll model (effort M)

- **Scope.** Same behaviour as WU4:
  - live tail and anchoring,
  - mouse-mode wheel,
  - pinch via `ScaleGestureDetector`,
  - long-press selection with edge auto-scroll, copy through
    `ClipboardManager`,
  - double tap selects a word, tap emits `onTap`,
  - basic TalkBack.
- **Acceptance / witness.** The same Maestro flows pass on an Android emulator,
  plus a manual pass on a device with Claude Code.
- **Touch points.** `modules/okena-terminal-view/android/`.

### WU7 — Parity, measurement, rollout (effort M)

- **Scope.**
  - Repeat the WU0 measurements on the same devices and record before and after
    in the run log.
  - Parity checklist against the Skia view:
    - colors and dim,
    - bold and italic,
    - underline and strikethrough,
    - wide CJK and emoji,
    - combining accents,
    - cursor shapes,
    - selection across scrollback,
    - font setting,
    - terminal switching with the chips,
    - session restore.
  - Switch the default renderer to `native`. Ship TestFlight and a Play
    internal build.
- **Acceptance / witness.**
  - The before/after numbers meet the WU3 and WU5 targets.
  - The checklist is all green on both platforms.
  - The builds are uploaded.
- **Touch points.** `settingsStore.ts`, release scripts.

### WU8 — Remove the Skia renderer (effort S, after about a week on TestFlight)

- **Scope.**
  - Delete `TerminalView.tsx`, `terminalFonts.ts` and `terminalGlyphs.ts` (if
    nothing else uses them), plus `src/native/cells.ts`, the renderer setting,
    and the uniffi functions `get_visible_cells(_packed)`, `take_dirty` and
    `is_dirty`.
  - Drop `@shopify/react-native-skia` if `Icon.tsx` moves to `react-native-svg`
    or to native icons. That is a separate decision; keep Skia for icons if it's
    cheaper.
- **Acceptance / witness.**
  - typecheck, lint, jest and `cargo test -p okena-mobile-ffi` pass.
  - The app size goes down, measured and logged.
- **Touch points.** Listed above, plus `src/native/okena.ts`.

## Out of scope (explicit)

- **Rewriting other screens natively.** They have no performance problem.
- **Moving keyboard input to native.** `softInput.ts` was just fixed and is not
  on the hot path.
- **A shared Rust GPU renderer (wgpu).** It would mean one renderer for both
  platforms, but text shaping and font fallback would move into Rust as well.
  Revisit only if keeping two renderers in parity turns out expensive.
- **Metal on iOS.** Core Text row layers should be enough. Revisit only if WU3
  misses its target.
- OSC window titles, Nerd Font glyphs, iPad split view and hardware-keyboard
  shortcuts.

## Decisions

- **Native view inside RN, not a full Swift rewrite.** The shared core is
  already in Rust, and Swift on Android would share the same logic a second
  time while UI would still differ per platform.
- **A hand-written C ABI for the hot path** instead of uniffi Swift/Kotlin
  bindings:
  - it keeps a single Rust copy and a single singleton,
  - frames are zero-copy,
  - there is no JNA on Android,
  - the surface is about 12 functions.
- **The native view owns scroll offset, gestures and sizing.** JS gets events
  only. Snapshots are read at an explicit `top_line`, so the view never has to
  push the scroll position back into alacritty.
- **iOS uses `UIScrollView` for physics plus Core Text row layers. Android uses
  `OverScroller` plus Canvas with `RenderNode` caching.** These are the
  platform-native scroll feel and the simplest renderers that should reach the
  targets.
- **A renderer toggle during rollout**, removed in WU8.
- At close, write an ADR "Native terminal renderer per platform over a C ABI"
  (it supersedes the Skia choice in `mobile/RN_MIGRATION.md`).

## Sequencing

| Step | Work | Parallel |
|---|---|---|
| 1 | WU0 (gate: linking works on both platforms) | none |
| 2 | WU1 + WU2 | both at once |
| 3 | WU3 → WU4 (iOS) and WU5 → WU6 (Android) | two agents or two people at once |
| 4 | WU7 | after both platforms |
| 5 | WU8 | about a week after rollout |

Estimate for one person: about 4–5 weeks. With iOS and Android in parallel
after step 2: about 3 weeks of calendar time. The biggest uncertainty is the
Android linking in WU0 and font fallback parity.

## Run log

<!-- Append as you work: discoveries, deviations, blockers. Graduate each entry:
     changed the *why* → ../decisions/NNNN ; new future work → ../backlog/NN ;
     transient → leave it (dies with the sprint on archive). After graduating,
     trim to a one-line pointer ("→ ADR-0007"). -->

- **WU2 (2026-10-17).** Package `mobile/rn/modules/okena-terminal-view` with placeholder
  native views. Deviations from the plan:
  - `onGridSize` uses `cellWidth`/`cellHeight`, not `cellW`/`cellH`.
  - `theme` is `{background, foreground, cursor, selection}` as packed unsigned ARGB doubles
    (Int32 cannot hold opaque ARGB). The ANSI palette is not passed because Rust resolves cell
    colors.
  - The native `focused` prop is named `focusedPane` on the Swift/Kotlin view classes, because
    `UIView.focused` already exists.
  - JS never resizes the PTY in native mode. It sends `scrollToBottom` after soft-keyboard and
    key-toolbar input (`TerminalPaneHandle.scrollToBottom`), because the native view keeps its
    own scroll offset.
  - Android sizes are in dp, the same as the Skia view.
