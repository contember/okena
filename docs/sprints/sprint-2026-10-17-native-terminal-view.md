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

### 2026-10-17 — WU0 linking spike (gate passed) and WU1

- **Verify first.** Before the change, the xcframework exported all uniffi
  symbols. The Android `.so` exported the uniffi C symbols dynamically only
  because the generated C++ references them. A Rust `#[no_mangle]` function
  that no C++ code references is dropped from the static archive at link
  time.
- **Contract location.** `crates/okena-mobile-ffi/include/okena_render.h` plus
  `module.modulemap` (Clang module `OkenaRender`). The JNI shim is
  `crates/okena-mobile-ffi/jni/okena_render_jni.cpp`. The plan said "header
  checked into the native view module". It lives in the crate instead,
  because the Android shim must compile into `libokena-mobile-ffi.so` and the
  view package did not exist yet. The view pod points `SWIFT_INCLUDE_PATHS`
  at that directory.
- **Android linking.** `scripts/fix-android-bindings.cjs` adds the shim to the
  generated `CMakeLists.txt` with absolute paths (the generated module is
  reached through a `node_modules` symlink, so relative paths break), adds
  the include dir, and adds `-Wl,--undefined-glob=okena_rv_*` so every entry
  point is kept and exported. JNI class: `com.okena.terminalview.RustBridge`.
- **Witness.**
  - iOS sim (iPhone 17 Pro Max, 26.5): a temporary Swift pod imported
    `OkenaRender` the way the view pod will. It logged
    `conn=<JS conn id> terminal_count=1`, and after WU1 it logged a snapshot:
    `cols=69 screen=28 top=20001 first=10001 hist=10000` after `seq 1 20000`,
    so the counter keeps counting past the 10k limit.
  - Android emulator (HN_Local, arm64): Kotlin `RustBridge` logged
    `terminal_count=1` and a snapshot read from a direct `ByteBuffer`.
  - The spike code was removed and is not committed.
- **Baseline (simulator/emulator numbers, NOT devices).** Physical devices
  were offline. JS-thread time per painted frame of today's Skia path, from
  temporary timing in `TerminalView.tsx` (removed). `ffi` is
  `getVisibleCellsPacked` plus decode. `paint` is `createPicture` with
  `paintTerminal`. The grid was 69×45 on iOS without the keyboard and 69×28
  with it. Times in ms, as p50 / p95 (max):

  | Platform | Scenario | ffi | paint | Painted frames (per 3 s window) |
  |---|---|---|---|---|
  | iOS sim | `seq 1 100000` burst | 0.16 / 0.4–5 | 10.5 / 11 (19) | 2–17 (output coalesced) |
  | iOS sim | sustained stream (`echo`, sleep 5 ms) | 0.14 / 0.2 (4.5) | 11.1 / 12 (22) | ≥ 60 |
  | iOS sim | drag-scroll, 16 swipes | 0.16 / 0.2 | 11.3 / 12 (17) | 31–47 |
  | Android emu | `seq 1 100000` burst | 0.17 / 1–26 | 11.6 / 29–32 | 3–16 |
  | Android emu | sustained stream | 0.17 / 0.5–2.8 (13.6) | 11.4 / 22–35 (81) | ≥ 60 |
  | Android emu | drag-scroll | 0.16 / 0.3–0.8 (11) | 9.7 / 20–24 | 43–59 |

  The sampler flushed every 60 frames or every 3 s, so "≥ 60" means a full
  window filled within 3 s. `paint` alone takes about 11 ms per frame on the
  JS thread, which leaves little of a 16.7 ms (or 8.3 ms ProMotion) budget. The FFI copy is small next to it. Repeat on devices in WU7.
