<!--
On close, prepend an OUTCOME block here, then `git mv` this file to ../archive/:

> **OUTCOME — shipped YYYY-MM-DD.** <one-paragraph result.> Commit map: WU1 → <sha>,
> WU2 → <sha>, … Verification: <the gate command + numbers>. Backlog closed:
> <ids deleted/rescoped>. Deferred: <honest notes>.
-->

# Sprint — What's New (2026-10-20)

**Goal.** Let a user see what changed in Okena, in the app, without opening
GitHub: a "What's New" overlay that renders the published release notes, and a
one-time nudge after an update.

**Theme.** Releases land every week or two and each one carries a hand-written
`## What's new` / `## Fixes` body on GitHub (since v0.33.0). Nothing in the app
surfaces it. The updater already talks to the GitHub Releases API through the
daemon and the app already renders Markdown natively, so this is wiring, not
infrastructure: one more field over the wire, one overlay, one settings key,
one toast.

Success: `Help → What's New` (and the command palette) opens an overlay listing
releases newest-first with their notes rendered as Markdown; after an update
the next start shows a toast "Okena v0.35.0 — see what's new" whose button
opens the same overlay; a user who never updates sees nothing new.

## Refs re-verified at HEAD (2026-10-20)

- ✔ GitHub Release bodies are Markdown. v0.33.0 and v0.34.0 have hand-written
  `## What's new` / `## Fixes` sections; v0.32.0 and older are the
  auto-generated PR list (`generate_release_notes: true` at
  `.github/workflows/build.yml:428`). Both render fine; only the newer ones
  read well.
- ✔ The daemon fetches the release list once (`?per_page=100`) in
  `list_revert_releases_blocking` —
  `crates/okena-ext-updater/src/checker.rs:59-79`. `catalog_release`
  (`:159-186`) drops everything that is not a revert target: it keeps only
  releases `< current` *with a platform asset* and never reads `body`.
  `is_stable_release` (`:188-191`) filters drafts/prereleases.
- ✔ The desktop reaches that list via the local daemon:
  `daemon_client::fetch_releases` (`crates/okena-ext-updater/src/daemon_client.rs:90`)
  → route `/v1/update/releases`
  (`crates/okena-remote-server/src/routes/mod.rs:222`, handler
  `routes/update.rs:74-88`, local-trusted only). The CLI prints the same
  catalog (`crates/okena-cli/src/commands.rs:1882`, `:1940`). `RevertRelease`
  is a public wire type — do not change its shape.
- ✔ Markdown rendering: `MarkdownDocument::parse(&str)`
  (`crates/okena-markdown/src/parser.rs:93`), per-node
  `render_node(idx, &theme, cx, selection)` and `highlight_code_blocks(is_dark)`
  (`crates/okena-markdown/src/lib.rs:120`, `render.rs:195`). The file viewer is
  the reference consumer — `crates/okena-files/src/file_viewer/render.rs:2245`.
  The app re-exports it at `crates/okena-app/src/views/overlays/markdown_renderer/mod.rs`.
- ✔ Modal overlays: `OverlayManager::toggle_keybindings_help`
  (`crates/okena-app/src/views/overlay_manager.rs:692-711`) is the pattern
  (entity + `Close` event + `open_modal`); `AboutModal` uses the
  `toggle_overlay!` macro (`:675`). Modal chrome comes from
  `views::components::{modal_backdrop, modal_content}` (see `about.rs:4`).
- ✔ A global action is registered in four places: the `actions!` list
  (`crates/okena-app/src/keybindings/mod.rs:18-28`), the keystroke parser
  (`mod.rs:450`), the command-palette description map
  (`keybindings/descriptions.rs:417-423`, category "Global"), and the window
  handler (`crates/okena-app/src/views/window/render.rs:1198`). Default
  bindings live in `keybindings/config.rs:76`.
- ✔ App menus are static tables in `crates/okena-app/src/app_menu.rs:70-103`
  (`MACOS_APP_MENU`, `MACOS_VIEW_MENU`, `TITLE_BAR_MENU`); `AppMenuAction` maps
  label → action name → boxed action (`:9-50`).
