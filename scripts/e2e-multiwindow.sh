#!/usr/bin/env bash
#
# e2e-multiwindow.sh — multi-window quit/restore E2E under headless GNOME.
#
# Guards the recurring "only one window after relaunch" regression: quit flows
# that deliver an OS close to every window (GNOME dock Quit, logout, Alt+F4
# per window) must NOT wipe extra windows from window-layout.json before the
# final quit-time save (see commit "stop quit-time window closes from wiping
# the multi-window layout"). Deliberate single closes must still be forgotten
# (multi-window PRD user story 22).
#
# How: boots a nested `gnome-shell --headless --unsafe-mode` compositor,
# seeds an isolated XDG_CONFIG_HOME with a layout of main + 2 extra windows,
# launches okena against it, and closes windows via org.gnome.Shell.Eval
# `meta_window.delete()` — real compositor close requests, the same path as
# Alt+F4 / dock Quit. The spawned okena daemon is isolated too (own config
# hash -> own socket; TCP falls back within 19100-19200).
#
# Scenarios:
#   A  restore 2 extras, close ALL windows (main last)  -> layout keeps 2
#   B  restore again,    close ALL windows (main first) -> layout keeps 2
#   C  close ONE extra, app keeps running -> forgotten after the 5s grace,
#      no zombie reopen; final quit keeps the surviving extra
#
# Requirements: GNOME Shell >= 45 (headless mode), dbus-run-session, python3.
# Wayland/X11 session state is NOT touched — everything runs nested.
#
# Usage: ./scripts/e2e-multiwindow.sh
#   OKENA_BIN=/path/to/okena   binary under test (default: target/debug/okena)
#   OKENA_E2E_KEEP=1           keep the scratch dir (logs, config) on exit
#
# Exit code = number of failed assertions.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
BIN="${OKENA_BIN:-$PROJECT_ROOT/target/debug/okena}"

if [ ! -x "$BIN" ]; then
  echo "ABORT: okena binary not found at $BIN (run 'cargo build' or set OKENA_BIN)"
  exit 2
fi

# Re-exec inside a private D-Bus session so the nested shell and gdbus calls
# can't touch the real desktop session.
if [ -z "${OKENA_E2E_INNER:-}" ]; then
  OKENA_E2E_INNER=1 exec dbus-run-session -- bash "$0" "$@"
fi

WORK="$(mktemp -d -t okena-e2e-XXXXXX)"
CFG="$WORK/xdg-config"
LAYOUT="$CFG/okena/profiles/default/window-layout.json"
GUILOG="$WORK/gui.log"
WL_DISPLAY="okena-e2e-$$"
FAILURES=0
SHELL_PID=""
OKENA_PID=""

