# Agent Workflow Guidelines

## Iteration Protocol

1. **Read** `INSTRUCTIONS.md` and `STATUS.md` in the plan directory.
2. **Pick** the first unchecked (`- [ ]`) issue from STATUS.md.
3. If an `issues/` subdirectory exists, read the matching issue file for details.
4. **Implement** that single issue — one issue per iteration, no more.
5. **Verify** your work:
   - Run the build (`cargo build`, `npm run build`, etc.)
   - Run tests (`cargo test`, `npm test`, etc.)
   - Run lints if configured (`cargo clippy`, `eslint`, etc.)
   - Fix any errors before proceeding.
6. **Commit** your changes with a clear, conventional commit message:
   - Format: `type(scope): description` (e.g., `feat(parser): add CSV support`)
   - Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`
   - Keep the subject line under 72 characters.
7. **Update STATUS.md** — check off the completed issue: change `- [ ]` to `- [x]`.
8. If the project has a `CLAUDE.md`, update it when you introduce new patterns,
   conventions, or architectural decisions that future agents should know about.

## Completion

When all issues in STATUS.md are checked off (`- [x]`), respond with:

```
<done>promise</done>
```

This sentinel tells Kruh the plan is complete. Do not emit it if any issues remain.

## Error Handling

- If you encounter a blocking error you cannot resolve, describe it clearly in your
  output and stop. Do not loop on the same error.
- If the build or tests fail after your change, attempt to fix them. If you cannot
  fix them within a reasonable effort, revert your change, explain the blocker, and
  move on.

## Quality Standards

- Follow existing code style and patterns in the project.
- Do not introduce unnecessary dependencies.
- Keep changes minimal and focused on the current issue.
- Do not refactor unrelated code unless the issue specifically asks for it.
