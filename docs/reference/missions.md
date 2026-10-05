# Missions

Missions are optional, durable groups of work on one daemon. A mission can connect
repositories, worktrees and agent conversations across implementation and review.
Ordinary projects and terminals work without missions, and the
[attention inbox](agent-status.md#shared-attention-inbox) includes agents outside
missions.

## Desktop navigation

The desktop has **Projects**, **Inbox** and **Missions** navigation. Missions open
a compact panel alongside the existing project-owned terminal surface. They do
not create cross-project splits or a second copy of a live terminal.

- Choose **+ New mission · <daemon>** in Missions to enter a title, optional goal
  and optional home project. Creation requires no ticket, branch, checkout or
  agent launch.
- The project/worktree context menu has **Create / attach to mission…**. The
  selected work can seed a new mission or be attached to an existing one on the
  same daemon. Agent rows also provide a mission entry point.
- Select a mission to see its goal, home project, members, attention and observed
  PR/CI information. **Attach existing work** lists candidates from that daemon.
  Retained offline conversations remain candidates after they are detached.
  Membership controls offer **Attach**, **Detach**, or an explicit **Move from
  ‘<mission>’ to this mission** when another mission owns the member.
- **Open terminal** reveals a member's real terminal through the existing
  window-aware navigation. A conversation without a current attachment is
  labeled **offline history**; its identity remains visible.
- **Edit title, goal and home** edits the mission. **Active**, **Done** and
  **Archived** filter the overview. Missions keep their stored creation order;
  attention counts do not reorder them.

Window navigation and selected mission are client presentation state. Selection
includes the connection ID as well as the mission ID. Disconnected daemons show
dimmed, last-known records and cannot receive mission mutations. An older daemon
without the work-overview capability is shown as unsupported.

## Identity and membership

A mission has a generated opaque ID, creation timestamp, title, optional goal,
optional home-project reference and lifecycle. Titles must contain 1–256 bytes,
must not be whitespace-only and cannot contain control characters. Goals allow
up to 4096 bytes without NUL. The home project is context, not implicit membership.

| Member | Meaning |
|---|---|
| Repository | Explicit repository participation. A repository may participate in several missions. Adding it does not claim every terminal or child worktree. |
| Worktree | One primary mission per worktree. It supplies default membership to work in that checkout. |
| Conversation | One primary mission per validated `(agent, session_id)` identity, independent of a live terminal. |
| Terminal | An explicit binding on a project layout leaf, useful before a conversation identity is known. The PTY ID is not the durable membership key. |

When a bound terminal or a terminal in a linked worktree reports a conversation,
the daemon adds that identity to the mission. An already assigned conversation
keeps its assignment when resumed elsewhere. Attaching a member owned by another
mission fails unless the caller explicitly chooses a move.

**Individual detach overrides the worktree default.** Detaching a terminal stores
an exclusion on its leaf; detaching a known conversation stores a durable
conversation exclusion. Later reports and restore do not silently put that work
back into the worktree's mission. A later explicit attachment or move can clear
the corresponding exclusion.

Leaf bindings and exclusions follow the leaf through moves and restore, including
replacement of terminal IDs. A newly split sibling does not copy an explicit
binding or exclusion. It can still derive membership from its worktree. Detaching
a repository or worktree removes that association, not the conversation history
already recorded in the mission.

Under **Create worktree for this mission**, choose **Create worktree in
<project>…** to open the existing **Create Worktree** dialog. It shows
**Mission: <title>** and carries that mission's connection and ID through
submission. Only repositories on the owning daemon are offered, and submission
rechecks that the repository, mission and connection are still available.
Ordinary worktree creation outside this entry point does not attach a mission.

The [remote action API](remote.md#mission-and-attention-actions) carries the
optional mission ID. The daemon validates it before creation and attaches the
worktree through the registration path. This does not require a branch-name
prefix.

## Mission lifecycle is not an agent turn

| Mission state | Desktop actions |
|---|---|
| Active | **Mark done**, **Archive** |
| Done | **Reopen**, **Archive** |
| Archived | **Restore** (returns to Active) |

These are explicit user choices. An agent's `done` report only finishes a turn.
Reading its completion only acknowledges that attention episode. Neither changes
mission lifecycle, and a merged PR does not complete a mission automatically.

Marking a mission done or archived does not stop agents, delete worktrees or mark
attention read. Unresolved attention remains reachable in the Inbox regardless
of the mission filter. Cleanup uses the separate [worktree workflow](worktrees.md).

## Durable history and PR observations

Closing a terminal removes its live attachment, not its conversation membership.
Removing a project or checkout removes live project/worktree membership while
retaining conversation identities and previously observed PR references. A
missing home project is shown as unavailable and can be replaced in the editor;
it is not recreated automatically.

Mission history contains conversation identities, not copied transcript contents.
The board is not a transcript viewer. The remote work overview includes only
public conversation identity and attachment data, without local transcript paths.
`conversation_history` lists retained identities independently of live attachments
or mission membership.
See [session resume](agent-status.md#session-resume) for the separate pane resume
record.

The daemon's git poller supplies PR observations for participating projects and
worktrees. PR identity is `(host, repository, number)`, so multiple checkouts of
the same PR produce one item. The board shows its link, last observed state,
observation age and CI summary, together with available branch/change/ahead/behind
context for members.

A failed poll, removed observation source or daemon restart does not mean that a
PR closed, merged or became healthy. Retained observations are marked last-known
until a live source supplies an observation again. There is no independent
archived-PR polling, review/conflict inbox, CI action or automatic cleanup here.

## Authority, persistence and compatibility

The daemon owns missions, membership, attention and shared read state. Clients
submit actions to that daemon; they do not persist a competing copy of those
records. A mission cannot contain members from another daemon. The desktop can
combine multiple daemons' lists while retaining the source connection for every
selection and action.

Mission data, exclusions and attention are persisted with the workspace. Old
workspace files default to empty mission/attention data. Named-session loading
and workspace import remap project and mission references and leaf bindings;
imported attention is not treated as a live input request. Restart preserves
unread completions and shared read state, while pending input observations become
unconfirmed until fresh reporting.

**Before downgrading**, stop Okena and preserve a separate copy of the current
`workspace.json` in the [configuration directory](configuration.md). An older
binary may load the file by ignoring these new fields and then discard them when
it saves. Backward-readable does not mean downgrade round-trip preservation.
Normal saves maintain a rolling `workspace.json.bak`, but repeated saves can
replace that backup; it is not a substitute for the preserved copy. Restore the
copy with a compatible version while Okena is stopped.

The ownership decision is recorded in
[ADR-0003](../decisions/0003-missions-and-shared-attention.md).
