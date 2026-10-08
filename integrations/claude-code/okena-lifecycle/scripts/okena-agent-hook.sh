#!/bin/sh

event=$(cat) || exit 0
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 0
printf '%s' "$event" | OKENA_AGENT=claude-code "$script_dir/okena-agent-status.sh" "$1"

if [ -n "${OKENA_TERMINAL_ID:-}" ] && command -v okena >/dev/null 2>&1 &&
    okena help mission context >/dev/null 2>&1; then
    printf '%s' "$event" | okena mission context --claude-hook
fi
exit 0
