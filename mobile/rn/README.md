# Okena mobile — React Native UI (scaffold)

This directory is the **React Native** side of the Flutter→RN migration described in
[`../RN_MIGRATION.md`](../RN_MIGRATION.md). It contains the two technically-meaty,
high-value pieces of the RN layer plus minimal project config:

1. **The native↔TS binding contract** — `src/native/okena.ts`. The full `OkenaNative`
   interface mirroring the ~60 Rust FFI functions in `mobile/native/src/api/{connection,terminal,state}.rs`,
   with all the record/enum types.
2. **The packed-cell decoder** — `src/native/cells.ts`. Reads the little-endian binary
   cell buffer that the render hot path consumes.
3. **The native terminal renderer** — `src/components/TerminalView.tsx`. A
   `@shopify/react-native-skia` port of `mobile/lib/src/widgets/terminal_painter.dart`
   (3-pass paint) + the sizing/poll loop from `terminal_view.dart`. **No `xterm.js`.**
4. **Theme** — `src/theme.ts`, ported from `mobile/lib/src/theme/app_theme.dart`.

## ⚠️ This is an UN-BUILT scaffold

It was authored on a **headless Linux box with no Android/iOS SDKs**, so it has **not**
been compiled or run on a device. What IS verified: the TypeScript type-checks against
the public APIs of `react-native@0.76` and `@shopify/react-native-skia@^1.5` (run
`npm install && npm run typecheck` once you have network). What is NOT verified: anything
that needs the native toolchain (the `ubrn`-generated module, Skia native binaries, an
emulator/device).

The contract files (`src/native/*`) are **specs both sides agree on**. The real native
module is *generated* — `src/native/okena.ts`'s `getOkenaNative()` throws until you wire
the generated package in (see step 4 below). `TerminalView` takes the native module via a
prop, so it is testable against a mock implementing `OkenaNative` without the real binding.

### One function does not exist Rust-side yet

`getVisibleCellsPacked(connId, terminalId): ArrayBuffer` is **being added** to the Rust
crate (`crates/okena-mobile-ffi`, migration Phase 1). The exact byte layout is the
contract documented in `src/native/cells.ts`. Until it lands, the renderer's per-frame
`getVisibleCellsPacked` call will fail — fall back to `getVisibleCells` (records) or mock it.

---

## Local build steps (run these on a machine with the RN toolchain)

Prereqs: Node ≥ 18, Watchman, JDK 17, Android SDK + NDK (for Android), Xcode + CocoaPods
(for iOS), and the Rust toolchain with the mobile targets (`aarch64-linux-android`,
`aarch64-apple-ios`, `aarch64-apple-ios-sim`).

### 1. Scaffold a bare RN host app (new architecture ON)

`@shopify/react-native-skia` and the `ubrn` native module are TurboModules/Fabric, so a
**bare** RN app (or Expo with a dev-client/prebuild — *not* Expo Go) is required.

```bash
# from mobile/rn/
npx @react-native-community/cli@latest init OkenaMobile --version 0.76.5
# RN 0.76 enables the new architecture by default. Confirm:
#   android/gradle.properties → newArchEnabled=true
#   ios: RCT_NEW_ARCH_ENABLED=1 (set by pod install on 0.76)
```

Then move/symlink the `src/` of this scaffold into the new app (or set the app's
`tsconfig`/Metro to resolve this package). The simplest path is to copy `src/`,
`tsconfig.json`, and the deps from `package.json` into the generated app.

### 2. Install dependencies

```bash
npm install @shopify/react-native-skia@^1.5.0
npm install            # react, react-native already pinned by the init template
# iOS native pods:
( cd ios && pod install )
```

### 3. Bundle the JetBrainsMono fonts

The fonts already live at `../fonts/JetBrainsMono-{Regular,Bold,Italic,BoldItalic}.ttf`.
Load them in JS with Skia's `useFont` (preferred — keeps them out of native asset configs):

