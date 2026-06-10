# Okena mobile — React Native

The **React Native** mobile client: the UI layer over the shared Rust core
(`crates/okena-mobile-ffi`, exposed to TypeScript via uniffi/ubrn), with a native
`react-native-skia` terminal renderer (**no `xterm.js`**). This replaces the retired Flutter
app; the migration plan is [`../RN_MIGRATION.md`](../RN_MIGRATION.md) and the architecture
overview is [`../../docs/mobile-status.md`](../../docs/mobile-status.md).

## What's here

A complete RN 0.76 project **minus the native host directories** (`android/`, `ios/`), which
are machine-generated (see step 1 below). What is in the repo:

- **JS host config** — `index.js`, `app.json`, `metro.config.js`, `babel.config.js`,
  `react-native.config.js`, `tsconfig.json`, `jest.config.js`, `.eslintrc.js`, `.prettierrc`.
- **The native↔TS binding contract** — `src/native/okena.ts`: the `OkenaNative` interface
  (the ~60 functions exported from `crates/okena-mobile-ffi/src/lib.rs`) + all record/enum
  types, plus `getOkenaNative()` which resolves the ubrn-generated module from `src/generated`.
- **The packed-cell decoder** — `src/native/cells.ts`: reads the little-endian cell buffer
  that `get_visible_cells_packed` produces (the render hot path).
- **App** — screens (`ServerList`, `Pairing`, `Workspace`), zustand stores (dependency-
  injected, so testable with a mock `OkenaNative`), `TerminalView` (Skia 3-pass paint),
  `KeyToolbar`, `LayoutRenderer`, `ProjectDrawer`, theme, and the JetBrainsMono fonts
  (`assets/`).

### Verified vs. not verified

Verified in CI / on any machine (no mobile toolchain needed):

```bash
cd mobile/rn
npm ci
npm run typecheck   # tsc --noEmit, strict
npm run lint        # eslint
npm test            # jest (packed-cell decoder smoke test)
```

**Not** verified here (needs the mobile toolchain + a device/emulator): the ubrn cross-compile,
the Skia native binaries, and an on-device run. Those are the steps below.

> Package manager: **npm** (the lockfile is `package-lock.json`). RN 0.76 native autolinking,
> CocoaPods, and ubrn are validated against npm/yarn — don't swap in a different manager here.

---

## Device-side setup (run on a machine with the RN toolchain)

Prereqs: Node ≥ 18, Watchman, JDK 17, Android SDK + NDK + `cargo-ndk` (Android), Xcode +
CocoaPods (iOS), and the Rust mobile targets:

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo install cargo-ndk
```

### 1. Generate the native host projects (`android/`, `ios/`)

`@shopify/react-native-skia` and the ubrn TurboModule are Fabric/TurboModules, so a **bare**
RN app (new architecture ON — the RN 0.76 default) is required; Expo Go won't work.

```bash
# from a temp dir: generate a host with the SAME app name as app.json ("OkenaMobile")
npx @react-native-community/cli@latest init OkenaMobile --version 0.76.5
# copy ONLY the generated native dirs into this project:
cp -R OkenaMobile/android OkenaMobile/ios ./
```

The JS/config files in this repo (`index.js`, `app.json`, `metro.config.js`, …) already match
what the template produces, so you only need its `android/` and `ios/` directories (both are
gitignored here). Confirm new-arch is on: `android/gradle.properties → newArchEnabled=true`.

### 2. Install JS deps + link fonts

```bash
npm ci
npx react-native-asset           # links assets/JetBrainsMono-*.ttf (react-native.config.js)
```

The Skia renderer additionally loads the same ttf via `useFont(require('../../assets/...'))`
in `WorkspaceScreen.tsx`, so the fonts are both linked (for `<Text fontFamily>`) and bundled.

### 3. Generate the Rust↔TS bindings with ubrn

`uniffi-bindgen-react-native` (`ubrn`) cross-compiles `crates/okena-mobile-ffi` and emits the
JSI TurboModule + TypeScript into `src/generated` (gitignored). Config: `ubrn.config.yaml`.

```bash
npm run ubrn:android     # ubrn build android --config ubrn.config.yaml --and-generate --release
npm run ubrn:ios         # ubrn build ios     --config ubrn.config.yaml --and-generate --release
( cd ios && pod install ) # pick up the generated xcframework
```

`getOkenaNative()` (`src/native/okena.ts`) already `require`s `../generated`, so once this
runs the app is wired — no code edit needed.

> **Version pairing:** ubrn and uniffi minor versions must match. This repo pins
> `uniffi-bindgen-react-native@0.31.0-3` (devDependency) ↔ `uniffi = "0.31"` in
> `crates/okena-mobile-ffi/Cargo.toml`. If `ubrn` reports a metadata/contract-version
> mismatch, bump both together.

> **NDK / TLS:** ensure `$HOME/.cargo/bin` is on the PATH the Gradle daemon sees. The crate
> already selects `rustls-tls` (via `okena-core`'s `client` feature), so no OpenSSL is
> cross-compiled for the NDK.

### 4. Run

```bash
npm run android     # device/emulator
npm run ios         # simulator
```

### 5. Phase-0 spikes (validate the two unknowns — see `../RN_MIGRATION.md` §3)

- **S1 (toolchain):** confirm `initApp()` + `connect()` + `connectionStatus()` work end-to-end
  through the ubrn module on a real Android device *and* iOS sim.
- **S2 (rendering):** confirm `react-native-skia` sustains the cell-grid paint at 60fps.

---

## File map

```
mobile/rn/
├── index.js · app.json                # RN entry + app name
├── metro.config.js · babel.config.js  # bundler + transpiler
├── react-native.config.js             # font asset linking
├── ubrn.config.yaml                   # ubrn: crate path, targets, output dirs
├── jest.config.js · __tests__/        # jest (cells decoder smoke test)
├── .eslintrc.js · .prettierrc         # lint + format
├── tsconfig.json · package.json
├── assets/JetBrainsMono-*.ttf         # bundled monospace fonts
└── src/
    ├── App.tsx · theme.ts
    ├── native/
    │   ├── okena.ts                    # OkenaNative contract + getOkenaNative()
    │   └── cells.ts                    # packed cell-buffer decoder
    ├── state/                          # zustand stores (DI), persistence, navigation
    ├── screens/                        # ServerList, Pairing, Workspace
    ├── components/                     # TerminalView (Skia), KeyToolbar, LayoutRenderer, …
    └── models/                         # SavedServer, LayoutNode
```
