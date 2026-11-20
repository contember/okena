#!/usr/bin/env bash
# Decides whether the nightly builds the commit its run started on.
#
#   nightly-gate.sh <ref> <sha> < runs.json
#
# runs.json is the response of
#   GET /repos/{repo}/actions/workflows/build.yml/runs?head_sha=<sha>
# Prints `build` when <ref> is main and the newest push run of build.yml on
# main for exactly <sha> completed successfully, otherwise `skip: <reason>`.
# Exits non-zero only on bad arguments or input that is not a runs response.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: nightly-gate.sh <ref> <sha> < runs.json" >&2
  exit 2
fi
ref=$1
sha=$2

if [[ "$ref" != refs/heads/main ]]; then
  echo "skip: the run started on $ref, nightlies build main only"
  exit 0
fi

# A re-run keeps its run id, so the highest id is the newest run of the commit.
jq -r --arg sha "$sha" '
  if (.workflow_runs | type) != "array" then error("not a workflow runs response") else . end
  | [.workflow_runs[] | select(.head_sha == $sha and .head_branch == "main" and .event == "push")]
  | max_by(.id) as $run
  | if $run == null then "skip: \($sha) has no Build run on main"
    elif $run.status != "completed" then "skip: Build run \($run.html_url) of \($sha) is \($run.status)"
    elif $run.conclusion != "success" then "skip: Build run \($run.html_url) of \($sha) concluded \($run.conclusion)"
    else "build"
    end
'
