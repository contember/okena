# Mobile App — React Native (uniffi over the Rust core)

Remote terminal client for Android/iOS. RN UI over the shared Rust core
(`crates/okena-mobile-ffi`, exposed via uniffi/ubrn as a JSI TurboModule). Native terminal
rendering with `react-native-skia` — **no `xterm.js`**. Replaced the retired Flutter app.

Architecture overview: `../../docs/mobile-status.md`. Migration plan: `../RN_MIGRATION.md`.

## Commands

```bash
cd mobile/rn
npm ci
npm run typecheck   # tsc --noEmit, strict
npm run lint        # eslint
npm test            # jest
npm run format      # prettier (opt-in; NOT enforced by lint)
```

Device build (needs Android NDK / Xcode — see `README.md`): `npm run ubrn:android|ios` then
`npm run android|ios`. The native host dirs (`android/`, `ios/`) and ubrn output
(`src/generated/`) are generated and gitignored.

## Key boundaries

- **`src/native/okena.ts`** — the `OkenaNative` TS interface: the hand-maintained contract for
  the ~60 functions in `crates/okena-mobile-ffi/src/lib.rs`. Keep both sides in sync.
  `getOkenaNative()` `require`s the ubrn-generated module from `src/generated` (throws with a
  "run ubrn" message until generated).
- **`src/native/cells.ts`** — decoder for the packed cell buffer from `get_visible_cells_packed`
  (the render hot path). Its byte layout is the contract the Rust encoder must match; the jest
  smoke test (`__tests__/cells.test.ts`) guards it.
- The native module is **dependency-injected**, never imported globally: stores via
  `configureConnectionStore` / `configureWorkspaceStore`, `TerminalView`/`TerminalPane` via a
  `native` prop. This keeps everything testable with a mock and lets `tsc`/jest run with no
  native module present.

## Conventions

- **Package manager: npm** (`package-lock.json`). Don't switch — RN autolinking / CocoaPods /
  ubrn are validated against npm/yarn.
- **State: zustand** stores with polling (mirrors the old provider cadence): fast (500ms) while
  connecting, slow (1–2s) when connected.
- **ESLint** enforces correctness only; `prettier/prettier`, `no-bitwise` (cell/ARGB decoding),
  `no-void` (fire-and-forget), and `curly` are off by design (see `.eslintrc.js`).
- **uniffi ⇄ ubrn version pairing** must match: `uniffi = "0.31"` (crate) ↔
  `uniffi-bindgen-react-native@0.31.0-3` (devDep). Bump together.
