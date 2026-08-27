#!/usr/bin/env bash
#
# okena-shot.sh — screenshot Okena's UI without putting a window on the user's screen.
#
# Boots a nested `gnome-shell --headless` inside a private D-Bus session, launches
# Okena against an isolated config, and captures the virtual monitor. Nothing
# touches the running desktop session or the user's live Okena instance.
#
# Usage:
#   scripts/okena-shot.sh <out.png> [click:X,Y ...] [scroll:X,Y,N ...]
#
# Env:
#   OKENA_BIN        binary under test        (default: $PWD/target/debug/okena)
#   OKENA_SHOT_CFG   isolated XDG_CONFIG_HOME (default: <out dir>/xdg-config)
#   SHOT_RES         virtual monitor size     (default: 1400x1000)
#   OKENA_SHOT_ENV   extra `KEY=VAL` pairs passed to the binary (space separated)
#
# Requires: gnome-shell >= 45, dbus-run-session, gdbus. No root, no Xvfb.
set -u

OUT="${1:?usage: okena-shot.sh <out.png> [click:X,Y ...]}"; shift
BIN="${OKENA_BIN:-$PWD/target/debug/okena}"
WORK="$(cd "$(dirname "$OUT")" && pwd)"
CFG="${OKENA_SHOT_CFG:-$WORK/xdg-config}"
WL="okena-shot-$$"

[ -x "$BIN" ] || { echo "ABORT: no binary at $BIN (cargo build first)"; exit 2; }

# Re-exec inside a private bus so the nested shell can't touch the real session.
if [ -z "${SHOT_INNER:-}" ]; then
  SHOT_INNER=1 OKENA_SHOT_CFG="$CFG" exec dbus-run-session -- bash "$0" "$OUT" "$@"
fi

eval_js() {
  gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell \
    --method org.gnome.Shell.Eval "$1" 2>&1
}

seed_config() {
  mkdir -p "$CFG/okena/profiles/default" "$CFG/okena/profiles/uicheck"
  # A fixed window rect keeps click coordinates reproducible between runs.
  python3 - "$CFG" <<'PYEOF'
import json, os, sys, uuid
cfg = sys.argv[1]
okena = os.path.join(cfg, "okena")
json.dump({"version": 1,
           "profiles": [{"id": "uicheck", "display_name": "UI Check",
                         "created_at": "2026-01-01T00:00:00Z"}],
           "last_used": "uicheck", "default_profile": "uicheck"},
          open(os.path.join(okena, "profiles.json"), "w"), indent=2)
win = {"id": str(uuid.uuid4()), "hidden_project_ids": [], "folder_filter": None,
       "project_widths": {}, "project_layout": "rows", "project_sort_mode": "manual",
       "show_attention_section": True, "folder_collapsed": {},
       "os_bounds": {"origin_x": 0.0, "origin_y": 40.0, "width": 1300.0, "height": 900.0},
       "sidebar_open": False}
for prof in ("default", "uicheck"):
    d = os.path.join(okena, "profiles", prof)
    json.dump({"version": 3, "main_window": win, "extra_windows": [], "project_layouts": {},
               "service_panel_heights": {}, "hook_panel_heights": {}},
              open(os.path.join(d, "window-layout.json"), "w"), indent=2)
    p = os.path.join(d, "settings.json")
    if not os.path.exists(p):
        # The daemon is spawned without --profile, so it reads `default`; both
        # profiles must agree or the daemon's snapshot overrides the client.
        json.dump({"version": 3}, open(p, "w"), indent=2)
PYEOF
}

seed_config

gnome-shell --headless --unsafe-mode --wayland-display="$WL" \
  --virtual-monitor "${SHOT_RES:-1400x1000}" >"$WORK/shell.log" 2>&1 &
SHELL_PID=$!
for _ in $(seq 1 30); do [ -S "$XDG_RUNTIME_DIR/$WL" ] && break; sleep 1; done
[ -S "$XDG_RUNTIME_DIR/$WL" ] || { echo "ABORT: nested shell did not start (see $WORK/shell.log)"; exit 2; }
sleep 2

# shellcheck disable=SC2086
env WAYLAND_DISPLAY="$WL" XDG_CONFIG_HOME="$CFG" ${OKENA_SHOT_ENV:-} \
  "$BIN" --profile uicheck >"$WORK/gui.log" 2>&1 &
OKENA_PID=$!
trap 'kill -9 "$OKENA_PID" 2>/dev/null; kill "$SHELL_PID" 2>/dev/null' EXIT
sleep 9

# Leave the overview: it renders windows scaled and offset, which breaks the
# mapping between screenshot pixels and pointer coordinates.
eval_js "
  if (typeof Main !== 'undefined') Main.overview.hide();
  let w = global.get_window_actors().map(a => a.meta_window)
    .find(w => (w.get_wm_class() || '').toLowerCase().includes('okena'));
  w.activate(global.get_current_time());
  let r = w.get_frame_rect();
  r.x + ',' + r.y + ',' + r.width + ',' + r.height" >"$WORK/rect.log"
sleep 2

VP="const C = imports.gi.Clutter; const G = imports.gi.GLib;
    const seat = C.get_default_backend().get_default_seat();
    if (!globalThis._vp) globalThis._vp = seat.create_virtual_device(C.InputDeviceType.POINTER_DEVICE);"

for step in "$@"; do
  case "$step" in
    click:*)
      xy="${step#click:}"; x="${xy%,*}"; y="${xy#*,}"
      eval_js "$VP _vp.notify_absolute_motion(G.get_monotonic_time(), $x, $y); 'move'" >/dev/null; sleep 1
      eval_js "$VP _vp.notify_button(G.get_monotonic_time(), C.BUTTON_PRIMARY, C.ButtonState.PRESSED); 'down'" >/dev/null; sleep 1
      eval_js "$VP _vp.notify_button(G.get_monotonic_time(), C.BUTTON_PRIMARY, C.ButtonState.RELEASED); 'up'" >/dev/null; sleep 1 ;;
    scroll:*)
      a="${step#scroll:}"; x="${a%%,*}"; rest="${a#*,}"; y="${rest%%,*}"; n="${rest#*,}"
      eval_js "$VP _vp.notify_absolute_motion(G.get_monotonic_time(), $x, $y);
        for (let i = 0; i < $n; i++) { _vp.notify_discrete_scroll(G.get_monotonic_time(), C.ScrollDirection.DOWN, C.ScrollSource.WHEEL); G.usleep(40000); }
        'scrolled'" >/dev/null; sleep 1 ;;
    *) echo "unknown step: $step" ;;
  esac
done

rm -f "$OUT"
eval_js "
const {Gio, Shell} = imports.gi;
let f = Gio.File.new_for_path('$OUT');
let st = f.replace(null, false, Gio.FileCreateFlags.NONE, null);
let s = new Shell.Screenshot();
s.screenshot(false, st, null, (o, res) => { try { s.screenshot_finish(res); } catch (e) { log('SHOTERR ' + e); } st.close(null); });
'started'" >/dev/null
for _ in $(seq 1 15); do [ -s "$OUT" ] && break; sleep 1; done

if [ -s "$OUT" ]; then
  echo "OK $OUT (window rect: $(sed -E "s/.*'\"?([^\"']*)\"?'.*/\1/" "$WORK/rect.log"))"
else
  echo "NO SHOT — see $WORK/shell.log and $WORK/gui.log"
  grep -i SHOTERR "$WORK/shell.log" | tail -3
  exit 1
fi