- ✔ The About overlay already shows the running version and the update card
  (`crates/okena-app/src/views/overlays/about.rs:324-419`); it reads
  `GlobalUpdateInfo` for `app_version()`.
- ✔ Toasts with buttons: `Toast::info(..).with_actions(vec![ToastAction::new(id, label, style)])`
  (`crates/okena-state/src/toast.rs:56-63, 113, 132`); the desktop handles a
  click as `ToastActionEvent` (`crates/okena-app/src/views/panels/toast.rs:15`,
  wired in `views/window/handlers.rs` around `:1113`). `soft_close.rs:110` is
  a working example.
- ✔ Settings: flat bools on `AppSettings` with `#[serde(default = ..)]`
  (`crates/okena-workspace/src/settings.rs:267+`, e.g. `show_focused_border`
  `:288`), setters in `crates/okena-app-core/src/settings.rs`, toggle rows in
  `crates/okena-app/src/views/overlays/settings_panel/render_general.rs:19`
  (`render_toggle(id, label, value, default)`).
- ⚠ The profile already records the running version
  (`profiles::record_app_version`, `src/main.rs:348`) but overwrites it before
  the UI starts, so it cannot tell "seen" from "started". A separate key is
  needed.
- ⚠ Local builds (`detect_local_checkout`, `GlobalLocalBuild`) carry the
  Cargo.toml version, which equals the latest tag until the next bump. Such a
  build has no newer notes to show; the overlay must still open and show the
  latest published notes rather than an empty list.

## Work units

### WU1 — Release notes over the wire (effort S)

- **Problem.** The daemon already downloads the release list but the body is
  thrown away and the catalog is filtered to revert targets
  (`checker.rs:159-186`). What's New needs releases *up to and including* the
  running version, with their notes, regardless of platform assets.
- **Verify first.**
  `curl -s 'https://api.github.com/repos/contember/okena/releases?per_page=3' | jq '.[] | {tag_name, published_at, html_url, body}'`
  — confirm the field names; `grep -rn "ReleaseCatalog\|RevertRelease" crates --include='*.rs'`
  — the consumers that must stay untouched.
- **Scope.**
  1. `checker.rs`: new public types
     `ReleaseNote { version, published_at, release_url, body }` and
     `ReleaseNotes { current_version, releases: Vec<ReleaseNote> }`; new
     `list_release_notes(app_version, since: Option<String>) -> ReleaseNotes`
     reusing `fetch_json` + `is_stable_release`. Rules: stable only; keep
     `since < version <= current`; when `since` is `None` keep everything
     `<= current`; cap at the newest 20; newest first (GitHub order). If no
     release is `<= current` (local build ahead of the last tag), fall back to
     the newest published release so the list is never empty. Pure helper
     `select_release_notes(releases, current, since)` so the rules are unit
     testable without the network.
  2. Daemon route `GET /v1/update/release-notes?since=<version>` next to
     `/v1/update/releases` (`routes/mod.rs:222`, `routes/update.rs:74`),
     local-trusted only like its sibling.
  3. `daemon_client::fetch_release_notes(since: Option<&str>)`.
  4. Re-export the two types from `okena-ext-updater/src/lib.rs`.
- **Acceptance / witness.** Unit tests on `select_release_notes` with a JSON
  fixture of five releases (one draft, one prerelease, versions
  0.32–0.35): `since=0.33, current=0.34` → `[0.34]`; `since=None, current=0.34`
  → `[0.34, 0.33, 0.32]`; `since=None, current=0.36` (bumped, not yet
  released) → `[0.35, 0.34, 0.33, 0.32]`; `since=None, current=0.31` (nothing
  `<= current`) → fallback `[0.32]`; drafts/prereleases never appear;
  `body: null` becomes an empty string; the cap keeps the newest 20. `cargo test -p okena-ext-updater`. Manual: `curl` the daemon route
  with the local token and see Markdown in `body`.
- **Touch points.** `crates/okena-ext-updater/src/{checker.rs,daemon_client.rs,lib.rs}`,
  `crates/okena-remote-server/src/routes/{mod.rs,update.rs}`.