- **WU1 decisions.**
  - **Damage vs. row hash.** We use row hashes. `Term::damage()` is
    viewport-relative, needs `&mut` plus a reset (one consumer only), and
    reports full damage on every scroll, which is every line of streaming
    output. The hash is computed while the row is copied anyway.
  - **Absolute lines.** alacritty has no scrolled-off counter. While the
    display offset is non-zero, `Grid::scroll_up` raises it by each scrolled
    line (alacritty's own `clear_viewport` reads it that way). So
    `process_output` parks the offset at 1 around each parser step of at most
    4096 bytes, and below the scrollback limit, reads the delta and restores
    the old API's offset. With an empty history it uses history growth.
    Alt-screen switches inside a step are settled when the primary grid is
    active again. Resize adjusts by the history delta and bumps
    `reflow_epoch`. Known limit: a scroll region that does not start at row
    0 on the primary screen while the history is full over-counts (rare;
    alt-screen TUIs are unaffected).
  - **Memory.** One reused, Rust-owned frame blob per handle, a single
    allocation in 8-byte words. The native side reads it in place (Swift
    through pointers, Kotlin through one cached direct `ByteBuffer`).
    Nothing is freed by the caller except `okena_rv_close`.
  - **Benchmark.** `snapshot` of 60×50 takes 19.5 µs on desktop (release,
    M-series), against the 200 µs target. Run it with `cargo test -p
    okena-mobile-ffi --release -- --ignored snapshot_bench --nocapture`.
- **Environment gotcha.** After the disk crash, `Pods/React-Core-prebuilt`
  held the Release React core while the Debug "replace" phase assumed Debug
  ("No previous build detected"). The Debug link then failed on missing
  `facebook::react::Sealable`/`Props` symbols. Fix: write `Release` into
  `Pods/React-Core-prebuilt/.last_build_configuration` (and the same for
  `ReactNativeDependencies`) so the next Debug build swaps the right
  framework in.
- **JS logs.** In this setup `console.log` did not reach `log stream` at all,
  so the timing went out by `fetch` to a local HTTP sink instead (with
  `adb reverse` on Android).

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

### 2026-10-17 — WU3 + WU4 iOS renderer and gestures

- **Code.** `modules/okena-terminal-view/ios/TerminalRendererView.swift` (scroll model, frame
  loop, sizing, overlays, gestures, lifecycle) and `TerminalRowPainter.swift` (fonts, glyph
  fallback, row bitmaps). No shared files changed (header, `render.rs`, TS, codegen spec).
  iOS-only build changes: the view podspec adds the `include` dir to its Swift and header
  search paths; `NotoSansSymbols2-Subset.ttf` is now an app resource in `project.pbxproj`
  (it was only a Metro asset for Skia).
- **Deviations.**
  - The cell height is rounded to whole device pixels (at most half a pixel off Skia's
    `measureCell`), so cached row bitmaps stay sharp wherever a row moves. Rarely this
    gives one row more or less than Skia.
  - Live tail = the screen's top row at the top edge (as Skia draws it). When another
    client made the PTY taller than the view, the rest of the screen is reached by scrolling
    down instead of being cut off.
  - Double tap keeps the word highlighted for 0.35 s before clearing (Skia cleared at once).
  - VoiceOver: one element whose label is the visible lines (not one element per row). It
    also lets Maestro match terminal text.
  - Cell backgrounds are written straight into the bitmap: CG rect fills were the largest
    cost in a Time Profiler run.
- **Verified on the iPhone 17 Pro Max sim, iOS 26.5** (native renderer on, isolated daemon),
  by screenshots and Maestro flows (kept in the scratchpad: the repo has no Maestro folder):
  `ls --color`, 256 colours and truecolor, bold/italic/dim/underline/strike/inverse/hidden,
  CJK, emoji, combining accents, box drawing and `⎿`, beam/underline/block cursors, fling
  through `seq 1 20000` with momentum, anchoring while a stream runs with the view scrolled
  up (two screenshots seconds apart are identical, toolbar Return then snaps to the live
  line), long-press drag selection with edge auto-scroll into scrollback and copy, double
  tap copies a word, font size from Settings, chip switching between two terminals,
  session restore after relaunch, soft keyboard and key toolbar input, `less` on the alt
  screen scrolls by arrows both ways while the scroll view stays put, background and
  foreground during a stream.
- **iOS 27.0 sim** (new device "NTV iPhone 17 Pro Max (27)"): same build, paired, style
  test, fling and scrollback selection pass.
- **Not verified.** Pinch (Maestro has no pinch on iOS; it shares the font-size path that
  the Settings test covers), mouse-mode apps such as `vim` with `mouse=a` (same code path
  as `less`), the MISSING-terminal state, VoiceOver by ear.
- **Parity vs. Skia (side by side with the toggle).** Same layout, colours and glyphs.
  Differences, all where Skia is wrong: Skia draws inverse text without the swapped
  background (it swaps again what Rust already swapped), drops combining marks, and draws
  `❤️` as a text glyph. Remaining gaps on both: every underline style is drawn as a single
  line and underlines skip blank cells; `focused` does not change the cursor. The selection
  tint sits over the text instead of under it (alpha 0x40, barely visible).
- **Performance (simulator numbers, 60 Hz, NOT device numbers).** From the opt-in perf log
  (`defaults write com.contember.okena OkenaTerminalPerfLog -bool YES`): `update` is the main
  thread time of one snapshot plus row redraws; `busy` is one main run-loop pass including
  layout and the Core Animation commit. p50 / p95 (max), ms.

  | Sim | Scenario | update | busy | Long frames (>1.5 vsync) |
  |---|---|---|---|---|
  | iOS 26.5 | fling, 12 fast swipes | 0.23–0.31 / 0.43–0.67 (1.1) | — | — |
  | iOS 26.5 | stream, ~85 coloured lines/s | 0.45–0.54 / 0.6–1.2 (4) | 0.7–1.6 / 1.9–2.3 (8) | 0 |
  | iOS 26.5 | same, history at the 10k limit | 1.0–1.2 / 1.4 (7.6) | 1.5–1.6 / 1.9–2.3 (8) | 0 |
  | iOS 27.0 | fling | 0.24–0.33 / 0.6–0.85 (1.5) | 0.6–0.8 / 1.2–2.0 (2.8) | 0 |

  Against the Skia baseline above (11 ms of JS-thread paint per frame), the native view
  uses about 0.5 ms of main-thread time per changed frame and nothing while idle. Windows
  with Maestro running show 20–30 ms passes from XCTest snapshots; they are excluded.
  A 10 s Time Profiler run of the stream: 466 ms of main-thread samples in total, 184 ms
  in `updateViewport`, down from 726 ms and 479 ms before moving background fills out of
  CG and keeping the content view's frame fixed. The first draw of uncommon glyphs (emoji,
  CJK) once took 225 ms on the iOS 27 sim (font cascade lookup on first use).
  `xctrace` Animation Hitches does not run on the simulator ("Hitches is not supported on
  this platform"), so the WU3 hitch target needs a device (WU7).
- **Environment.**
  - `node_modules/okena-terminal-view` (the `file:` link) was missing; `pod install` then
    silently dropped the pod and Metro could not resolve the module. Recreated the symlink
    by hand.
  - With the Android emulator connected, Maestro needs `--device <UDID>`; without it the
    first flows here may have gone to the emulator.
  - Maestro's iOS hierarchy loses most of the app while the drawer is open, sometimes
    (reproduced with the Skia renderer too, mostly with the soft keyboard up). Flows tap the
    drawer's Settings row by position instead of by text.
  - Soft-keyboard input via Maestro sometimes lost the first character or kept one from the
    previous flow (e.g. `qclear`). Not investigated; `softInput.ts` is outside this WU.
  - After session restore, text with SGR 8 (hidden) showed up (both renderers): the restored
    screen seems to lose the attribute.

- **WU5/WU6 Android (2026-10-17, branch `wip/ntv-android`).** `TerminalRendererView.kt` plus
  `RowPainter`, `ScrollModel`, `RvFrame`, `TerminalFonts`, `TerminalAccessibility` and
  `RustBridge` in `modules/okena-terminal-view/android`. Verified on the emulator (HN_Local,
  arm64) against the isolated daemon with the native renderer on. Maestro flows and
  screenshots stay in the session scratchpad.
  - **Works.** 16/256/truecolor, bold/italic/dim/underline/double underline/strike/inverse/
    hidden; CJK, emoji, combining marks; box drawing and `⎿`; Claude Code symbols without
    tofu; cursor block/beam/underline; fling through `seq 1 20000` with edge effects;
    anchoring while streaming scrolled up; long-press selection with edge auto-scroll and
    copy across scrollback; double-tap word; font setting; chips; session restore; soft
    keyboard and key toolbar typing snap to the bottom; `less` (alt screen, 1007) and vim
    `mouse=a` scroll through `wheel`; TalkBack reads one node per row.
  - **Not verified.** Pinch: the emulator image has no root, so two-finger input could not
    be injected. The font-size path it ends in is verified through Settings.
  - **Deviations from Skia, on purpose.** Cell width is the font's real advance (0.6 em,
    16.5 px at 10.5 dp); Skia hints the advance to a whole dp (18.4 px), so native fits 67
    columns where Skia fits 58 (Settings already estimates 65). Inverse, hidden and combining
    marks render correctly (Skia double-swapped inverse, showed hidden text and dropped
    combining marks). An underline continues across spaces. Ligatures are off, as in Skia.
    Like Skia, the grid cells use the Rust default bg (#1e1e1e) and the margins the theme bg.
  - **Resize authority.** The daemon has one resize owner for all its terminals ("last to
    interact wins", claimed by input), so a second client's resize is denied and the server
    puts the grid back. The view re-sends its size (at most twice per size) and again after
    `scrollToBottom`, which JS sends after input. When the grid stays taller than the view,
    the live tail shows the grid's bottom rows.
  - **Measurements (EMULATOR, software GPU: SwANGLE + llvmpipe; `-gpu host` does not boot
    headless).** `dumpsys gfxinfo framestats`, last 120 frames, ms p50/p90/p95 (max).
    UI thread = input + traversal + `onDraw` recording; render thread = rasterisation.

    | Scenario | UI thread | onDraw record | Render thread |
    |---|---|---|---|
    | fling through 20k lines (12 swipes) | 0.75 / 4.4 / 7.0 (25) | 0.09 / 0.27 / 0.50 | 63 / 99 / 113 |
    | sustained stream (`printf` + sleep 5 ms) | 0.66 / 2.9 / 7.0 (12) | 0.34 / 1.8 / 3.4 | 39 / 52 / 65 |
    | idle, 5 s | 5 frames | | |

    Against the Skia baseline above (11 ms p50 of JS-thread paint per frame), the UI-thread
    cost per frame is about 15× lower. Janky-frame percentages are meaningless here: every
    frame misses the deadline in software rasterisation, whatever draws it. The WU5 targets
    (< 1 % janky fling, UI thread < 8 ms p90 while streaming) need a device run in WU7; the
    UI-thread half already holds on the emulator.

### 2026-10-17 — WU7 parity, measurement and rollout prep (merged branch)

- **Builds.** From `feat/native-terminal-view` after the Android merge: Rust bindings
  regenerated (`ubrn:ios`, `ubrn:android`; `render.rs` was newer than the libraries),
  `pod install`, iOS Debug on the 26.5 sim with Metro, Android release-style APK re-signed
  with the debug keystore (scratchpad `and-build.sh`/`and-install.sh`, now pointing at this
  worktree). Both apps were paired with the isolated daemon and ran at the same time, one
  project each.
- **Default renderer.** `settingsStore` defaults to `native`. Version 1 settings wrote the
  old `skia` default on every font-size change, so a stored renderer could not mean an
  explicit choice. The object now carries `version: 2`, and a renderer is only read back
  from version 2 on: every existing install moves to native once, later choices stick. The
  Settings toggle stays as the Skia fallback. Witness on the sim: a seeded
  `{"terminalFontSize":10.5,"renderer":"skia"}` loads as "Native view". Jest 194 pass.
- **Parity fixes (both platforms now alike).**
  - Selection: baked into the row under the text and over cell backgrounds, as Skia's
    pass 1 drew it (both platforms drew it over the text before). The row's selected span is
    part of the row cache key (iOS row layers, Android RenderNodes); the iOS overlay layers
    are gone.
  - Underline and strike: a separate pass over every cell that carries them, so they cover
    spaces on both (iOS skipped blank cells, Android only covered spaces inside a glyph run).
    iOS now draws the double underline Android drew.
  - iOS re-sends its size when the daemon kept another client's size, at most twice per size
    and again after input, as Android did (the daemon has one resize owner).
  - iOS double-tap word highlight 0.25 s, as on Android.
  - iOS draws `█` as a pixel-snapped rect over the glyph's vertical extent: runs of it showed
    faint vertical seams (Claude Code progress bars, logo). Android had none.
- **Verified on both (iOS 26.5 sim and Android emulator, native renderer as default).**
  16/256/truecolor, bold/italic/dim/underline/double/strike/inverse/hidden; CJK, emoji,
  combining accents; box drawing and Claude Code glyphs (`⎿ ● ✻ ⏺`, braille spinner);
  beam/underline/block cursors; fling through `seq 1 20000`; anchoring while streaming (two
  screenshots 3 s apart identical while scrolled up); long-press drag selection with copy
  ("Copied", Android clipboard preview); font size from Settings; chip switching; session
  restore after killing the app; soft keyboard typing including Czech diacritics
  (`příliš žluťoučký kůň ĚŠČŘŽ`, Maestro `inputText`); key toolbar Arrow up + Return;
  `less` and vim `mouse=a` scroll through `wheel`; Skia ↔ native toggled 6 times on each
  (8 on Android) with no crash and no leak (iOS `heap`: one `OkenaTerminalRendererView`;
  Android `dumpsys meminfo`: Views stays at 96 across further toggles); background and
  foreground during a stream (output keeps coming); disconnect from the drawer and reconnect
  to the server (terminal content back, native view).
- **Claude Code-like TUI** (`$S/dhome/cc.py`: redraws a 6-line region with spinner, star,
  progress bar and box at 30 Hz, cursor-up + clear like ink, while appending coloured
  transcript lines with diff backgrounds, CJK, emoji and Czech). Output correct on both;
  scrolled up during it the view stays anchored; a selection made during it stays on its
  text while new lines push it up.
- **Measurements (simulator / emulator, NOT devices).** Before = Skia (WU0); WU3/WU5 = the
  platform agents' native runs; WU7 = this merged build, same methods (iOS: opt-in perf log,
  `update` = snapshot + row redraws, `busy` = one main run-loop pass; Android:
  `dumpsys gfxinfo framestats`, UI thread = input + traversal + `onDraw` recording).
  ms p50 / p95 (max), Android p50 / p90 / p95 (max).

  | Platform | Scenario | Before (Skia, JS thread paint) | WU3/WU5 native | WU7 merged native |
  |---|---|---|---|---|
  | iOS sim | fling / drag through scrollback | 11.3 / 12 (17) | update 0.23–0.31 / 0.43–0.67 | busy 0.64–0.87 / 1.25–2.6; 0 long frames |
  | iOS sim | sustained stream | 11.1 / 12 (22) | update 0.45–0.54 / 0.6–1.2; busy 0.7–1.6 / 1.9–2.3 | update 0.52–1.08 / 1.18–1.64 (2.0); busy 0.74–1.34 / 1.7–2.5; 0 long frames |
  | iOS sim | Claude-like TUI | — | — | update 0.69–1.57 / 1.2–2.4 (3.0); busy 0.7–2.1 / 1.3–5.3 |
  | Android emu | fling | 9.7 / 20–24 (drag) | UI 0.75 / 4.4 / 7.0 (25) | UI 0.47 / 1.46 / 1.86 (4.9); record 0.15 / 0.37 / 0.43 |
  | Android emu | sustained stream | 11.4 / 22–35 (81) | UI 0.66 / 2.9 / 7.0 (12) | UI 0.49 / 1.08 / 1.68 (7.9); record 0.25 / 0.51 / 0.70 |
  | Android emu | Claude-like TUI | — | — | UI 0.50 / 0.62 / 0.78 (1.4); record 0.26 / 0.32 / 0.37 |
  | Android emu | idle 5 s | — | 5 frames | 5 frames |

  The terminal's history was at the 10k limit during the iOS stream runs (WU3 measured
  1.0–1.2 ms p50 for `update` in that state). iOS windows that contained a `simctl`
  screenshot or a running Maestro flow show 20–30 ms passes; they are excluded. Android render
  thread time is software rasterisation (31 / 42 / 46 ms in the fling), so janky-frame
  percentages stay meaningless on this emulator.
