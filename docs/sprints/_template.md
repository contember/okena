<!--
On close, prepend an OUTCOME block here, then `git mv` this file to ../archive/:

> **OUTCOME — shipped YYYY-MM-DD.** <one-paragraph result.> Commit map: WU1 → <sha>,
> WU2 → <sha>, … Verification: <the gate command + numbers>. Backlog closed:
> <ids deleted/rescoped>. Deferred: <honest notes>.
-->

# Sprint — <theme> (YYYY-MM-DD)

**Goal.** <one sentence: the single theme this sprint delivers.>

**Theme.** <why these items belong together; the success condition for the batch.>

## Refs re-verified at HEAD (YYYY-MM-DD)

Re-read the load-bearing facts in the actual code before planning on them.
`✔` = confirmed live · `⚠` = drift/nuance caught.

- ✔/⚠ <claim> — `path/to/file.rs:NN`.

## Work units

### WU1 — <title> (effort S/M/L)

- **Problem.** <what's wrong at HEAD; cite `file:line`.>
- **Verify first.** <cheap checks to run before writing code.>
- **Scope.** <what lands, in priority order.>
- **Acceptance / witness.** <the test or observation that proves it — not "looks
  done".>
- **Touch points.** <files / modules.>

### WU2 — <title> (effort S/M/L)

- **Problem.**
- **Verify first.**
- **Scope.**
- **Acceptance / witness.**
- **Touch points.**

## Out of scope (explicit)

<What's deliberately deferred + why; link follow-up backlog items.>

## Decisions

<Decisions resolved for this sprint. If one constrains future work or rejected a
real alternative, also write a ../decisions/ ADR and link it here.>

## Sequencing

<Order + what can run in parallel. A short table if it helps.>

## Run log

<!-- Append as you work: discoveries, deviations, blockers. Graduate each entry:
     changed the *why* → ../decisions/NNNN ; new future work → ../backlog/NN ;
     transient → leave it (dies with the sprint on archive). After graduating,
     trim to a one-line pointer ("→ ADR-0007"). -->
