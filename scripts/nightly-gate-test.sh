#!/usr/bin/env bash
# Tests for nightly-gate.sh. Run: scripts/nightly-gate-test.sh
set -uo pipefail

gate="$(dirname "$0")/nightly-gate.sh"
sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
other=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
failures=0

# run <id> <sha> <branch> <event> <status> <conclusion>
run() {
  jq -n --argjson id "$1" --arg sha "$2" --arg branch "$3" --arg event "$4" \
    --arg status "$5" --arg conclusion "$6" \
    '{id: $id, head_sha: $sha, head_branch: $branch, event: $event, status: $status,
      conclusion: (if $conclusion == "" then null else $conclusion end),
      html_url: "https://example.test/runs/\($id)"}'
}

runs() {
  jq -s '{total_count: length, workflow_runs: .}'
}

# expect <name> <ref> <expected prefix> < runs.json
expect() {
  local name=$1 ref=$2 want=$3 got
  got=$("$gate" "$ref" "$sha" 2>&1)
  if [[ "$got" == "$want"* ]]; then
    echo "ok   $name"
  else
    echo "FAIL $name: want '$want…', got '$got'"
    failures=$((failures + 1))
  fi
}

main=refs/heads/main

runs < /dev/null | expect "no run" $main "skip: $sha has no Build run on main"

run 1 $sha main push completed success | runs | expect "green run" $main "build"

run 1 $sha main push in_progress "" | runs \
  | expect "running" $main "skip: Build run https://example.test/runs/1 of $sha is in_progress"

run 1 $sha main push queued "" | runs | expect "queued" $main "skip: Build run https://example.test/runs/1 of $sha is queued"

run 1 $sha main push completed failure | runs \
  | expect "red run" $main "skip: Build run https://example.test/runs/1 of $sha concluded failure"

run 1 $sha main push completed cancelled | runs | expect "cancelled" $main "skip: Build run https://example.test/runs/1 of $sha concluded cancelled"

{ run 1 $sha main push completed success; run 2 $sha main push completed failure; } | runs \
  | expect "newest run red" $main "skip: Build run https://example.test/runs/2"

{ run 2 $sha main push completed success; run 1 $sha main push completed failure; } | runs \
  | expect "newest run green, listed first" $main "build"

{ run 1 $sha main push completed success; run 2 $sha main push in_progress ""; } | runs \
  | expect "newest run still running" $main "skip: Build run https://example.test/runs/2 of $sha is in_progress"

run 1 $other main push completed success | runs | expect "green run of another commit" $main "skip: $sha has no Build run on main"

run 1 $sha feature push completed success | runs | expect "green on another branch" $main "skip: $sha has no Build run on main"

run 1 $sha main pull_request completed success | runs | expect "green pull_request run" $main "skip: $sha has no Build run on main"

run 1 $sha main workflow_dispatch completed success | runs | expect "green dispatch run" $main "skip: $sha has no Build run on main"

run 1 $sha main push completed success | runs \
  | expect "dispatched on a branch" refs/heads/feature "skip: the run started on refs/heads/feature, nightlies build main only"

# Malformed input must fail the step, not read as a skip.
for input in '{}' '{"message":"Bad credentials"}' 'not json'; do
  if printf '%s' "$input" | "$gate" $main $sha > /dev/null 2>&1; then
    echo "FAIL malformed input accepted: $input"
    failures=$((failures + 1))
  else
    echo "ok   malformed input rejected: $input"
  fi
done

if "$gate" $main < /dev/null > /dev/null 2>&1; then
  echo "FAIL missing sha accepted"
  failures=$((failures + 1))
else
  echo "ok   missing sha rejected"
fi

if [[ $failures -ne 0 ]]; then
  echo "$failures failed"
  exit 1
fi
echo "all passed"
