# Okena mobile

React Native terminal client over `crates/okena-mobile-ffi`, with a native Skia renderer.
See [architecture](../../docs/reference/mobile.md).

## Android toolchain

- Node 24, npm (`package-lock.json`), JDK 17.
- Rust from the repository's `rust-toolchain.toml`; `cargo-ndk` 4.1.2.
- Android SDK 36, Build Tools 36.0.0, NDK 28.2.13676358, CMake 3.22.1.
- React Native 0.86.3 / React 19.2.3 / Skia 2.14.0.
- UniFFI 0.31 and ubrn 0.31.0-3 must stay paired.

The Android host is versioned. Do not run `react-native init` over it. The native module
is generated inside `modules/okena-mobile-ffi`; only its package.json is versioned.
The npm scripts run ubrn from that package, then fix its Node package resolution and
missing Android manifest. No manual post-generation edits are needed.

```bash
# From mobile/rn; set ANDROID_HOME to your installed SDK.
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/28.2.13676358"
sdkmanager 'platforms;android-36' 'build-tools;36.0.0' 'ndk;28.2.13676358' 'cmake;3.22.1'
rustup target add aarch64-linux-android x86_64-linux-android
cpu-lease run -n 2 -- cargo install cargo-ndk --version 4.1.2 --locked
npm ci
cpu-lease run -n 4 -- env CARGO_BUILD_JOBS=4 npm run ubrn:android
cpu-lease run -n 2 -- npm run typecheck
cpu-lease run -n 2 -- npm run lint
cpu-lease run -n 2 -- npm test -- --runInBand
```

Android builds include arm64-v8a and x86_64. Regenerate bindings after changing the Rust
FFI, before building the app. The host bundles fonts directly from `assets/` through a
Gradle copy task; no `react-native-asset` command is needed.

```bash
# Unsigned release validation; includes the JS bundle and needs no Metro server.
cd android
cpu-lease run -n 4 -- ./gradlew --no-daemon --max-workers=4 :app:bundleRelease :app:assembleRelease :app:lintRelease
cd ..
python3 scripts/check-native-alignment.py android/app/build/outputs/bundle/release/app-release.aab

# Device development: run Metro separately with npm start.
cpu-lease run -n 4 -- npm run android
```

Never upload an unsigned validation artifact. Release builds do not fall back to a debug
key. Local builds are for validation; publish through CI.

## Google Play internal beta

Application ID: `com.contember.okena` by default. Display name: Okena. Version name: 0.1.0.
The application ID is permanent after the first Play upload. The manGoweb Play internal
beta uses `cz.mangoweb.okena` (matching its TestFlight build); pass it as the workflow's
`application_id` input, or locally as `OKENA_APPLICATION_ID`.

The **Android beta** workflow (`.github/workflows/android.yml`) builds unsigned validation
artifacts on relevant pushes/PRs. Merge the workflow into the default branch before its
manual dispatch is available in GitHub Actions. For a signed release, run it manually on the branch to
release, enable `signed`, and supply a `version_code` greater than every previous Play
upload. Download the artifact named `okena-android-signed-<application_id>-<version_code>`.
CI cross-compiles Rust, generates the native module, runs TypeScript/lint/Jest, builds the
AAB/APK, runs Android lint, verifies 16 KB ELF/APK alignment, and checks the AAB signature.

Configure these repository secrets before requesting a signed build:

| Secret | Value |
| --- | --- |
| `ANDROID_UPLOAD_KEYSTORE_BASE64` | Base64-encoded upload keystore |
| `ANDROID_UPLOAD_STORE_PASSWORD` | Keystore password |
| `ANDROID_UPLOAD_KEY_ALIAS` | Upload key alias |
| `ANDROID_UPLOAD_KEY_PASSWORD` | Key password |

Keep a separate secure backup of the keystore and passwords. GitHub secrets cannot be
read back. The workflow removes the decoded key after each run.

Gradle reads the corresponding `OKENA_UPLOAD_STORE_FILE`, `OKENA_UPLOAD_STORE_PASSWORD`,
`OKENA_UPLOAD_KEY_ALIAS`, and `OKENA_UPLOAD_KEY_PASSWORD` environment variables. Set all
four or none. `OKENA_VERSION_CODE` defaults to 1 for local validation.

In Play Console:

1. Create/select the app for this application ID and enable Play App Signing.
2. Create an Internal testing release using the signed CI AAB.
3. Add tester Google accounts or a Google Group, and a feedback contact.
4. Roll out the internal release and share the opt-in link with those testers.
5. Verify installation and an update through Play before expanding the group.

