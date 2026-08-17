# Review Workspace — UI specification (v2)

Status: approved design, being implemented. Product background: `review-workspace-product-plan.md`.
This document is the source of truth for the review UI. Where the product plan and this
document differ on presentation, this document wins.

## 1. Goal

When a reviewer opens a branch comparison the view must answer, in order:

1. **How big is it really?** Implementation volume vs. supporting volume (tests, fixtures, docs, config).
2. **Where do I start?** One ordered list; every row states the reasons that put it there.
3. **Show me.** The line diff, with the changed symbol's signature and call changes one keystroke away.

Non-goals for this iteration: notes / reviewed state, evidence links, AI summaries, PR targets,
mutable (working tree / staged) targets, per-commit file lists. The shell reserves space for them.

## 2. Principles

- **Rank from git facts first.** The ordered list is built from the inventory (roles, paths, churn,
  status, rename similarity) the moment it loads — for 100 % of files, every language. Structure
  analysis (tree-sitter) then promotes and annotates the rows it reached. The list is never empty
  because tree-sitter did not run.
- **Honest coverage.** Structure-derived counts are lower bounds when analysis is partial and are
  shown as `≥ N`. A capped run is “limited”, never “complete”. Debug enum names never reach the screen.
- **One number per fact.** No repeated totals, no zero-valued cells.
- **Pipeline is status, not content.** One status pill in the header; details in a popover; one
  caveat line next to the ranked list. No coverage strips.
- **Navigator, not tabs.** Two navigator modes — *Files* (tree) and *Attention* (ordered list). No
  Inventory / Structure / CallDiff tabs. Call changes belong to the selected function.
- **Measurements, not scores.** Deterministic tiers with the reason spelled out. No opaque number, no AI.
- **Selection always lands somewhere.** Every row opens something.
- **Small change, small screen.** ≤ 10 files or ≤ 500 changed lines → skip the Overview and open the
  first ranked file. Content width < 1 000 px → Overview reflows to one column.

## 3. Shell