```ts
import { useFont } from '@shopify/react-native-skia';
import { TerminalTheme } from './src/theme';

const regular = useFont(require('../fonts/JetBrainsMono-Regular.ttf'), TerminalTheme.defaultFontSize);
const bold = useFont(require('../fonts/JetBrainsMono-Bold.ttf'), TerminalTheme.defaultFontSize);
const italic = useFont(require('../fonts/JetBrainsMono-Italic.ttf'), TerminalTheme.defaultFontSize);
const boldItalic = useFont(require('../fonts/JetBrainsMono-BoldItalic.ttf'), TerminalTheme.defaultFontSize);
// render once all four are non-null:
// <TerminalView native={Okena} connId={…} terminalId={…} fonts={{ regular, bold, italic, boldItalic }} />
```

(`TerminalView` re-sizes the fonts in place to its `fontSize` prop, so passing them at any
base size is fine.) The chrome UI's `.SF Pro` maps to RN's `System` font on iOS; Android
falls back to Roboto — see `src/theme.ts`.

### 4. Generate the Rust↔TS bindings with `ubrn`

The native module is generated from the sibling Rust crate by
[`uniffi-bindgen-react-native`](https://github.com/jhugman/uniffi-bindgen-react-native) (`ubrn`).

```bash
# Install the generator (verify the current version during the Phase-0 spike):
npm install --save-dev uniffi-bindgen-react-native

# A ubrn.config.yaml at the app root points at the Rust crate, e.g.:
#   name: okena-mobile-ffi
#   rust:
#     directory: ../../../crates/okena-mobile-ffi     # the uniffi-annotated crate
#     manifestPath: Cargo.toml
#   bindings:
#     cppModuleName: okena_mobile_ffi
#     outputDir: ./modules/okena-mobile-ffi
#   android: { ... }   # NDK targets + jniLibs wiring
#   ios:     { ... }   # xcframework wiring

# Cross-compile + generate the TS/C++/JNI glue:
npx ubrn build android --config ubrn.config.yaml --and-generate
npx ubrn build ios     --config ubrn.config.yaml --and-generate
( cd ios && pod install )    # re-run pods to pick up the generated xcframework
```

`ubrn` emits an installable JS package whose exported functions match `OkenaNative`. Wire
it into `src/native/okena.ts`:

```ts
// src/native/okena.ts — replace the throwing getOkenaNative() body:
import * as gen from 'okena-mobile-ffi';            // ← the ubrn output package
export function getOkenaNative(): OkenaNative {
  return gen as unknown as OkenaNative;             // generated names already camelCase
}
```

> The `crates/okena-mobile-ffi` crate is owned by a separate agent (Phase 1 of the plan):
> it strips the `flutter_rust_bridge` attributes, adds `#[uniffi::export]`, ports all ~60
> functions, and **adds `get_visible_cells_packed`**. Do not edit `mobile/native` for this.

### 5. Run

```bash
npx react-native run-android      # device/emulator with the app installed
npx react-native run-ios          # simulator
```

The first run does the Rust cross-compile via `ubrn`'s Gradle/CocoaPods integration (the
RN equivalent of what `cargokit` does for Flutter today). Ensure `$HOME/.cargo/bin` is on
the PATH the Gradle daemon sees, and use `rustls-tls` (not `native-tls`) Rust-side to avoid
cross-compiling OpenSSL for the NDK — same constraints as the Flutter build (`../CLAUDE.md`).

---

## Verifying just the TS (no device needed)

```bash
cd mobile/rn
npm install          # needs network for react / react-native / skia type packages
npm run typecheck    # tsc --noEmit, strict
```

## File map

```
mobile/rn/
├── README.md                      # this file
├── package.json                   # RN 0.76, react 18.3, skia ^1.5, typescript
├── tsconfig.json                  # strict, react-jsx, bundler resolution
└── src/
    ├── theme.ts                   # colors + typography (← app_theme.dart)
    ├── native/
    │   ├── okena.ts               # OkenaNative contract (← api/*.rs), getOkenaNative() shim
    │   └── cells.ts               # packed cell-buffer decoder + flag constants + ARGB helpers
    └── components/
        └── TerminalView.tsx       # Skia 3-pass renderer (← terminal_painter.dart + terminal_view.dart)
```