Internal testing supports up to 100 testers and can start before completing the store
listing. Apps distributed exclusively on this track are exempt from the Data safety form.
See [Google's testing documentation](https://support.google.com/googleplay/android-developer/answer/9845334?hl=en).
The workflow produces artifacts; it does not create a Play listing or roll out a release.

### Android release acceptance on a device

- Install the Play-delivered release without Metro or ADB port forwarding.
- Pair over LAN/VPN with TLS, restart, and reconnect using the saved certificate pin.
- Verify terminal output, typing, Ctrl-C, arrows, clipboard and Compose.
- Check keyboard, system bars, landscape, and navigation on Android 16.
- Background/foreground the app; interrupt and restore network connectivity.
- Install the next Play version and confirm saved servers survive.
- Run on a 16 KB Android image/device; binary alignment alone does not prove runtime behavior.

### Tester setup

Okena mobile controls an existing Okena server; it does not provide a shell on the phone.
Run the desktop/server on a host reachable from the phone over LAN or VPN. Add its host
and port (normally 19100), keep TLS enabled, and enter the pairing code from `okena pair`.
The code expires after 60 seconds. For an emulator, `10.0.2.2` addresses the development host.

## iOS toolchain

- macOS with Xcode 26; Node 24, npm.
- Ruby 3.4 and Bundler; `Gemfile.lock` pins CocoaPods 1.17.0.
- Rust targets `aarch64-apple-ios` (device) and `aarch64-apple-ios-sim` (simulator).

The iOS host in `ios/` is versioned. Do not regenerate it from the React Native template.
The Xcode project bundles the fonts directly from `assets/` and lists them in `UIAppFonts`.
iOS resolves the family name `JetBrains Mono`; Android resolves the file name `JetBrainsMono`.

```bash
# From mobile/rn
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
npm ci
bundle install
npm run ubrn:ios
(cd ios && bundle exec pod install)

# Device development: run Metro separately with npm start.
npm run ios
```

`ios/Podfile.lock` is versioned. CI runs `pod install --deployment` and fails when the lock
is stale; commit the updated lock after changing native dependencies or the FFI podspec.
To run on a physical device, select your team in Xcode. Do not commit `DEVELOPMENT_TEAM`.

## TestFlight internal beta

Bundle ID: `com.contember.okena`. Display name: Okena. Version: 0.1.0.
The bundle ID is permanent after the first App Store Connect upload.

The **iOS beta** workflow (`.github/workflows/ios.yml`) builds an unsigned release archive
on relevant pushes/PRs. It cross-compiles Rust for `aarch64-apple-ios`, generates the native
module, installs pods, archives, and checks the bundle ID, version, JS bundle, fonts, and
privacy manifest in the archive.

For a TestFlight build, run the workflow manually on the branch to release, enable `upload`,
and supply a `build_number` greater than every previous upload of this version. The workflow
then signs with the team's cloud-managed distribution certificate and uploads to App Store
Connect. Merge the workflow into the default branch before its manual dispatch is available.

Configure these repository secrets before requesting an upload:

| Secret | Value |
| --- | --- |
| `APPLE_TEAM_ID` | 10-character Team ID (Apple Developer → Membership details) |
| `APP_STORE_CONNECT_API_KEY_ID` | Key ID of the App Store Connect API key |
| `APP_STORE_CONNECT_API_ISSUER_ID` | Issuer ID shown above the API key list |
| `APP_STORE_CONNECT_API_KEY_P8` | Full text of the downloaded `AuthKey_<key id>.p8` |

The API key must be a **Team key with the Admin role**. Cloud-managed distribution
certificates are not available to keys with lower roles. Apple allows only one download of
the `.p8` file; keep a separate secure backup. The workflow removes the key after each run.

One-time setup:

1. In Certificates, Identifiers & Profiles, register the explicit App ID `com.contember.okena`.
2. In App Store Connect, create the iOS app with that bundle ID and an unused SKU.
3. In Users and Access → Integrations → App Store Connect API, create the Team key (Admin).
4. Add the four repository secrets.
5. Run the workflow with `upload`. Processing in App Store Connect takes several minutes.
6. In TestFlight, add the build to an internal testing group. Internal testers must be
   App Store Connect users (up to 100). External testers require Beta App Review.

`Info.plist` sets `ITSAppUsesNonExemptEncryption` to `false`: the app uses only standard TLS.
App Store Connect then does not ask the export-compliance question for each build.

### iOS release acceptance on a device

- Install the TestFlight build without Metro.
- Accept the Local Network prompt; pair over LAN/VPN with TLS, restart, and reconnect.
- Verify terminal output, typing, Ctrl-C, arrows, clipboard and Compose.
- Check the keyboard, safe areas, and rotation on an iPhone and an iPad.
- Background/foreground the app; interrupt and restore network connectivity.
- Install the next TestFlight build and confirm saved servers survive.

## Beta verification (2026-10-12)

Verified locally from freshly generated Android bindings:

- TypeScript, ESLint and all 25 Jest tests.
- Release AAB/APK for arm64-v8a and x86_64; Android lint: 0 errors, 14 warnings.
- Bundletool validation, upload-key signatures, 16 KB alignment of all 26 native
  libraries, APK ZIP alignment, and `PAGE_ALIGNMENT_16K` in the AAB configuration.
- Signed release installation, APK update preserving saved servers, and cold start on the
  Android 16 / API 36 Google APIs
  16 KB x86_64 emulator, without Metro.
- TLS pairing to an isolated daemon, Skia terminal rendering, a shell command/output
  round trip, Compose Run, and saved-access reconnect after force-stopping the application.
- Portrait soft-keyboard layout: terminal key rail, Add Server fields/button, and Compose
  input/actions remain above the keyboard.

This is local validation. A GitHub Actions run, a Play-delivered install/update, and
physical-device acceptance of the upgraded app are still required. The emulator
landscape keyboard can consume almost all terminal space; use portrait for this beta
and validate landscape ergonomics on physical devices. Android lint warnings
include upstream template resources, backup-rule guidance, and launcher-icon conventions.

## Historical device verification

The notes below describe the previous RN 0.76 build, not acceptance of the upgraded beta.
The earlier build was exercised on a physical Android 13 phone. New native versions need
the release acceptance checks above.

### Terminal input controls

The terminal uses a hidden native `TextInput` for the system keyboard and keeps a terminal-specific
key rail visible below the canvas:

- Tap the terminal to focus the system keyboard. Autocorrect, capitalization, suggestions, and
  autofill are disabled; Android uses its code-friendly `visible-password` keyboard mode.
- `ctrl`, `alt`, and `meta`: tap for a one-shot modifier; hold for 350 ms to lock it. A short accent
  marker means one-shot, while a full accent fill and long marker mean locked. Locked modifiers
  survive subsequent input until tapped or held again.
- `^C` sends an immediate interrupt. `esc`, `tab`, common path/shell characters, clipboard paste,
  Compose, and keyboard hide are directly available in the scrollable rail.
- The fixed arrow pad sends on touch and repeats after holding for 360 ms. Modifiers are honored;
  Meta + arrows map to Home/End/Page Up/Page Down.
- `more` hides the system keyboard and opens the extended key deck: Ctrl-C/D/Z, Escape,
  Home/End/Page Up/Page Down/Delete, explicit arrows, and common raw characters.
- `paste` sends a single-line clipboard value directly. Multiline clipboard content opens in
  Compose first so pasted newlines cannot run several commands without review.
- `edit` in the primary row opens Compose: a native text field with autocorrect, capitalization,
  spellcheck, and dictation, for writing prompts. Compose offers **Insert** (send exactly the
  edited text) and **Run** (send text followed by Enter).
- Long-press and drag selects terminal text; release copies it to the OS clipboard and briefly
  shows a `Copied` confirmation. Double-tap copies the selected word.
- Vertical drag scrolls the pane under the finger: scrollback in a normal shell, or wheel events
  when the app enabled mouse reporting (vim, tmux `mouse on`, agent TUIs), like the desktop.
  Typing or a special key snaps back to the bottom. Tapping a pane makes it the keyboard and
  toolbar target. The mobile mirror only holds scrollback received since it attached.

### Input UX validation status (2026-07-17)

Verified statically:

- strict TypeScript check, ESLint, and 10 Jest tests,
- Android arm64 release bundle and native build, including clipboard autolinking,
- modifier state/encoding tests: one-shot reset, long-press lock persistence, Ctrl and Meta bytes.

Verified remotely on a physical OnePlus DN2103 running Android 13:

- install and restart with saved token, ADB-reversed loopback connection, stable Skia rendering,
  command input, and returned output,
- LAN TLS pairing and saved-token reconnect to the LAN host with no ADB reverse rule,
  including a terminal command/output round-trip,
- code-friendly soft keyboard behavior: lowercase input is preserved and the canvas, rail, and IME
  remain usable together,
- Ctrl tap for one-shot, automatic reset after input, long-press lock, persistence across input, and
  explicit unlock; the dedicated Ctrl-C key interrupts the shell,
- arrow tap and hold-to-repeat through shell history, including returning to the empty prompt,
- the complete extended key deck in portrait, with all signal, navigation, character, paste, and
  Compose controls visible,
- Compose **Insert** without Enter and **Run** with Enter,
- double-tap word copy, `Copied` feedback, and a single-line clipboard paste round-trip,
- the terminal rail with the keyboard both shown and hidden, plus the full rail in landscape.

Still worth checking with a person holding the device:

1. Alt, Meta, and modifier-plus-arrow combinations in real applications;
2. arrow repeat, Home/End/Page navigation, and Ctrl-D/Z in a TUI such as `vim`;
3. multiline clipboard review and long-press/drag selection across several cells;
4. one-handed reach, long-press timing, landscape keyboard layout, and iOS behavior.

---
