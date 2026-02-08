# Commit

Commit staged and unstaged changes following the rules below.

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

## Splitting into logical commits

- If you have context about what work was done (e.g. you just finished implementing something), commit **only** that work. Don't bundle unrelated changes — leave other unstaged/untracked changes alone.
- You MAY split your work into multiple logical commits if it makes sense (e.g. a refactor commit + a feature commit, or separating test changes from implementation).
- If you have no prior context, read `git diff` and `git diff --cached` to understand all changes, then group them into reasonable logical commits by topic/purpose.
- Each commit should be self-contained and buildable on its own when possible.

## Procedure

1. Run `git status` and `git diff` (staged + unstaged) to see all changes.
2. Review the changes and decide how to split them into commits (if needed).
3. For each commit:
   - Stage the relevant files with `git add <specific files>` (never `git add -A` or `git add .`)
   - Create the commit
4. Run `git status` after all commits to verify nothing was missed.
