---
id: 04
title: Hand an agent knowledge entries when it launches
blocked-by: [../sprints/sprint-2026-09-10-knowledge-stores.md]
---

# 04 — Hand an agent knowledge entries when it launches

**Summary.** Let the user pick knowledge entries (docs, skills, agents) when
starting an agent session, and let a running agent look knowledge up itself.
Effort M–L.

## Problem

Every launch flow hands the agent only its opening prompt and a working
directory: `TaskStartWork` (`execute/tasks.rs`), `SpecDraftChange`
(`execute/specs.rs` `brief`), `AgentStartSession` (`execute/tasks.rs`
`custom_brief`), and break-down (`views/harness/new_task_form.rs`). The org's
engineering principles, CI notes and service boundaries never reach it unless
the user pastes them in. The only thing injected automatically is okena's MCP
config (`execute/agent_mcp.rs`).

## Approach / acceptance

- A knowledge picker in the start-work, draft-spec and new-agent dialogs. It
  suggests entries from the stores the project follows (`.okena/knowledge.yaml`
  `stores:`) plus the project's own root.
- Deliver chosen entries per agent capability. Docs are listed in the brief as
  absolute paths with their descriptions, so the agent reads them rather than
  having megabytes inlined. Skills and agents are handed over the way the agent
  CLI takes them (for claude: a session-scoped plugin/skills directory), with a
  per-agent table like `agent_mcp::injection_args`.
- MCP tools `okena_knowledge_list` and `okena_knowledge_read` (in
  `okena-cli/src/mcp.rs`), scoped to the session project's followed stores and
  going through the daemon's discovered-key and path guards.
- Witness: an executor test that a started session's brief names the selected
  entries, and an MCP test that a read outside a followed store is refused.

## Touch points

`okena-core/src/api.rs` (launch actions gain `knowledge: Vec<KnowledgeRef>`),
`execute/{tasks,specs,agent_mcp}.rs`, `okena-cli/src/mcp.rs`, the three launch
dialogs in `okena-app/src/views/`.

<!-- Origin: sprint-2026-09-10-knowledge-stores, "Out of scope". -->