### WU2 — What's New overlay (effort M)

- **Problem.** No surface renders release notes.
- **Verify first.** Read `about.rs:324-419` for modal sizing/chrome and
  `file_viewer/render.rs:2245` for how `MarkdownDocument` nodes are laid out
  and themed; confirm `okena-markdown` handles link clicks (the v0.33 body has
  a `[setup guide](...)` link) — if not, links render as text and that is
  acceptable for this sprint.
- **Scope.**
  1. `crates/okena-app/src/views/overlays/whats_new.rs`: `WhatsNewModal`
     entity with states `Loading` / `Loaded(ReleaseNotes)` / `Failed(String)`.
     On open it spawns `daemon_client::fetch_release_notes(None)` via
     `cx.background_spawn` / `smol::unblock` (same pattern as the updater
     settings view's catalog load, `okena-ext-updater/src/settings.rs:17`).
     Renders: title "What's New", the running version, then one section per
     release — `v0.34.0 · 9 Oct 2026` header, "Open on GitHub" link
     (`okena_core::process::open_url`), the body as `MarkdownDocument`
     (parsed once per release, `highlight_code_blocks` on theme flip). A
     scrollable column; width like the About modal, height ~70% of the window.
     Escape / backdrop click / × closes (`Close` event, same as
     `KeybindingsHelpEvent::Close`). Failure state shows the error and a
     "Retry" button; empty list shows "No release notes yet".
  2. `OverlayManager::toggle_whats_new` modelled on
     `toggle_keybindings_help` (`overlay_manager.rs:692`).
  3. On open, record the running version as seen (WU3 setter) so the toast
     does not come back.
- **Acceptance / witness.** `#[gpui::test]` on the modal: `Loaded` with two
  releases renders two headers in version order; `Failed` renders the error
  and the retry button. Formatting helper `release_header(version, published_at)`
  is a pure unit test (`"2026-10-09T15:49:55Z"` → `"9 Oct 2026"`, garbage →
  version only). Manual: open from the palette, scroll, click the GitHub link,
  toggle dark/light and see code blocks re-highlight. `cargo test -p okena-app`.
- **Touch points.** `crates/okena-app/src/views/overlays/{whats_new.rs,mod.rs}`,
  `crates/okena-app/src/views/overlay_manager.rs`.

### WU3 — Entry points, seen marker, post-update toast (effort S)

- **Problem.** The overlay needs a way in, and the "you just updated" moment
  is the one time a user actually wants it.
- **Verify first.** `grep -n "ShowKeybindings" crates/okena-app/src -r` — the
  four registration sites; `grep -n "ToastActionEvent" crates/okena-app/src -r`
  — how a toast button reaches the window.
- **Scope.**
  1. Action `ShowWhatsNew` in `keybindings/mod.rs` (actions list + parser),
     `descriptions.rs` ("What's New", "Show release notes for recent Okena
     versions", category "Global"), handler in `window/render.rs` next to
     `ShowKeybindings`. No default keybinding.
  2. Menu: `AppMenuAction::WhatsNew` ("What's New") in `app_menu.rs`, placed
     after `Keybindings` in `MACOS_VIEW_MENU` and `TITLE_BAR_MENU`. A
     "What's new" link in the About modal next to the GitHub link
     (`about.rs:164-213` `render_link`), emitting a new
     `AboutModalEvent::ShowWhatsNew` the overlay manager turns into
     `toggle_whats_new`.
  3. Settings: `AppSettings.whats_new_seen_version: Option<String>`
     (`#[serde(default, skip_serializing_if = "Option::is_none")]`) and
     `AppSettings.show_whats_new_after_update: bool` (default `true`, toggle
     row "Show What's New after an update" in General). Setters in
     `okena-app-core/src/settings.rs`.
  4. Startup check, once per process after the daemon connection is up (look
     at how `update_checker.rs` schedules its first background check and hook
     in beside it): read `whats_new_seen_version`; `None` → write the running
     version, no toast (fresh install, or first run of this feature); seen `<`
     running and the toggle is on → `Toast::info("Okena v{running} — see what's
     new")` with one `Primary` action `whats-new:open`; the `ToastActionEvent`
     handler opens the overlay (which marks the version seen). Dismissing the
     toast without opening does **not** mark it seen; the toast returns on the
     next start until opened or the toggle is turned off. Pure helper
     `whats_new_nudge(seen: Option<&str>, running: &str, enabled: bool) -> Nudge { Toast, RecordOnly, Nothing }`.
- **Acceptance / witness.** Unit tests on `whats_new_nudge`: `(None, "0.35.0", true)`
  → `RecordOnly`; `("0.34.0", "0.35.0", true)` → `Toast`;
  `("0.35.0", "0.35.0", true)` → `Nothing`; `("0.36.0", "0.35.0", true)`
  (reverted) → `Nothing`; `("0.34.0", "0.35.0", false)` → `Nothing`;
  unparsable seen → `RecordOnly`. Settings round-trip test: a `settings.json`
  without the two keys loads with `None` / `true`. Manual: set
  `whats_new_seen_version` to `"0.33.0"` in `settings.json`, start the app,
  see the toast, click it, overlay opens, restart, no toast.
  `cargo test -p okena-workspace -p okena-app-core -p okena-app`.
- **Touch points.** `crates/okena-app/src/keybindings/{mod.rs,descriptions.rs}`,
  `crates/okena-app/src/app_menu.rs`, `crates/okena-app/src/views/window/{render.rs,handlers.rs}`,
  `crates/okena-app/src/views/overlays/about.rs`,
  `crates/okena-workspace/src/settings.rs`, `crates/okena-app-core/src/settings.rs`,
  `crates/okena-app/src/views/overlays/settings_panel/render_general.rs`,
  `crates/okena-ext-updater/src/update_checker.rs` (or wherever the startup
  hook lands).

## Out of scope (explicit)

- **Bundled / offline notes.** GitHub is the only source; the updater already
  depends on it and the overlay degrades to an error + retry. Revisit only if
  someone asks for offline.
- **Unreleased changes for local builds.** A build from `main` shows the latest
  published notes. Rendering `git log` since the last tag is a different
  feature (it would need the checkout, not the API).
- **Web and mobile clients.** Desktop only. The route is local-trusted, nothing
  new is exposed to remote peers.
- **Polishing old release bodies.** v0.32.0 and older stay the auto-generated
  PR list; the overlay renders them as-is.
- **Changing the release process.** Hand-written notes in the GitHub release
  body remain the contract; no CHANGELOG file is introduced.

## Decisions

- **Source = GitHub Releases body, fetched through the daemon.** Same API, same
  trust boundary and same rate-limit handling as the updater; the desktop
  never talks to GitHub directly.
- **New type, not a `body` field on `RevertRelease`.** `RevertRelease` is
  printed by the CLI and tied to revert semantics (platform asset, snapshot).
  Release notes have different filtering; mixing them would make both worse.
- **Nudge is a toast, not an auto-opened modal.** An update should never steal
  the screen from a running session. One primary button, returns until acted
  on, one toggle to silence it.
- **"Seen" is recorded on open, stored in `settings.json`.** The profile's
  app-version marker is overwritten at startup and serves config snapshots;
  reusing it would couple two unrelated features.
- **`None` seen version means "record, don't nudge".** The first run after
  this ships (and every fresh install) stays quiet; the first *following*
  update produces the toast.

## Sequencing

| Step | WUs | Notes |
|---|---|---|
| 1 | WU1 | wire + tests, no UI; can be reviewed alone |
| 2 | WU2 ‖ WU3 (settings + action + menu parts) | both build on WU1's types; the toast handler in WU3 needs WU2's `toggle_whats_new`, so finish WU2 before wiring the toast |
| 3 | WU3 (toast) | last, then the manual end-to-end check |

One commit per WU (`feat(whats-new): ...`). After WU3, update
`docs/reference/` if a user-facing doc lists menus/settings (check
`grep -rn "Show Keybindings" docs/reference`), then close this sprint per
`docs/CLAUDE.md`.

## Run log

<!-- Append as you work: discoveries, deviations, blockers. -->