```
┌ header: base → head · merge-base <sha> │ N files · +A −D · C commits │ [status pill] [Whitespace] [Unified/Split] [⧉] [✕] ┐
├────────────────────────┬─────────────────────────────────────────────────────────────────────────────┤
│ navigator (resizable)  │ content: Overview  |  File view                                             │
│  [Files N] [Attention] │                                                                              │
│  filter box   (/)      │                                                                              │
│  [Roles · all 11 ▾]    │                                                                              │
│  tree  |  ordered list │                                                                              │
│  footer: filter state  │                                                                              │
├────────────────────────┴─────────────────────────────────────────────────────────────────────────────┤
│ footer: keys that work on THIS screen (both halves used)                                             │
└──────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

- Header title: `origin/main → feat/workers-host`; for `DiffMode::Commit` the commit subject. Merge-base
  short SHA with the exact base/head/merge-base OIDs on hover. Totals from the exact diff when loaded,
  else from inventory totals. Whitespace toggle unchanged (`w`).
- The content area shows the Overview until a file / symbol / directory is selected. `o` returns to it.
- Legacy commit-info bar (`[`/`]` between commits) stays only for `DiffMode::Commit` with a commit list.

## 4. State (client-side)

Held on the diff viewer next to `SmartReviewState`:

- `navigator: Files | Attention`
- `content: Overview | File` — the open file is `SmartReviewState::selected_file` (single source of truth);
  `selected_symbol: Option<SymbolRef { file: ReviewFileKey, change_index }>`; `queue_target:
  Option<AttentionTarget>` (stable identity; the position is derived from the visible Attention order)
- `role_filter: RoleFilter { roles: set of FileRole, preset }` — presets: *Everything* (default),
  *Review code* = Implementation + Configuration + Unclassified, *Supporting* = Test, Fixture,
  Snapshot, Example, Documentation. Extra saved filters: *likely mechanical only*, *not analyzed only*.
- `attention_filter: set of ReasonKind` (OR) + `include_tests: bool` (default false)
- `expanded_dirs`, `flatten: bool`, `attention_grouped_by_file: bool`
- `details_expanded: bool` (symbol details), remembered for the session
- transient: roles menu open, status popover open, outline popover open

## 5. Review model (pure, derived from inventory + structure + coverage)

Built client-side whenever inventory or structure lands (or fails). **Filter-independent**: role filter,
filter text, `include_tests` and grouping are applied by pure view functions over the model, never by
rebuilding it. Files are keyed by `ReviewFileKey`; model order is inventory order and is **not** the diff
pane's `file_stats` order. No GPUI types.

- **FileEntry** per inventory file: key, path(s), status, role + rule id, similarity, lines added/deleted,
  binary, analysis status (Parsed/Partial/Pending/Unsupported/Failed/Skipped/not-in-structure),
  reasons (see §6), tier, changed symbols (from structure), churn.
- **Directory aggregation**: tree of directories with file count and summed +/− over all files (the navigator
  recomputes totals over the visible subset);
  single-child chains joined; `no test files changed next to it` flag on implementation directories
  (an implementation directory = contains ≥ 1 implementation-role file; “next to it” = any test-role
  file under the same directory subtree; computed on the top-most directory that has both kinds where
  applicable — see §6 tier 4).
- **Volume by role**: files and changed lines (added + deleted) per role, percentage of total changed
  lines; all 11 roles listed, roles with 0 files omitted from display but present in the model.
- **Facts**: Public API (`removed`, `signatures changed`, `added` counts of Public/Exported symbols;
  `lower_bound: bool` when coverage < total), Tests (implementation directories with / without test
  changes), Moves (renames split into likely mechanical = residual ≤ 20 lines vs with edits), Commits
  (count, merge count = commits with > 1 parent, authors, span, first/last SHA), Also (lockfiles,
  submodule pointers, binary files, deleted implementation files).
- **Attention list**: ordered `AttentionItem { target: Symbol{file, symbol_change_index} | File(key) |
  Directory(path), tier, reasons: Vec<Reason>, lines_added, lines_deleted, navigation: Option<target> }`,
  deduplicated (a symbol or file appears once with all reasons). “Start here” = first 10 items.
- **Analysis status** for the pill: `LoadingInventory | AnalyzingStructure | Ready{files, languages} |
  Limited{analyzed, total} | ReadyWithFailures{failed} | Unavailable`. Any active truncation
  (FileLimit / FactLimit / ResponseLimit / …) or ≥ 1 parse failure ⇒ amber, never green.
- **Omission groups** in words: one sentence per `OmittedFileReason` (e.g. “Not analyzed — file limit
  (200), taken in path order”, “Unsupported language”, “Skipped — mode-only change”, “Failed to parse”)
  with counts, languages/extensions, and the resolved OIDs.

## 6. Ranking — tiers and reasons

Deterministic. Every item shows the reasons that placed it. Structure signals apply only to files that
were analyzed; unanalyzed files rank from git facts and carry `not analyzed · <lang>`.

| Tier | Rows | Source |
|---|---|---|
| 1 Contract | Public/Exported symbol removed · deleted implementation file · public signature changed (signature **and** body ranks above signature only) | `SymbolChange` + `visibility()`; `ReviewFileFact.status` |
| 2 Behaviour | Changed functions with call changes — removed or modified calls first; control context (`ErrorBranch`, `Condition`, `Loop`, …) named in the reason | `CallDiffChange` + control context |
| 3 Volume | Most-edited existing implementation functions (changed lines) · largest new implementation functions (line count) and types (member count) — separate measures | `ChangedLines`, `FunctionLineCount`, `TypeMemberCount` **intersected with `SymbolChange`** (hotspots are emitted for every head-side symbol) |
| 4 Git facts | Implementation directory with no test-file changes next to it · CI / config / lockfile / submodule touches · new implementation files by size (any language) · renames with residual > 20 lines · binary implementation files | roles, paths, status, similarity, churn |
| 5 Rest | Every other changed symbol, then remaining files by churn; likely-mechanical moves last | — |

- Within a tier: implementation role first, then number of reasons, then changed lines desc, then path.
- Unclassified counts as implementation for ranking.
- Complexity (`SyntacticNestingDepth ≥ 5`, `ParameterCount ≥ 6`) is never a tier; it is an extra reason on
  a changed symbol worded “changed code in an already complex function”.
- Renames judged by residual lines (`lines_added + lines_deleted`), not similarity.
- Test-role items carry `is_test`; the visible list hides them unless `include_tests` (tiers are unaffected).
- Reason kinds (used as chips and filters): `PublicRemoved`, `PublicSignature`, `ExportedSignature`,
  `Body`, `Calls{n, context}`, `New{lines}`, `NewPublic`, `Removed`, `Moved{similarity, residual}`,
  `NoTestChanges`, `CiConfig`, `Lockfile`, `Submodule`, `Binary`, `Complex{depth|params}`,
  `NotAnalyzed{lang}`, `LargeChurn`.
- Chip wording: `public symbol removed`, `public signature`, `exported signature`, `body`,
  `2 calls · error branch`, `new · exported`, `240 lines`, `nesting 6`, `moved 98 %`, `86 residual lines`,
  `no test files changed next to it`, `CI config`, `not analyzed · JS`.

## 7. Navigator

Segmented control `Files N · Attention N`. Filter box (`/`) applies to both modes. One **Roles**
button opens a menu: presets first, then all 11 roles with counts (checkbox, OR), then the two saved
filters. When a filter is active the button reads the preset name or role names with a clear ✕ and the
sidebar footer says “113 of 385 files · Review code · show all”. Overview clicks (legend rows, facts)
set the same filter.

**Files mode** — real tree (reuse `okena_files::file_tree` helpers): directory rows show file count and
summed +/− of the visible subset; single-child chains joined; under ~40 files everything is expanded,
above that top-level directories collapsed; `flatten` shows a plain list; virtualized. File rows: icon,
name (rename rows `…/old.ts → …/new.ts` keeping basenames, full paths on hover), at most two reason
markers (`sig N`, `calls`, `removed`, `new`, `moved N %`), +/− right-aligned; role badge only when the
role is not implementation; directory-level `no tests` marker; **not-analyzed files are dimmed** (not
badged), reason on hover. Selection highlight must actually paint (accent left border + selection bg).

**Attention mode** — the full ordered list (Start here is its top). Reason chips at the top act as OR
filters (`sig 12`, `removed 14`, `calls 18`, `new 89`, `no tests 2`, `git facts 24`, `tests` toggle).
Tier separators (`CONTRACT`, `BEHAVIOUR`, `VOLUME`, `GIT FACTS`, `REST`). Two-line rows: kind glyph +
name + churn; then reason chips + path. Footer toggles *ordered list* ↔ *group by file*. Directory and
file items sit in the same list with a different glyph.

Kind glyphs: `ƒ` function, `m` method, `C` class/struct, `T` type/interface/enum, `M` module, `≡` file, `▸` directory.

Switching modes keeps the selection: a symbol selected in Attention highlights its file in Files; a
file selected in Files scrolls Attention to its first item.

## 8. Overview

Two blocks, then nothing else.

**Change at a glance** — headline `Implementation 15 692 lines · 45 % of 34 640 · 97 files` (hint:
“changed lines = added + deleted”); stacked bar by role; legend rows (swatch, role, files, lines, %),
clickable → role filter. For binary-only / no line totals the headline uses file counts; for
deletion-heavy comparisons the sign is visible.

Facts (one line each, omitted when empty, never zeros):
- **Public API** — `≥ 3 removed · ≥ 12 signatures changed · ≥ 34 added — analyzed subset, TS/TSX → Attention`
  (`≥` only when coverage is partial; “no supported language in this comparison” when applicable).
- **Tests** — `Test files changed next to 4 of 6 implementation directories · none next to packages/workers/src (26 files, +6 118) → show`.
- **Moves** — `21 high-similarity moves · 17 likely mechanical (≤ 20 residual lines) · 4 with edits, ranked below → filter`.
- **Commits** — `14 · 1 merge · <author> · 6 days · <first sha> … <last sha> → show ledger` (ledger: relative
  dates via `okena_git::format_relative_time`, SHA, subject, author). “Open commit diff” per row needs an
  app-level overlay request and is deferred with the backend items.
- **Also** — `2 lockfiles · 1 submodule pointer · 3 binary files → show`.

**Start here** — header `Start here · one ordered list · every row names its reasons`, right link
`all N → Attention`, caveat line under it: `structure reached 63 of 97 implementation files (first 200 in
path order) — the rest ranked from git facts` (only when partial). Ten rows: index, kind glyph, name +
dimmed path, reason chips, +/−. Unanalyzed rows dimmed. Footer sentence: tiers and `]` steps through it.

While structure is loading, git-fact rows are shown immediately (no skeleton); symbol rows are inserted
when structure lands. If structure fails, the list stays and the pill turns red.

## 9. File view

Opens on file / symbol / directory selection (a directory item opens its first ranked file and expands the
directory in the tree).

- **File header** (40 px): path with directory dimmed; role badge (hover: the classifying rule in words);
  status; +/−; the file's reason chips; language + parse status; `outline` link (popover with base and
  head outlines from `StructuredFile`); queue position `3 of 236` with ‹ › (previous / next in Attention
  order). Renames: `old → new · moved 98 %`. Unsupported / unanalyzed: `JavaScript · not analyzed`, no
  symbol bar, plain diff, git-fact reasons still shown. Binary: a binary state instead of a diff.
- **Symbol bar** (32 px, sticky at the top of the diff, only when the file has changed symbols): follows
  the changed symbol currently in view (hunk → symbol via `SymbolChange.hunks`; deepest enclosing
  changed symbol wins; on explicit selection it shows the selected symbol). Kind glyph, name, reason
  chips, +/− within the symbol, `changed symbol 1 of 4 · } next`, `▸ details` toggle. Collapsed by
  default; `d` or click expands; state remembered per session.
- **Details** (expanded): *Signature (normalized)* block only when `signature_change` exists — old line
  with `−`, new line with `+`, the differing span highlighted (token diff of the two normalized strings).
  *Calls changed in this function — same file, syntactic; callers are not tracked* only when `call_diff`
  is non-empty: `+ callee(args)`, `− callee(args)`, `~ callee(old) → (new)`, control-context stack as a
  muted suffix (“in condition”, “in error branch”). Complexity metrics only if they are a reason.
- **Persistent marker**: the selected symbol's hunks keep a left accent marker until another symbol is
  selected (replace the 2 s auto-clearing highlight in `review_nav`).
- Diff pane itself is unchanged (unified / split, search, selection, context menus).

## 10. Analysis status

Header pill states: `Loading inventory…` (spinner) · `Analyzing structure…` (spinner, no count — the
request is one-shot) · `Structure ready · 385 files · TS, TSX, Rust` (green) · `Structure limited · 200
of 385 files · details` (amber) · `Structure ready · 3 files failed to parse · details` (amber) ·
`Structure unavailable · diff still works` (red). Details popover: one row per omission group in words
with counts and languages/extensions, failed-parse stage + message, resolved OIDs, and the sentence “Not
analyzed files stay in the tree (dimmed), open as a plain diff, and are ranked from git facts.”

## 11. Keyboard

Two regions (navigator, content). `Tab` traverses controls inside a region; `F6` / `Ctrl+1` / `Ctrl+2`
switch region. Single-letter shortcuts are inert while a text field has focus. `1`/`2` are swallowed by
the overlay. All bindings rebindable; `?` shows the map. Footer hints only for keys that work on the
current screen.

| Key | Action |
|---|---|
| `↑` `↓` | move in navigator (list keeps selection visible; moving the selection opens the row in the content area, `↵` additionally moves focus to content) |
| `←` `→` `Space` `Home` `End` | collapse / expand / toggle tree node; jump |
| `1` `2` | navigator mode Files / Attention |
| `/` | focus filter box (`Esc` clears and returns) |
| `r` | Roles menu |
| `o` | Overview |
| `]` `[` | next / previous item in Attention order (from any file; keeps queue position) |
| `}` `{` | next / previous changed symbol in the open file |
| `Alt+↓` `Alt+↑` | next / previous hunk |
| `d` | expand / collapse symbol details |
| `s` `w` | split / unified · ignore whitespace (content focused, as today) |
| `Ctrl+F` `n` `N` | find in the displayed diff, next / previous match; on the Overview `Ctrl+F` focuses the navigator filter |
| `y` | copy `path:line` of the current symbol / hunk |
| `Ctrl+C` | copy diff selection, or the selected navigator row (path / qualified symbol) when the navigator is focused |
| `?` | shortcut help |
| `Esc` | close find → back to Overview → close the review; never closes from inside an input without clearing it first |

## 12. Edge cases

| Case | Behaviour |
|---|---|
| ≤ 10 files or ≤ 500 changed lines | open on the first ranked file with a one-line summary in the file header; Overview on `o`; navigator fully expanded |
| 2 000 files | tree virtualized, top-level directories collapsed, flatten available; Attention built from git facts for all files; caveat states structure reach |
| all-unsupported languages | Public API fact says “no supported language in this comparison”; Start here = git-fact ranking; nothing empty |
| structure fails after inventory | pill red; git-fact rows stay; symbol markers / symbol bar / details absent all at once; diff unaffected |
| binary-only / deletion-heavy | headline in file counts / sign visible; binary files get a binary state |
| single commit target | header subtitle = commit subject; Commits fact hidden; merge-base hidden |
| content width < 1 000 px | Overview one column; file header drops language and role labels behind the badge tooltip; symbol bar keeps name + first two chips |
| whitespace toggle | reloads diff + structure only; header +/− tagged “ignoring whitespace” |

## 13. Data mapping

Client-side from existing types: volume by role, directory aggregation, tests fact, moves split, Public
API counts (`SymbolChange` + `visibility()`), tiers and reasons (`SymbolChange`, `CallDiffChange`,
hotspots ∩ `SymbolChange`, inventory facts), signature token diff (normalized strings), symbol bar
following the viewport (`SymbolChange.hunks`), outline popover (`StructuredFile.old_outline /
new_outline`), commits summary (`ReviewCommitFact.parent_oids`, `timestamp` + `format_relative_time`),
status pill / popover (`ReviewCoverage`, `OmittedFileGroup`, `LanguageCoverage`, `AnalysisError`,
`StructuredFile.status`), rule id → label map (11 rules).

Backend (out of scope here, tracked separately): per-commit file list (Commits navigator mode), analysis
selection policy (implementation + churn first instead of path order), stale-target check, file lists per
omission group (“show files”), progress events, retry action.

## 14. Removed from the current UI

Lens tab bar · both stat strips and per-lens summary strips · the unlabeled selected-path line · the
flat section stream · Structure tab's Outline / File errors / Language coverage / Aggregate omissions /
Aggregate errors sections · CallDiff tab · raw rule ids, provenance words, Debug enum names, unix epochs
in text · footer hints for keys that do nothing on the current screen.
