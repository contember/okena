#!/usr/bin/env bash
#
# audit-daemon-client-gaps.sh — find daemon/client wiring "holes".
#
# In the daemon-client architecture the desktop GUI (crates/okena-app + the
# gpui view crates) is a THIN CLIENT: its `Workspace` is a read-only MIRROR and
# the daemon is the single writer + owner of PTYs/services/git. Two recurring
# classes of bug ("X isn't transferred / X isn't wired") are statically
# detectable:
#
#   CAT 1 — GUI does the daemon's job: a handler mutates the mirror Workspace,
#           spawns/kills a local PTY, or writes persistence, instead of
#           dispatching an ActionRequest to the daemon. (Silent no-op / wrong
#           process write.)
#   CAT 2 — daemon drops data: a field exists on a domain type but not on its
#           wire `Api*` projection, so it never reaches the client. (Shows as
#           empty/None — "unwired".)
#
# This is a heuristic linter, not a proof: CAT 1 uses a denylist of data-mutating
# calls (visual/presentation mutations are allowlisted); CAT 2 is a field-name
# diff. Triage each hit. Exit non-zero if any CAT 1 hit is found (CI-gateable).
#
# Usage:  scripts/audit-daemon-client-gaps.sh
set -uo pipefail
cd "$(dirname "$0")/.."

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

# GUI client crates that must NOT mutate workspace data / run local exec.
GUI_GLOBS=(crates/okena-app/src crates/okena-views-sidebar/src \
           crates/okena-views-git/src crates/okena-views-services/src \
           crates/okena-views-terminal/src crates/okena-views-remote/src)

bold "== CAT 1: GUI client doing the daemon's job (should be an ActionRequest) =="
echo "   (each hit: a mirror write / local PTY / local persistence in client code)"
echo

# Data-mutating Workspace methods + local-exec + persistence that the client
# must route to the daemon instead. Visual/presentation mutators are NOT here on
# purpose (update_split_sizes, toggle_*minimized, set_fullscreen, exit_fullscreen,
# focus_*, set_folder_collapsed, set_active_tab, *_ui_only — all client-local-OK).
CAT1_PATTERNS=(
  'persistence::save_workspace'
  # `ws.`-anchored so a view's own dispatch method (this.add_project /
  # this.delete_project, which route to an ActionRequest) isn't a false positive.
  '\bws\.add_project\('
  '\bws\.delete_project\('
  '\.create_worktree_project\('
  '\.remove_worktree_project\('
  '\.add_discovered_worktree\('
  '\.add_to_worktree_ids\('
  '\.replace_data\('
  '\.set_terminal_shell\('
  'set_global\((crate::)?(workspace::)?hooks::HookRunner'
  '\.backend\.create_terminal\('
  '\.backend\.kill\('
  'ServiceManager::new\('
  'okena_git::get_git_status\('   # local git read — returns None in client mode
  # Sidebar folder/project/order/pin/color data mutations — all daemon-owned;
  # the GUI must dispatch the matching ActionRequest, not write its mirror.
  # (`toggle_worktree_visibility` is deliberately absent: it's per-window
  # presentation state — `toggle_hidden(window_id, ...)` — that stays client-local.)
  '\bws\.create_folder\('
  '\bws\.delete_folder\('
  '\bws\.rename_folder\('
  '\bws\.rename_project\('
  '\bws\.move_project\('
  '\bws\.move_project_to_folder\('
  '\bws\.move_item_in_order\('
  '\bws\.toggle_project_pinned\('
  '\bws\.reorder_worktree\('
  '\bws\.set_folder_color\('
  '\bws\.set_folder_item_color\('
  '\bws\.set_worktree_color_override\('
)

