# Sprint — Missions and shared attention (2026-10-02)

**Goal.** Give desktop users one reliable attention inbox and lightweight, durable missions that connect work across projects and agent conversations without changing terminal ownership.

**Theme.** A user can find an agent needing input, inspect its real terminal, and follow one piece of work through implementation, review and multiple repository PRs. Missions are optional. The inbox works without them.

This is an implementation plan, not a record of shipped behavior. Work units below are unstarted. The user selected desktop + daemon for the first version and shared read acknowledgment across clients. The detailed contracts below are the proposed implementation baseline to review before WU1.

## Sources and decision

- [RFC #189](https://github.com/contember/okena/issues/189) is the original mission-centric proposal. This plan narrows its first delivery and supersedes its focus-as-resolution and cross-project-layout assumptions for this sprint.
- [Current agent status](../reference/agent-status.md), [daemon ownership](../decisions/0001-headless-two-process-daemon.md), and [test rules](../reference/testing.md) describe the existing constraints.
- Herdr at `d6b40d4`: [completion presentation](https://github.com/herdrdev/herdr/blob/d6b40d4/src/client/shell/endpoint_agent_state.rs) and [agent automation](https://github.com/herdrdev/herdr/blob/d6b40d4/docs/next/website/src/content/docs/agent-automation.mdx) demonstrate separation of lifecycle, viewed completion, terminal control and agent control. Its per-client read state is deliberately not adopted.
- The `n1rna/okena` fork at `43eaf21a` provides precedents for session panels and worktree/PR associations. This plan does not depend on merging that fork or adopting its session-as-project model.

**Decision weight.** Persistent identity, ownership and wire compatibility outweigh UI convenience. Scope is cross-cutting and includes persisted data and public protocol additions; implementation must preserve old workspaces and old clients.

**Alternatives considered.** A flat agent list is cheapest and best when one conversation equals one task, but loses continuity across agents. A ticket-driven orchestration environment is best when Okena must own assignment and execution policy, but is not needed to validate this workflow. Lightweight missions plus an independent inbox fit the multi-repository, multi-session case while allowing ordinary project work.

**Confidence and falsifier.** High confidence in separating missions, sessions and terminals; medium confidence in the default overview layout. If real work rarely spans sessions or worktrees and users avoid naming missions, keep the inbox and revisit mission prominence rather than making missions mandatory.

## Refs re-verified at HEAD (2026-10-02)

Baseline: `9234dfb885efbbf587507f1e3ae4da4a4ecbcfd5`. Recheck these seams if HEAD changes before implementation. Line references describe this baseline.

- ✔ Projects own layouts: `crates/okena-state/src/workspace_data.rs:195`. The desktop is a mirror, not a state writer: `crates/okena-app/src/CLAUDE.md:52`.
- ✔ Conversation identity is `(agent, session_id)`; history is independent of current pane attachments: `crates/okena-core/src/agent_session.rs:28,48`, `crates/okena-workspace/src/state.rs:1776,1800`.
- ✔ Terminal IDs can change on restore. Pending identity follows a layout leaf and is consumed after successful spawn: `crates/okena-workspace/src/persistence.rs:201,253`, `crates/okena-app-core/src/workspace/actions/execute/mod.rs:902,1002`.
- ✔ Layout leaves already hold persistent pending-session metadata: `crates/okena-layout/src/lib.rs:23`. Adding membership metadata must preserve normalization, moves and restore semantics.
- ⚠ Agent lifecycle is runtime-only and can become stale. `done` is a finished turn, not completed work; `clear` does not distinguish session start from end: `docs/reference/agent-status.md:62,66,231,284`.
- ⚠ The OSC parser retains the last status and a dirty flag. Several transitions can arrive in one PTY batch: `crates/okena-terminal/src/terminal/osc_sidecar.rs:412,445`. Session capture already has an owner-only queue at `:504`.
- ✔ PTY generations are validated before parsing, and post-batch ingestion precedes hook-driven removal: `crates/okena-daemon-core/src/pty_loop.rs:208,272,546`.
- ⚠ Desktop `FocusTerminal` is presentation-local and forwards only project activity, not read acknowledgment: `crates/okena-app/src/action_dispatch.rs:260`.
- ✔ Daemon mutations flow through shared actions and workspace observers/autosave: `crates/okena-app-core/src/workspace/actions/execute/mod.rs:81`, `crates/okena-daemon-core/src/command_loop.rs:4775`, `crates/okena-daemon-core/src/observers.rs:247,464`.
- ✔ Client terminal keys are connection-prefixed; clear statuses are explicitly reconciled: `crates/okena-workspace/src/remote_apply.rs:65`. They must not become daemon-persisted identities.
- ⚠ Existing PR data includes URL/number/state/base and CI summary, but no full review/conflict model: `crates/okena-core/src/api.rs:205,234`.
- ✔ Git observations and worktree registration have daemon-owned seams: `crates/okena-daemon-core/src/git_poll.rs:371,411,491`, `crates/okena-workspace/src/actions/worktree.rs:547`.

## First-version user contract

1. **Inbox** lists current input-needed episodes and unread completions across connected, compatible daemons, including agents outside missions. Input-needed comes first; within each group, oldest first with a stable tie-breaker.
2. Opening an inbox item reveals its actual project-owned terminal through the existing window-aware focus path. Merely opening the inbox does not mark everything read. An explicit “Mark read” action is available for a completion with no live terminal.
3. A completion is marked read after its terminal is successfully revealed in the active window, or explicitly marked read. The action targets the observed episode/revision. All clients of that daemon receive the acknowledgment.
4. Reading an input-needed item does not resolve it. A fresh agent lifecycle report showing it has moved on resolves the live block. No generic Allow/Deny buttons or synthetic responses are inferred from terminal text.
5. **Missions** have a title, optional goal and home project, and explicit `Active`, `Done`, `Archived` lifecycle. They can contain several repositories, worktrees and conversations. Marking done or archived does not stop agents, remove worktrees or hide unresolved inbox items.
6. Create a mission from a project, worktree or agent session; attach existing work; rename/edit it; mark done, reopen, archive and restore it. Creation does not require a ticket, new branch, worktree or agent launcher.
7. **Mission board** lists sessions, project/worktree context and PR/CI observations. Selecting a live member uses the existing terminal surface; offline sessions remain visible as history. Projects navigation remains available.
8. The compact mission overview has stable manual/creation order and attention counts. No automatic reordering or collapsing of a terminal while the user is working in it.
9. A mission belongs to exactly one daemon. An attached remote daemon can own missions too; members must come from that same daemon. The desktop combines lists without inventing cross-daemon ownership.

## Proposed data and behavior contracts

### Missions and membership

- Add mission records to daemon-owned `WorkspaceData`, defaulting to empty for old files. Use an opaque generated ID, bounded title/goal, creation timestamp and explicit lifecycle. Keep per-window selection/presentation in the existing client-owned window state.
- Store durable conversation references using the existing validated harness/session identity. Do not copy transcript contents or expose transcript paths merely to render a mission.
- Keep repository participation separate from worktree membership. Adding a main repository does **not** capture every unrelated terminal in it. A linked worktree supplies default membership to work started there.
- Allow one primary mission per worktree or conversation in v1. A repository may participate in many missions. Conflicting assignments require an explicit move; never silently reassign existing work.
- For an explicitly attached terminal without a conversation ID, keep an optional mission binding on its layout leaf, not a durable reference to a replaceable PTY ID. It follows moves/restore; a newly split sibling starts without copying an explicit binding. New terminals inside a linked worktree derive membership from that worktree.
- When a bound terminal reports a conversation identity, record that conversation in the mission. A resumed conversation retains its existing assignment. Do not attach a session that is already assigned elsewhere without an explicit move.
- Worktree creation from a mission carries the mission ID through the existing creation action. Validate ownership up front; finalize membership through the existing registration/rollback path. Do not enforce a branch-name prefix.
- Removing a project/worktree detaches its live membership but retains conversation history and previously observed PR references. Do not recreate deleted projects to satisfy mission references. Missing home projects render as unavailable and can be replaced.
- Named-session loading and workspace import must explicitly remap project/mission IDs and leaf bindings. Never leave imported references pointing to an unrelated existing object. Current ephemeral input-needed state is not imported as a live request.

### Attention episodes

- Keep `AgentLifecycle` backward compatible. Add a separate daemon-owned attention representation with an opaque episode ID/revision, source, kind (`InputNeeded` or `Completion`), bounded summary, daemon timestamp and read state. UI labels must not change the meaning of existing `done` on the wire.
- Capture ordered transitions at the PTY owner, including the session identity valid **at that transition**. A client parsing mirrored terminal bytes must not create authoritative events.
- Deduplicate repeated reports of the same episode. A material change to a waiting summary revises the episode, so a delayed acknowledgment cannot consume new information. A new lifecycle cycle creates a new episode even when the text is identical.
- Retain at most the latest completion per source plus its current input-needed episode. A newer completion supersedes an older one from that source. Read completions may be removed. This is a current-work inbox, not an append-only turn transcript.
- Event validity uses the runtime attachment/generation; completion retention and supersession use daemon-scoped `(agent, session_id)` when known. A completion after resume supersedes the conversation's previous completion without accepting its stale acknowledgment. Before identity is known, use a generation-scoped terminal source; prune resolved/read retired anonymous sources and bound retained unavailable anonymous sources explicitly.
- Replacing the identity on an attachment makes the previous source's unresolved block unavailable, even if `clear` already carries the new identity. It preserves the old completion and acknowledgment identity. Test `blocked(A) → clear(B) → working(B)` and `done(A) → clear(A)` explicitly.
- `working` ends current input-needed attention; `done` ends it and creates a completion; `idle` ends it without inventing a completion. `clear`, terminal exit and daemon restart make unresolved input-needed observations **unconfirmed/unavailable**, not successfully answered. Only a fresh report establishes a live block again. Unavailable items can be dismissed explicitly; dismissing is not answering.
- Completed unread results and shared read state survive restart. Reconnect rehydrates authoritative episodes without generating new ones or desktop notifications. Restored pending blocks must visibly say that live status has not been confirmed.
- Existing terminal bells/unread flags remain distinct. Do not turn every bell into an actionable permission request. Preserve existing desktop notification behavior and avoid adding a second notification producer.
- Input-needed means **last reported blocked**, not independently verified process liveness. An agent crash that leaves its shell running and sends no report remains undetected until another report, pane exit or daemon restart. V1 preserves this existing producer limitation and must document it rather than adding an implicit process detector.
- Acknowledgment validates the observed episode ID/revision and is idempotent. An older acknowledgment must not consume a newer completion or waiting question. No client-local fallback writer for an old/disconnected daemon.
- Bound message sizes and the owner event buffer. Test overflow behavior: coalesce redundant updates while preserving the latest lifecycle/identity and expose loss rather than claiming a complete history. Drain per processed PTY event if necessary; do not add an unbounded queue.

### PR observations and availability

- Reuse the existing daemon git poller for member worktrees. Store a stable PR reference including host/repository/number and the last observed state, with its observation timestamp. Multiple worktrees referring to one PR must produce one item.
- A polling failure does not mean closed, merged or healthy. Keep the last observation marked unavailable/stale. After worktree removal, keep the reference as last-known until a live observation source exists again.
- Display the current PR and CI summary on the board. Independent polling of archived PRs, CI actions, conflict detection and review-request inbox items are deferred.
- Disconnecting one daemon dims its records and disables mutations against them without affecting other daemons. Selection and acknowledgment always retain the source connection; identical raw IDs on two daemons cannot collide.

## Work units

### WU1 — Attention reducer and loss-aware transition capture (effort L)

- **Problem.** Last-status snapshots lose intermediate edges and cannot identify which completion a user read.
- **Verify first.** Read the OSC parser, transport-owner check, PTY generation handling and existing status/session tests. Add one regression fixture containing several transitions and two session identities in one PTY batch.
- **Scope.** Define attention data and pure reducer in the existing core/state layers; capture ordered owner-side events and drain them through the daemon PTY loop before hook/project cleanup. Implement the transition, coalescing, stale-source and buffer-bound rules above.
- **Acceptance / witness.** Tests cover `working → done`, repeated `done`, `blocked → working`, `blocked → idle`, `done → clear`, identity switch in one batch, stale PTY generations, no-identity agents, client mirrors producing no authoritative events, and buffer overflow. A finished turn never marks a mission done.
- **Touch points.** `crates/okena-core/src/agent_status.rs`; new `crates/okena-core/src/attention.rs`; `crates/okena-terminal/src/terminal/{mod.rs,osc_sidecar.rs}`; `crates/okena-daemon-core/src/pty_loop.rs`; `crates/okena-state/src/workspace_data.rs`; new workspace attention module.

### WU2 — Shared acknowledgment, persistence and wire projection (effort L)

- **Problem.** Focus is client-local; read state must be authoritative and reconnect-safe.
- **Verify first.** Trace `GetState`, snapshot construction, workspace observers/autosave and remote ID conversion. Inspect old-client serde behavior before choosing optional snapshot fields/capability handling.
- **Scope.** Add acknowledgment/dismissal actions, persisted attention state and snapshot projection through the existing mutation pipeline. New clients show unsupported state for old daemons instead of pretending shared acknowledgment succeeded. Publish only UI-needed session identity, not private transcript paths.
- **Acceptance / witness.** Daemon tests with two clients show one shared read result; stale and duplicate acknowledgments cannot clear newer episodes. Save/load preserves unread completions. Restart leaves pending blocks unconfirmed. Initial snapshot/reconnect causes no extra episode or notification. Old workspace/snapshot fixtures still load. Two connections with equal raw IDs route to the correct owner.
- **Touch points.** `crates/okena-core/src/api.rs`; `crates/okena-app-core/src/{remote_snapshot.rs,workspace/actions/execute/mod.rs}`; `crates/okena-workspace/src/{persistence.rs,remote_apply.rs}`; `crates/okena-daemon-core/src/{command_loop.rs,observers.rs}`; `crates/okena-app/src/action_dispatch.rs`.

### WU3 — Desktop inbox and reliable reveal (effort M)

- **Problem.** The Agents list exposes lifecycle but has no shared unread completion queue.
- **Verify first.** Trace the existing window-aware `focus_terminal_by_id`, hidden/folder-filtered projects, detached windows and active-tab changes. Verify acknowledgment can occur after successful reveal rather than on a click request.
- **Scope.** Add a toggleable inbox using existing sidebar/window patterns, kind grouping, stable age ordering, counts, source labels, and unavailable states. Jump to the real terminal; send the observed completion acknowledgment after it is visible in the active window. Provide explicit mark-read/dismiss where navigation is unavailable.
- **Acceptance / witness.** GPUI tests exercise reveal through a hidden project, inactive tab and detached window, and ensure a failed reveal does not acknowledge. A background window does not consume completion. Reading a block keeps it pending. Pure ordering tests cover equal timestamps and updates. Manual two-window verification confirms shared read state and ordinary terminal typing/search/focus.
- **Touch points.** New `crates/okena-app/src/views/attention/`; `crates/okena-app/src/views/window/`; `crates/okena-views-sidebar/src/`; `crates/okena-app/src/{action_dispatch.rs,keybindings/}`. Reuse focus/request-broker machinery; do not add a second focus implementation.

### WU4 — Durable missions and membership actions (effort L)

- **Problem.** Work has no durable identity spanning several agent conversations and repositories.
- **Verify first.** Trace session history, project/worktree deletion, pending resume, layout leaf moves and named-session import. Write a migration/restore test before adding fields.
- **Scope.** Add mission types, validation, membership resolution and actions to create/edit/attach/detach/move/change lifecycle. Implement optional terminal leaf binding and conversation promotion. Persist in the existing workspace and project through snapshots; integrate worktree creation and failure rollback. Record the accepted identity/ownership contract in an ADR during implementation.
- **Acceptance / witness.** One mission spans two repositories and two conversations; closing the implementer preserves membership for review. Reload without a session backend rekeys terminals without losing assignments. Failed spawn preserves pending state. Split siblings do not accidentally inherit explicit bindings; moved leaves retain theirs. Worktree creation failure leaves no dangling member. Old files load, invalid/cross-daemon references are rejected, import rekeys references, deleted projects do not destroy conversation history, and an explicit move cannot leave two primary assignments.
- **Touch points.** New `crates/okena-state/src/mission.rs` and workspace mission module; `crates/okena-state/src/{lib.rs,workspace_data.rs}`; `crates/okena-layout/src/lib.rs`; `crates/okena-workspace/src/{state.rs,persistence.rs,sessions.rs,actions/worktree.rs}`; shared action execution, daemon command loop, API/snapshot/remote reconciliation.

### WU5 — Mission overview and board (effort L)

- **Problem.** Durable grouping is useful only if users can create it from existing work and navigate it cheaply.
- **Verify first.** Identify reusable dialogs, cards, window presentation state and the existing terminal/project column. Prototype the board with existing terminal rendering; verify that entering it does not add a second resize writer for the same terminal.
- **Scope.** Add mission creation/attachment entry points on existing project/worktree/agent rows. Add stable compact overview, active/done/archive filtering and a board showing goal, members and attention. Keep Projects navigation. Select a live member through its owner view; do not create a new cross-project `LayoutNode`. Expose offline conversations as history, not live agents. Persist window selection through existing client window state.
- **Acceptance / witness.** User creates a mission from an existing session with no new checkout, attaches a second repository's worktree, navigates both and reopens the mission after restart. Closing/archiving a mission does not kill processes or acknowledge inbox items. UI tests cover selected-member disappearance, return navigation and foreign-daemon selection. Manual check verifies readable terminal width and no resize oscillation between windows.
- **Touch points.** New `crates/okena-app/src/views/missions/`; `crates/okena-app/src/views/{window/,overlays/}`; sidebar/context menus; keybindings; `crates/okena-state/src/window_state.rs`; existing project-column and terminal reveal integration.

### WU6 — Mission PR/CI observations and worktree context (effort M)

- **Problem.** The mission needs to retain identifiable outputs after agents or worktrees disappear.
- **Verify first.** Inspect repository identity helpers and what `poll_github` actually returns. Reuse existing host/repo resolution; do not infer repository identity from a branch name alone.
- **Scope.** Associate observed branch PRs with member worktrees and persist deduplicated last-known references. Render PR links, observed state, CI summaries and worktree dirty/ahead information already available. Retain stale last-known references on polling failure or checkout deletion. Offer explicit “Mark mission done”; cleanup remains the existing separate worktree operation.
- **Acceptance / witness.** Tests distinguish same PR number on different repos/hosts, deduplicate one PR observed via two worktrees, preserve last-known results during failure/deletion, avoid reassigning old PRs after branch changes, and never infer mission completion from PR state. Manual flow opens the correct PR and returns to its mission.
- **Touch points.** `crates/okena-daemon-core/src/git_poll.rs`; existing `okena-git` repository helpers; mission workspace/state module; API/snapshots; mission board.

### WU7 — Compatibility, end-to-end exercise and documentation (effort M)

- **Problem.** Several individually correct surfaces can still lose identity or attention during restore and remote reconciliation.
- **Verify first.** Re-run the smallest integration scenarios from WU1–6 against the combined changes. Inspect CI's current formatter/linter/build commands instead of guessing flags.
- **Scope.** Run the release gates below, document shipped behavior and current integration limitations, and record manual results. Finish new ADR and update references/indices. Keep all unchecked work explicitly unshipped.
- **Acceptance / witness.** Complete the acceptance scenario below with two clients and a daemon restart. Existing web/mobile clients still load snapshots. The standalone daemon remains GPUI-free. Report actual command results and platform coverage, not inferred support from successful compilation.
- **Touch points.** Existing daemon/API/GPUI tests; `docs/reference/agent-status.md`; new missions reference; `docs/reference/{remote.md,README.md}`; docs indices and ADR index.

## Sequencing and checkpoints

| Order | Deliverable | Checkpoint |
|---|---|---|
| WU1 → WU2 → WU3 | Shared inbox over current projects | Use several real agents; check missed/duplicate attention and reveal behavior before expanding UI. |
| WU4 → WU5 | Lightweight missions and board | Follow one real task across implementation and review. Confirm that grouping earns its creation cost. |
| WU6 → WU7 | Outputs, compatibility and final verification | Complete the multi-repository scenario and record evidence. |

Implement sequentially by default. Shared API, snapshot, workspace and action files overlap heavily; parallel writes are not assumed. Each WU should be independently buildable and verified. If the inbox checkpoint fails, fix it before adding mission UI. If terminal rendering requires cross-project ownership changes, stop and revisit the board composition instead of widening this sprint silently.

## Acceptance scenario

1. Open two desktop clients/windows on one daemon; retain an unrelated project and agent outside any mission.
2. Create a mission from an existing agent session in repository A. Attach a worktree from repository B. Keep an unrelated terminal in A outside the mission.
3. Run supported reporting agents; one becomes blocked, one finishes a turn. Both appear in the inbox, including any unrelated agent needing attention.
4. Open the completed result in client 1: client 2 no longer lists it unread. Opening the blocked agent does not resolve the block. Its next working report does.
5. Submit an acknowledgment from a stale snapshot after another completion. The newer completion stays unread.
6. Close the implementation pane and use another session for review. Both conversations remain associated with the mission.
7. Observe PRs for both worktrees, lose a poll/connection, then remove one checkout through the existing workflow. The board retains correctly identified, explicitly last-known PR information.
8. Restart the daemon without a persistent session backend. Mission and read state survive; terminal rekeying retains bindings; pending blocks are unconfirmed rather than falsely live or answered.
9. Mark the mission done and archive it. No process is killed and any unresolved attention remains reachable. Restore the mission.
10. Attach a second daemon with colliding raw IDs in a fixture. Its inbox item routes to that daemon; membership across the two daemons is refused. Disconnect it and verify the first remains usable.

## Verification commands and test policy

- Follow [test selection](../reference/testing.md). Add tests for reducers, race handling, reference validation/remapping, persistence and focus behavior. Do not add tests for trivial setters or copy implementation into simulated tests.
- For each WU, run focused tests in the changed crates. Typical gate: `cpu-lease run -n 2 -- cargo test -p okena-core -p okena-state -p okena-workspace -p okena-daemon-core`; narrow further with a test filter while iterating. Terminal/layout/GPUI units also require their corresponding crate tests.
- Final Rust gate: `cpu-lease run -n 4 -- cargo test --workspace`, plus current CI's formatting and clippy checks under a CPU lease where CPU-bound. Build embedded web assets using the repository's existing build procedure if required.
- Verify `cargo tree -i gpui -p okena-daemon` has no dependency path to GPUI. Check the web/mobile snapshot decoding compatibility through their existing test/build tooling where affected.
- No benchmark is required. No build/test results are claimed by this planning document.

## Out of scope

- Full web/mobile mission or inbox UI; their API compatibility is in scope.
- One mission spanning multiple daemons; a unified desktop list is in scope.
- Cross-project splits, duplicated live-terminal tiles, wall mode and auto-collapsing/reordering columns. First validate compact overview plus existing terminal navigation.
- A new agent launcher/orchestrator, task providers, Knowledge/OpenSpec, prompt templates, WASM extensions, token billing and a generic event timeline.
- Terminal screen scraping, new agent integrations, or native Windows reporting support. V1 uses current explicit reporting; unsupported agents still work as terminals. Supporting additional producers can follow the same daemon contracts later.
- Generic inline answers/Allow/Deny, CI rerun/fix actions, PR review/conflict inbox items, independent archived-PR polling and automatic worktree cleanup on mission completion.

## Rollback and next step

Keep additions backward-readable and the existing Projects path usable. Disabling the new UI must not discard mission or read data. Older binaries can ignore new fields but may drop them on save; do not promise downgrade round-trip preservation. Use the existing config checkpoint/recovery behavior and document this before release.

Review the proposed contracts, then begin with WU1. Do not start a broad UI rewrite or cherry-pick the fork as a prerequisite. At each checkpoint, report verified behavior and remaining work before proceeding.

## Run log

- 2026-10-02 — Planning only. User selected desktop + daemon scope and daemon-shared read acknowledgment. Baseline and integration seams inspected; no implementation or test run yet.
- 2026-10-02 — User approved full subagent implementation, disjoint single-checkout waves and local commits. Plan committed as `eabc3735`. Independent contract review clarified source replacement, conversation-level completion supersession and the surviving-shell crash limitation. Web assets built successfully with `cpu-lease run -n 2 -- bun run build`.
