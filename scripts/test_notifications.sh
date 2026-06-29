#!/usr/bin/env bash
#
# Emit terminal notification/activity escape sequences for manual daemon/headless
# propagation testing.
#
# Run this inside an Okena terminal. For native desktop notification checks, run
# it in a background tab/pane or unfocused window: Okena intentionally suppresses
# OS notifications for the pane the user is actively looking at.
set -euo pipefail

interval="1.25"
repeat=1
quiet=0

usage() {
  cat <<'EOF'
Usage: scripts/test_notifications.sh [options]

Options:
  --interval SECONDS  Delay between events (default: 1.25)
  --repeat COUNT      Repeat the full sequence COUNT times (default: 1)
  --fast              Shortcut for --interval 0.15
  --quiet             Do not print explanatory labels before events
  -h, --help          Show this help

What this emits:
  - BEL terminal bell
  - OSC 9 plain notifications (BEL and ST terminated)
  - OSC 777 rich notifications
  - OSC 99 kitty notifications, including chunked and base64 payloads
  - OSC 9;4 progress updates, which should NOT create notifications
  - OSC 133;D command-finished activity edge

Expected manual checks in daemon/headless mode:
  - Background panes should raise native notifications when notification
    settings are enabled.
  - Focused panes should not raise native notifications, by design.
  - Bell/OSC attention should reach the daemon-owned state and client sidebar.
  - OSC 9;4 progress should update progress state, not produce a notification.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --interval)
      if [ "$#" -lt 2 ]; then
        printf 'error: --interval requires a value\n' >&2
        exit 2
      fi
      interval="$2"
      shift 2
      ;;
    --repeat)
      if [ "$#" -lt 2 ]; then
        printf 'error: --repeat requires a value\n' >&2
        exit 2
      fi
      repeat="$2"
      shift 2
      ;;
    --fast)
      interval="0.15"
      shift
      ;;
    --quiet)
      quiet=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown option: %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$repeat" in
  ''|*[!0-9]*)
    printf 'error: --repeat must be a positive integer\n' >&2
    exit 2
    ;;
  0)
    printf 'error: --repeat must be greater than zero\n' >&2
    exit 2
    ;;
esac

ESC=$'\033'
BEL=$'\007'
ST=$'\033\\'

pause() {
  if [ "$interval" != "0" ] && [ "$interval" != "0.0" ]; then
    sleep "$interval"
  fi
}

label() {
  if [ "$quiet" -eq 0 ]; then
    printf '\n[%s] %s\n' "$(date '+%H:%M:%S')" "$1"
  fi
}

osc_bel() {
  printf '%s]%s%s' "$ESC" "$1" "$BEL"
}

osc_st() {
  printf '%s]%s%s' "$ESC" "$1" "$ST"
}

emit_bell() {
  printf '%s' "$BEL"
}

if [ "$quiet" -eq 0 ]; then
  cat <<EOF
Okena notification propagation test
TERM=${TERM:-unknown}  interval=${interval}s  repeat=${repeat}

Tip: for native notification bubbles, leave this pane unfocused or run it in a
background tab/pane. Focused-pane suppression is expected behavior.
EOF
fi

run=1
while [ "$run" -le "$repeat" ]; do
  suffix="run ${run}/${repeat}, pid $$"
  kitty_id="okena-test-${run}-$$"

  label "BEL: terminal bell (${suffix})"
  emit_bell
  pause

  label "OSC 9: plain notification, BEL terminated (${suffix})"
  osc_bel "9;Okena OSC 9 notification (${suffix})"
  pause

  label "OSC 9: plain notification, ST terminated (${suffix})"
  osc_st "9;Okena OSC 9 ST notification (${suffix})"
  pause

  label "OSC 777: rich notification with title/body (${suffix})"
  osc_bel "777;notify;Okena OSC 777;Rich body from ${suffix}"
  pause

  label "OSC 777: rich notification preserving semicolons (${suffix})"
  osc_st "777;notify;Okena OSC 777 ST;body contains; semicolons; ${suffix}"
  pause

  label "OSC 99: kitty title-only notification (${suffix})"
  osc_bel "99;;Okena OSC 99 title-only notification (${suffix})"
  pause

  label "OSC 99: kitty chunked title/body notification (${suffix})"
  osc_bel "99;i=${kitty_id}:d=0;Okena OSC 99 chunked"
  sleep 0.1
  osc_bel "99;i=${kitty_id}:p=body;Chunked body from ${suffix}"
  pause

  label "OSC 99: kitty base64 notification (${suffix})"
  # "Encoded from OSC99" in standard base64.
  osc_bel "99;e=1;RW5jb2RlZCBmcm9tIE9TQzk5"
  pause

  label "OSC 9;4: progress update, should NOT create a notification (${suffix})"
  osc_bel "9;4;1;42"
  sleep 0.25
  osc_bel "9;4;3;0"
  sleep 0.25
  osc_bel "9;4;0;0"
  pause

  label "OSC 133;D: command-finished activity edge, not a notification (${suffix})"
  osc_st "133;D;0"
  pause

  run=$((run + 1))
done

if [ "$quiet" -eq 0 ]; then
  printf '\nDone. Re-run with --repeat N or --fast for stress testing.\n'
fi