- **Release paths (nothing uploaded).** iOS Release for `generic/platform=iOS` with
  `CODE_SIGNING_ALLOWED=NO` builds (47 MB app with the fonts and `main.jsbundle`; signing
  config untouched). Android `bundleRelease` builds (66 MB AAB, arm64-v8a + x86_64, JNI
  exports present in the release `.so`). Outputs deleted. After the Release build
  `Pods/React-Core-prebuilt` holds the Release core again, which is the state the next Debug
  build expects (see the WU1 gotcha).
- **Not verified here.** Pinch (no two-finger input on either), double tap on Android in this
  pass (verified by WU6), the MISSING-terminal state, hitches and jank on real hardware,
  VoiceOver/TalkBack by ear.
- **Divergences left on purpose or logged.** Cell width differs per platform font metrics
  (iOS 69 columns, Android 53 at 10.5 on these screens). Bigger gaps → backlog 04: resize
  ownership with two clients (a non-owner's resize is denied and the grid stops fitting until
  the user types; once the Android view looked blank apart from the cursor), split emoji
  clusters (skin tone, flags, ZWJ), single-line curly/dotted/dashed underlines, unfocused
  cursor style, different wheel/auto-scroll constants, SGR 8 lost after restore.
- **Tooling notes.** Maestro `swipe` does not trigger the long press on Android; `adb shell
  input motionevent DOWN/MOVE/UP` does. Maestro's iOS `takeScreenshot` once failed with
  `NSPOSIXErrorDomain 64` and left the app in the background; `simctl io screenshot` is more
  reliable. Starting a swipe at the left edge on Android triggers the system back gesture.

