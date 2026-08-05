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
# Output device: prefer the pane's *current* slave pty named by
# `$OKENA_TTY_FILE`, then the spawn-time `$OKENA_TTY`, then the controlling
# terminal. See the selection block below for why that order. This drains stdin
# so a hook feeding event JSON on the pipe never blocks, and is a silent no-op
# when there's no device to write to — safe to call from anywhere.
#
# Debugging: set OKENA_AGENT_STATUS_LOG=/path/to/log to append one line per
# invocation recording the state, the target device and where it came from, and
# whether the write actually succeeded. This makes the whole path observable —
# pair it with Okena's own `okena_terminal::terminal::osc_sidecar` debug logs
# (the receiving end) to see where a status update is lost. Unset → zero
# overhead, no file.

state="${1:-}"
message="${2:-}"
# Harness id (e.g. "claude-code", "codex"), set by the per-agent hook glue. Used
# only to tag the captured session in the optional lbl= field; empty is fine.
agent="${OKENA_AGENT:-}"

# Pane this agent runs in, stamped onto the sequence so Okena can drop a status
# that reaches the wrong pane. Empty outside Okena — the param is then omitted.
terminal_id="${OKENA_TERMINAL_ID:-}"

# Pick the device to write to, preferring Okena's own slave pty for this pane.
#
# 1. `$OKENA_TTY_FILE` — a stable path whose CONTENTS Okena rewrites on every
#    spawn and reattach. The only source that stays correct for a pane that
#    outlives Okena: with a session backend (dtach/tmux/screen) the shell — and
#    the agent under it — survives a restart, so its environment still holds the
#    pty of the *previous* run, which Linux has since handed to another pane.
# 2. `$OKENA_TTY` — the same device, captured once at spawn. Correct until the
#    pane outlives the Okena that launched it; also the only one an Okena older
#    than the pointer file exports.
# 3. `/dev/tty` — last resort. It always resolves to the pane's current pty, but
#    under a session backend that is the NESTED pty: with `session_backend =
#    tmux` the pane process is tmux, so a hook's controlling terminal is tmux's
#    own pty, and tmux forwards only a fixed allowlist of OSC numbers — 9001 is
#    not on it, so the sequence dies before reaching Okena's reader. Screen
#    behaves the same way. And a harness that runs its hooks in a new session
#    (Claude Code does) has no controlling terminal at all.
#
# Writing to a device 1 or 2 named means writing to Okena's own slave, which
# bypasses any nested pty. When both are stale the `tid=` param below contains
# the damage: Okena drops a status that lands in a recycled pane rather than
# driving another agent's indicator.
#
# Every probe MUST stay in a subshell. POSIX makes a redirection error on a
# special built-in (`:`) exit the shell, so a bare `: >/dev/tty` aborts this
# script under dash in exactly the case the fallback exists for — and since this
# runs as a PreToolUse hook, a non-zero exit blocks the tool call.
tty_dev=""
tty_src="none"

if [ -n "${OKENA_TTY_FILE:-}" ] && [ -r "$OKENA_TTY_FILE" ]; then
    live_tty=$(head -n1 "$OKENA_TTY_FILE" 2>/dev/null | tr -d '\r\n')
    if [ -n "$live_tty" ] && (: >"$live_tty") 2>/dev/null; then
        tty_dev="$live_tty"
        tty_src="file"
    fi
fi

if [ -z "$tty_dev" ] && [ -n "${OKENA_TTY:-}" ] && (: >"$OKENA_TTY") 2>/dev/null; then
    tty_dev="$OKENA_TTY"
    tty_src="env"
fi

if [ -z "$tty_dev" ] && (: >/dev/tty) 2>/dev/null; then
    tty_dev="/dev/tty"
    tty_src="ctty"
fi

# Nothing writable: keep a name so the write below fails as `write=failed` in the
# log rather than against an empty path.
if [ -z "$tty_dev" ]; then
    tty_dev="${OKENA_TTY:-/dev/tty}"
fi