cat1_hits=0
for pat in "${CAT1_PATTERNS[@]}"; do
  # Exclude test modules, the snapshot reconciler, the action dispatcher, and
  # the daemon/headless owners (legitimate writers).
  matches=$(grep -rnE "$pat" "${GUI_GLOBS[@]}" 2>/dev/null \
            | grep -vE '(/tests?/|_test\.rs|#\[cfg\(test\)\]|remote_apply\.rs|action_dispatch\.rs|/app/headless\.rs)' \
            | grep -vE '^\s*//' )
  if [ -n "$matches" ]; then
    red "  ▸ $pat"
    echo "$matches" | sed 's/^/      /'
    cat1_hits=$((cat1_hits + $(echo "$matches" | grep -c .)))
    echo
  fi
done
[ "$cat1_hits" -eq 0 ] && green "  none" && echo

bold "== CAT 2: domain fields dropped from the wire (Domain -> Api*) =="
echo "   (each: a field on the domain struct with no same-named field on Api*)"
echo

# Extract `pub <field>:` names from a named struct block in a file.
struct_fields() {
  local file="$1" struct="$2"
  awk -v s="pub struct $struct" '
    $0 ~ s {inb=1}
    inb && /^\}/ {inb=0}
    inb && /^[[:space:]]*pub [a-z_]+:/ {
      line=$0; sub(/^[[:space:]]*pub /,"",line); sub(/:.*/,"",line); print line
    }
  ' "$file" 2>/dev/null | sort -u
}

# pairs: "Domain|domain_file|Api|api_file"
PAIRS=(
  "GitStatus|crates/okena-git/src/lib.rs|ApiGitStatus|crates/okena-core/src/api.rs"
  "ProjectData|crates/okena-state/src/workspace_data.rs|ApiProject|crates/okena-core/src/api.rs"
  "FolderData|crates/okena-state/src/workspace_data.rs|ApiFolder|crates/okena-core/src/api.rs"
  "WindowState|crates/okena-state/src/window_state.rs|ApiWindow|crates/okena-core/src/api.rs"
)

# Domain fields that are intentionally NOT on the wire (client-local presentation
# or daemon-internal), so they don't count as gaps:
#   connection_id / is_remote — set by the client's snapshot reconciler, not the
#                               daemon (the daemon's own projects are is_remote=false).
#   service_terminals         — daemon-internal persistence routing; the client
#                               gets live service terminal ids via ApiServiceInfo.
#   hidden_terminals          — dormant placeholder (`#[allow(dead_code)]`); actual
#                               per-terminal visibility flows through
#                               LayoutNode.minimized/detached, which ARE on the wire.
CAT2_IGNORE='^(version|service_panel_heights|hook_panel_heights|connection_id|is_remote|service_terminals|hidden_terminals)$'

cat2_hits=0
for pair in "${PAIRS[@]}"; do
  IFS='|' read -r dname dfile aname afile <<< "$pair"
  dfields=$(struct_fields "$dfile" "$dname")
  afields=$(struct_fields "$afile" "$aname")
  [ -z "$dfields" ] && { red "  ! could not read $dname in $dfile (struct moved?)"; continue; }
  missing=""
  while IFS= read -r f; do
    [ -z "$f" ] && continue
    echo "$f" | grep -qE "$CAT2_IGNORE" && continue
    # present if the same field name appears on the Api struct
    echo "$afields" | grep -qxF "$f" || missing="$missing $f"
  done <<< "$dfields"
  if [ -n "$missing" ]; then
    red "  ▸ $dname -> $aname  missing:$missing"
    cat2_hits=$((cat2_hits + $(echo $missing | wc -w)))
  else
    green "  ✓ $dname -> $aname"
  fi
done
echo

bold "== summary =="
echo "  CAT 1 (GUI doing daemon's job): $cat1_hits hit(s)"
echo "  CAT 2 (fields dropped from wire): $cat2_hits field(s) — triage (some may be intentional client-local)"
echo
echo "  Note: heuristic. CAT 2 false-positives are expected for genuinely"
echo "  client-local/derived fields; add them to CAT2_IGNORE once confirmed."

# Gate on CAT 1 only (CAT 2 needs human triage).
[ "$cat1_hits" -eq 0 ]
