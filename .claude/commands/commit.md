---
description: Commit staged and unstaged changes following Okena's message and splitting conventions.
allowed-tools: Bash(git:*)
---

# Commit

## Commit message format

Use semantic commit messages matching the project convention:

```
<type>: <short summary in imperative mood>
```

Types: `feat`, `fix`, `refactor`, `perf`, `chore`, `ci`, `docs`, `test`

- Summary is lowercase, no period at the end
- Imperative mood ("add X", not "added X" or "adds X")
- Keep the first line under 72 characters
- Add a blank line + body only if the "why" isn't obvious from the summary

## Splitting and staging

Group unrelated changes into separate commits; each one should compile on its own
(`cargo check`). Stage files explicitly — never `git add -A` or `git add .`, since
this tree routinely carries worktree and cross-branch edits that are not yours to
commit (see the Git Rules in the root `CLAUDE.md`).