cleanup() {
  [ -n "$OKENA_PID" ] && kill -9 "$OKENA_PID" 2>/dev/null
  [ -n "$SHELL_PID" ] && kill "$SHELL_PID" 2>/dev/null
  if [ -n "${OKENA_E2E_KEEP:-}" ] || [ "$FAILURES" -gt 0 ]; then
    echo "scratch kept at: $WORK"
  else
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

fail() { echo "FAIL: $1"; FAILURES=$((FAILURES+1)); }
pass() { echo "PASS: $1"; }

seed_config() {
  rm -rf "$CFG"
  mkdir -p "$CFG"
  python3 - "$CFG" <<'PYEOF'
import json, os, sys, uuid

cfg_root = sys.argv[1]
okena = os.path.join(cfg_root, "okena")
profile = os.path.join(okena, "profiles", "default")
os.makedirs(profile, exist_ok=True)

json.dump({
    "version": 1,
    "profiles": [{"id": "default", "display_name": "Default",
                  "created_at": "2026-01-01T00:00:00Z"}],
    "last_used": "default",
    "default_profile": "default",
}, open(os.path.join(okena, "profiles.json"), "w"), indent=2)

def window(x):
    # Field values must parse as okena-state WindowState (serde rejects the
    # whole layout otherwise and the app silently starts fresh).
    return {
        "id": str(uuid.uuid4()),
        "hidden_project_ids": [],
        "folder_filter": None,
        "project_widths": {},
        "project_layout": "rows",
        "project_sort_mode": "manual",
        "show_attention_section": True,
        "folder_collapsed": {},
        "os_bounds": {"origin_x": float(x), "origin_y": 0.0,
                      "width": 700.0, "height": 500.0},
        "sidebar_open": False,
    }

json.dump({
    "version": 3,
    "main_window": window(0),
    "extra_windows": [window(720), window(1440)],
    "project_layouts": {},
    "service_panel_heights": {},
    "hook_panel_heights": {},
}, open(os.path.join(profile, "window-layout.json"), "w"), indent=2)
PYEOF
}

extras_count() {
  python3 -c "import json;print(len(json.load(open('$LAYOUT'))['extra_windows']))"
}

shell_eval() {
  gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell \
    --method org.gnome.Shell.Eval "$1" 2>&1
}

okena_window_count() {
  shell_eval "global.get_window_actors().filter(a => (a.meta_window.get_wm_class()||'').toLowerCase().includes('okena')).length" \
    | sed -E "s/.*'([0-9]+)'.*/\1/"
}

# Close every okena window via real compositor close requests (== Alt+F4 /
# dock Quit). $1 is extra JS run on the `ws` array first ("" = map order,
# main first; "ws.reverse();" = extras first, main last).
close_all_okena() {
  shell_eval "
    let ws = global.get_window_actors()
      .map(a => a.meta_window)
      .filter(w => (w.get_wm_class()||'').toLowerCase().includes('okena'));
    $1
    ws.forEach(w => w.delete(global.get_current_time()));
    ws.length;" >/dev/null
}

close_one_extra() {
  # Topmost okena window in stacking order = the last-mapped extra.
  shell_eval "
    let ws = global.get_window_actors().map(a => a.meta_window)
      .filter(w => (w.get_wm_class()||'').toLowerCase().includes('okena'));
    ws[ws.length-1].delete(global.get_current_time());" >/dev/null
}

start_shell() {
  gnome-shell --headless --unsafe-mode --wayland-display="$WL_DISPLAY" \
    --virtual-monitor 2200x900 >"$WORK/shell.log" 2>&1 &
  SHELL_PID=$!
  for _ in $(seq 1 20); do
    [ -S "$XDG_RUNTIME_DIR/$WL_DISPLAY" ] && break
    sleep 1
  done
  [ -S "$XDG_RUNTIME_DIR/$WL_DISPLAY" ] || { echo "ABORT: nested shell did not start (see $WORK/shell.log)"; exit 2; }
  sleep 2
}

start_okena() {
  WAYLAND_DISPLAY="$WL_DISPLAY" XDG_CONFIG_HOME="$CFG" "$BIN" >"$GUILOG" 2>&1 &
  OKENA_PID=$!
}

wait_okena_windows() {
  local want=$1
  for _ in $(seq 1 60); do
    [ "$(okena_window_count)" = "$want" ] && return 0
    sleep 1
  done
  echo "  (window count now: $(okena_window_count), wanted $want)"
  return 1
}

wait_okena_exit() {
  for _ in $(seq 1 30); do
    if ! kill -0 "$OKENA_PID" 2>/dev/null; then OKENA_PID=""; return 0; fi
    sleep 1
  done
  return 1
}

echo "== setup (binary: $BIN) =="
seed_config
start_shell
echo "nested shell up (pid $SHELL_PID, display $WL_DISPLAY)"

echo "== scenario A: restore 2 extras, quit-all (main closed LAST) =="
start_okena
if wait_okena_windows 3; then pass "A1 restore opened 3 windows (main + 2 extras)"; else fail "A1 restore did not open 3 windows"; fi
grep -q "Opening 2 extra window(s)" "$GUILOG" && pass "A2 log shows 'Opening 2 extra window(s)'" || fail "A2 missing restore log line"
sleep 2
close_all_okena "ws.reverse();"   # extras first, main last
if wait_okena_exit; then pass "A3 app exited after closing all windows"; else fail "A3 app did not exit"; kill -9 "$OKENA_PID"; OKENA_PID=""; fi
sleep 1
c=$(extras_count)
if [ "$c" = "2" ]; then pass "A4 layout kept 2 extras after quit-all (was the bug)"; else fail "A4 layout has $c extras, expected 2"; fi

echo "== scenario B: restore again, quit-all (main closed FIRST) =="
start_okena
if wait_okena_windows 3; then pass "B1 restore opened 3 windows again"; else fail "B1 restore did not open 3 windows"; fi
sleep 2
close_all_okena ""                # map order: main first
if wait_okena_exit; then pass "B2 app exited"; else fail "B2 app did not exit"; kill -9 "$OKENA_PID"; OKENA_PID=""; fi
sleep 1
c=$(extras_count)
if [ "$c" = "2" ]; then pass "B3 layout kept 2 extras (main-first order)"; else fail "B3 layout has $c extras, expected 2"; fi

echo "== scenario C: deliberate single close still forgets (story 22) =="
start_okena
if wait_okena_windows 3; then pass "C1 restore opened 3 windows"; else fail "C1 restore did not open 3 windows"; fi
sleep 2
close_one_extra
sleep 8   # forget grace 5s + autosave debounce 0.5s + margin
c=$(extras_count)
if [ "$c" = "1" ]; then pass "C2 single OS close forgot the extra after grace"; else fail "C2 layout has $c extras, expected 1"; fi
wc=$(okena_window_count)
if [ "$wc" = "2" ]; then pass "C3 no zombie reopen (2 windows remain)"; else fail "C3 window count $wc, expected 2"; fi
close_all_okena "ws.reverse();"
if wait_okena_exit; then pass "C4 app exited"; else fail "C4 app did not exit"; kill -9 "$OKENA_PID"; OKENA_PID=""; fi
sleep 1
c=$(extras_count)
if [ "$c" = "1" ]; then pass "C5 final layout kept the 1 surviving extra"; else fail "C5 layout has $c extras, expected 1"; fi

echo "== done: $FAILURES failure(s) =="
exit "$FAILURES"