# Append a debug line when OKENA_AGENT_STATUS_LOG points somewhere writable;
# a silent no-op otherwise. Never fails the hook.
log() {
    [ -n "${OKENA_AGENT_STATUS_LOG:-}" ] || return 0
    ts=$(date '+%Y-%m-%dT%H:%M:%S%z' 2>/dev/null || echo '????')
    printf '%s pid=%s state=%s tty=%s src=%s %s\n' \
        "$ts" "$$" "${state:-<none>}" "$tty_dev" "$tty_src" "$1" \
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

# Reduce $event to just its TOP-LEVEL key/value pairs.
#
# The regex below is greedy, so it picks the *last* match on the line — and
# Claude Code's PreToolUse/PostToolUse payloads embed `tool_input` as nested
# JSON whose keys are not string-escaped. A tool argument named `session_id` or
# `transcript_path` (routine for MCP tools) would therefore shadow the real
# field: the forged value becomes the pane's sticky session, gets persisted, and
# with auto-resume on becomes the argument to `claude --resume`.
#
# Repeatedly collapse the innermost {...} / [...] until only the top level is
# left. Bounded so a pathological payload can't spin.
event_top=""
if [ -n "$event" ]; then
    event_top=${event#\{}
    event_top=${event_top%\}}
    nesting=0
    while [ "$nesting" -lt 16 ]; do
        flatter=$(printf '%s' "$event_top" | sed 's/{[^{}]*}/""/g; s/\[[^][]*\]/""/g' 2>/dev/null)
        [ "$flatter" = "$event_top" ] && break
        event_top=$flatter
        nesting=$((nesting + 1))
    done
fi

# `jq` gives an exact answer when it's around; the sed path keeps the script
# dependency-free otherwise.
has_jq=$(command -v jq 2>/dev/null || true)

# Print the string value of TOP-LEVEL JSON key $1 in $event, or nothing.
json_str() {
    if [ -n "$has_jq" ]; then
        printf '%s' "$event" | jq -r --arg k "$1" '.[$k] // empty' 2>/dev/null
        return 0
    fi
    printf '%s' "$event_top" | sed -n \
        "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" 2>/dev/null | head -n1
}
session_id=$(json_str session_id)
transcript_path=$(json_str transcript_path)

# Assemble the optional lbl= JSON object only when we actually have a session id
# (the durable bit Okena persists). Values are JSON-escaped (\\ then ").
lbl_json=""
if [ -n "$session_id" ]; then
    # Okena needs `agent` to know which harness could resume this session, and
    # drops the whole session without it. Say so rather than emitting a label
    # object Okena will silently discard.
    [ -n "$agent" ] || log "skip=no-agent (set OKENA_AGENT to capture the session)"
    json_escape() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g' 2>/dev/null; }
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

# `$state` is spliced straight into the parameter list, so accept only the
# states Okena knows. Without this a caller passing through a less-trusted value
# could append `;lbl=<b64>` (planting an arbitrary session Okena would persist
# and later resume) or `;tid=` to retarget the status at another pane — and a
# plain typo would be dropped by the receiver while still logging `write=ok`.
case "$state" in
    working | blocked | done | idle | clear) ;;
    *)
        log "skip=bad-state"
        exit 0
        ;;
esac

# Assemble OSC 9001 params: `st` is required; `msg`/`lbl` are optional and
# base64-encoded so their values stay ';'/ST-safe (the VTE parser splits OSC
# params on ';').
params="st=$state"
# Address the status at our own pane, so Okena can drop it if it lands
# elsewhere (a stale device path since recycled by another pane). Omitted
# outside Okena, where the receiver accepts anything.
if [ -n "$terminal_id" ]; then
    params="$params;tid=$terminal_id"
fi
if [ -n "$message" ]; then
    msg_b64=$(printf '%s' "$message" | base64 2>/dev/null | tr -d '\n')
    params="$params;msg=$msg_b64"
    msg_info="msglen=${#message}"
else
    msg_info="msglen=0"
fi
if [ -n "$lbl_json" ]; then
    lbl_b64=$(printf '{%s}' "$lbl_json" | base64 2>/dev/null | tr -d '\n')
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
