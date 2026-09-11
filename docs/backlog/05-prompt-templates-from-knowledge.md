---
id: 05
title: Build launch prompts from knowledge-store templates
blocked-by: [../sprints/sprint-2026-09-10-knowledge-stores.md]
---

# 05 — Build launch prompts from knowledge-store templates

**Summary.** Replace the hardcoded agent briefs with templates from a
knowledge store's `templates/` folder, falling back to built-in defaults.
Effort M.

## Problem

Prompts are Rust `format!` strings: the spec brief (`execute/specs.rs`
`brief`), the free-form brief (`execute/tasks.rs` `custom_brief`), the
break-down goal (`views/harness/new_task_form.rs`), and the
`harness.agent_args` substitution of `{key}`/`{title}`/`{url}`/`{branch}`
(`execute/tasks.rs` `substitute`). An organisation can't change how its agents
are briefed without a release.

## Approach / acceptance

- Define the flow ids and the variable set per flow (`task-start`,
  `spec-draft`, `agent-session`, `break-down`). A template declares
  `for: [<flow>]` in frontmatter (ADR-0003) and uses `{name}` placeholders, the
  same syntax `agent_args` already uses.
- Resolution order: a template the user picked at launch, then the project's
  followed store, then any store's template for the flow, then the built-in
  brief. Unknown placeholders are left verbatim and reported, never silently
  emptied.
- The launch dialog shows the rendered prompt before sending.
- Witness: pure tests for rendering (every variable, an unknown placeholder, an
  escaped brace) and the resolution order. A test that each built-in brief
  renders identically through the template path, so the switch changes nothing
  for a user with no templates.

## Touch points

`execute/{tasks,specs}.rs`, a small renderer (likely in `okena-knowledge`),
the launch dialogs, `docs/reference/knowledge.md`.

<!-- Origin: sprint-2026-09-10-knowledge-stores, "Out of scope". -->
