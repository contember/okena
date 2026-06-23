#!/bin/sh
# okena-agent-status — report an AI agent's lifecycle to Okena.
#
# Writes Okena's agent-status OSC (OSC 9001) to the controlling terminal so the
# Okena tab + the sidebar "Agents" section reflect what the agent is doing, and
# so Okena raises a desktop notification when the agent finishes or gets blocked.
#
# Usage:
#   okena-agent-status <working|blocked|done|idle|clear> [message]
#
# Designed to be wired up as a Claude Code hook (see docs/agent-status.md), but
# it's agent-agnostic — anything that can run a command can call it.
#
# Output device: a hook runs as a subprocess with NO controlling terminal, so
# `/dev/tty` is unavailable to it. Okena therefore exports `OKENA_TTY` (the
# pane's slave pty path) into the pane's environment; we write there. Writing to
# the slave reaches Okena's reader even through a nested session backend
# (dtach/tmux). Falls back to `/dev/tty` for the interactive case. It drains
# stdin so a hook feeding event JSON on the pipe never blocks, and is a silent
# no-op when there's no device to write to — safe to call from anywhere.
#
# Debugging: set OKENA_AGENT_STATUS_LOG=/path/to/log to append one line per
# invocation recording the state, the target device, and whether the write
# actually succeeded. This makes the whole path observable — pair it with
# Okena's own `okena_terminal::terminal::osc_sidecar` debug logs (the receiving
# end) to see where a status update is lost. Unset → zero overhead, no file.

state="${1:-}"
message="${2:-}"
# Harness id (e.g. "claude-code", "codex"), set by the per-agent hook glue. Used
# only to tag the captured session in the optional lbl= field; empty is fine.
agent="${OKENA_AGENT:-}"

tty_dev="${OKENA_TTY:-/dev/tty}"

# Append a debug line when OKENA_AGENT_STATUS_LOG points somewhere writable;
# a silent no-op otherwise. Never fails the hook.
log() {
    [ -n "${OKENA_AGENT_STATUS_LOG:-}" ] || return 0
    ts=$(date '+%Y-%m-%dT%H:%M:%S%z' 2>/dev/null || echo '????')
    printf '%s pid=%s state=%s tty=%s %s\n' \
        "$ts" "$$" "${state:-<none>}" "$tty_dev" "$1" \
        >>"$OKENA_AGENT_STATUS_LOG" 2>/dev/null || true
}

# Capture any hook event JSON on stdin (Claude Code & co. feed it there) so the
# writer never blocks, then mine it for the agent's session id / transcript path
# to forward to Okena. No `jq` dependency — a narrow regex over the
# machine-generated JSON, with a clean fallback to "no session label" when the
# fields aren't present.
event=""
if [ ! -t 0 ]; then
    event=$(cat 2>/dev/null || true)
fi

# Print the first string value of JSON key $1 found in $event, or nothing.
json_str() {
    printf '%s' "$event" | sed -n \
        "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" | head -n1
}
session_id=$(json_str session_id)
transcript_path=$(json_str transcript_path)

# Assemble the optional lbl= JSON object only when we actually have a session id
# (the durable bit Okena persists). Values are JSON-escaped (\\ then ").
lbl_json=""
if [ -n "$session_id" ]; then
    json_escape() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
    add_kv() {
        [ -n "$2" ] || return 0
        ev=$(json_escape "$2")
        if [ -n "$lbl_json" ]; then
            lbl_json="$lbl_json,\"$1\":\"$ev\""
        else
            lbl_json="\"$1\":\"$ev\""
        fi
    }
    add_kv agent "$agent"
    add_kv session_id "$session_id"
    add_kv transcript_path "$transcript_path"
fi

# Nothing to do without a state.
if [ -z "$state" ]; then
    log "skip=no-state"
    exit 0
fi

# Assemble OSC 9001 params: `st` is required; `msg`/`lbl` are optional and
# base64-encoded so their values stay ';'/ST-safe (the VTE parser splits OSC
# params on ';').
params="st=$state"
if [ -n "$message" ]; then
    msg_b64=$(printf '%s' "$message" | base64 | tr -d '\n')
    params="$params;msg=$msg_b64"
    msg_info="msglen=${#message}"
else
    msg_info="msglen=0"
fi
if [ -n "$lbl_json" ]; then
    lbl_b64=$(printf '{%s}' "$lbl_json" | base64 | tr -d '\n')
    params="$params;lbl=$lbl_b64"
    msg_info="$msg_info sid=$session_id"
fi
seq=$(printf '\033]9001;%s\033\\' "$params")

# Write to the device, recording success/failure for the debug log. The write is
# allowed to fail silently (no device, not in Okena) — that's a clean no-op.
# `2>/dev/null` comes first so a failed-to-open redirection (e.g. no such
# device) is suppressed too: shell redirections apply left to right, so stderr
# must already point at /dev/null before the `>"$tty_dev"` open is attempted.
if printf '%s' "$seq" 2>/dev/null >"$tty_dev"; then
    log "$msg_info write=ok"
else
    log "$msg_info write=failed (device unwritable: $tty_dev)"
fi

exit 0
