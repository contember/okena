---
name: ui-screenshot
description: Look at Okena's UI by taking a real screenshot of the running app, headlessly — nothing appears on the user's desktop and their live Okena instance is untouched. Use whenever a change is visual (layout, spacing, colors, a new widget, "make it prettier") and you need to see the result, or when a GPUI element renders nothing and you need to find out why. Triggers on "screenshot the app", "how does it look", "is it visible", "verify the UI", "the element doesn't render".
---

# Seeing Okena's UI

`cargo build` proves a view compiles. It does not prove anything is *visible* —
GPUI will happily lay an element out at the wrong coordinates and clip every
quad you paint. Take a screenshot.

## Run it

```bash
cargo build
.claude/skills/ui-screenshot/scripts/okena-shot.sh /tmp/shot.png
```

Then `Read` the PNG. Crop first when the interesting part is small — a full
1400x1000 frame downscales too far to judge a 6px scrollbar:

```bash
convert /tmp/shot.png -crop 800x640+300+170 +repage /tmp/crop.png     # region
convert /tmp/shot.png -crop 40x480+1060+240 +repage -resize 500% /tmp/edge.png  # zoom
convert /tmp/shot.png -crop 60x1+1930+800 +repage txt:                # exact pixel values
```

The pixel dump is the ground truth when "is it there?" is the question — eyes
lie about 1px borders and 6% alpha.

## How it works

A nested `gnome-shell --headless --unsafe-mode` runs inside `dbus-run-session`
on a virtual monitor, with Okena launched against an isolated
`XDG_CONFIG_HOME`. No root, no Xvfb (not installed), nothing on the user's
screen, and the user's own Okena keeps running.

Do **not** instead launch a debug build on the real display:

- It pops a window over whatever the user is doing.
- GNOME denies `org.gnome.Shell.Screenshot` to anything but its own UI, and
  `import`/`xwd` only see XWayland windows — Okena runs Wayland-native.
- A second instance is only safe because `reconcile_dtach_sessions` bails out
  when another daemon holds the runtime dir. Don't lean on that.

## Driving the UI

Pointer events go through a Clutter virtual device:

```bash
okena-shot.sh /tmp/shot.png click:353,318 scroll:900,400,5
```

Coordinates are **window coordinates**, which equal screenshot pixels only
once the overview is hidden (the script does this). If clicks land in the wrong
place, check `rect.log` next to the output for the window's real frame.

Clicks are flaky against GPUI. When a click just needs to reach a specific
screen, prefer a temporary env-var hook in the app over fighting input
injection — e.g. reading `OKENA_DEBUG_SETTINGS_TAB` where the initial category
is chosen, then passing `OKENA_SHOT_ENV="OKENA_DEBUG_SETTINGS_TAB=terminal"`.
Delete the hook before committing.

Two hooks are usually worth adding temporarily:

- open the overlay you want on startup (dispatch the action from
  `cx.open_window(...)`'s follow-up with a delay),
- select the tab/state inside it.

## Config for the isolated instance

The script seeds `<cfg>/okena/profiles/{default,uicheck}/settings.json`.
Write both: the client runs `--profile uicheck` but spawns its daemon **without**
the flag, so the daemon reads `default` and its settings snapshot overrides the
client's. Setting only one profile silently does nothing.

```bash
CFG=/tmp/shot-cfg
mkdir -p $CFG/okena/profiles/{default,uicheck}
for p in default uicheck; do
  printf '{"version":3,"theme_mode":"dark","enabled_extensions":["claude-code"]}' \
    > $CFG/okena/profiles/$p/settings.json
done
OKENA_SHOT_CFG=$CFG .claude/skills/ui-screenshot/scripts/okena-shot.sh /tmp/shot.png
```

## When an element paints nothing

Symptom: `prepaint`/`paint` clearly run (add an `eprintln!`, read `gui.log`),
yet no pixels appear.

1. **Paint the whole element bounds in `gpui::red()`.** If the red block is
   missing too, the problem is geometry or clipping, not your colors.
2. **Log `bounds` in `prepaint` next to a known-good reference** — for a
   scrollbar, `ScrollHandle::bounds()`. They should match.
3. **An absolutely positioned element needs explicit insets.** `Style` with
   `position: Absolute` and `size: relative(1.0)` but `inset: auto` is placed at
   taffy's *static position* — for a child that follows a sibling in the flex
   line, that is offset by the sibling's full extent, so the element lands
   entirely outside the visible area and every quad is clipped away:

   ```rust
   let style = Style {
       position: Position::Absolute,
       inset: Edges { top: px(0.).into(), right: px(0.).into(),
                      bottom: px(0.).into(), left: px(0.).into() },
       ..Default::default()
   };
   ```

4. Only then suspect color/alpha. Dump pixels to check.

## Reading a `ScrollHandle` in the same frame

A plain `div` reading `ScrollHandle::max_offset()` during `render` sees the
*previous* frame's geometry — so on the first frame it reads zero, decides
there is nothing to show, and nothing ever triggers a redraw. Implement it as a
raw `Element` and read the handle in `prepaint`: siblings prepaint in order, so
a scroll container placed before you has already published its geometry.
`crates/okena-ui/src/scrollbar.rs` is the worked example.