## Rollout checklist

Needs a real device before the TestFlight / Play internal build goes to users (WU7 did not
upload anything):

- [ ] iPhone with ProMotion: Instruments Animation Hitches during a 5k-line fling, target 0
      hitches over 5 ms; main thread under 4 ms per frame while Claude Code streams.
- [ ] Mid-range Android: `dumpsys gfxinfo` during a 5k-line fling, under 1 % janky frames;
      UI thread under 8 ms p90 while Claude Code streams (hardware GPU, not SwANGLE).
- [ ] Pinch to zoom on both (live scale, snaps to 0.5 pt, persists).
- [ ] Double tap selects and copies a word on Android with a real finger.
- [ ] VoiceOver and TalkBack by ear: rows read, no focus traps.
- [ ] Claude Code session end to end on both: scroll during streaming, select a code block,
      paste it elsewhere.
- [ ] Two clients on one daemon (desktop + phone): change the phone's font or rotate while
      the desktop owns the resize; the grid must recover after typing (backlog 04).
- [ ] Font fallback on device: no tofu with the glyph test script (system font cascades on
      device can differ from the simulator).
- [ ] Upgrade path from a build with a stored `skia` setting lands on the native renderer.
- [ ] Then: TestFlight (team 3T3Q9PBUAH, `cz.mangoweb.okena`) and Play internal track.
