# Review Workspace Product Plan

## Status

- **Stage:** Product definition
- **Primary goal:** Help a developer understand what changed in a large pull request or local branch, decide where to spend attention, and send grounded feedback back to an agent.
- **Scope boundary:** Review and navigation only. Editing, refactoring, merging, and conflict resolution are out of scope.
- **Reference branch:** `pletivo/feat/workers-host` against `origin/main` — 14 commits, 385 changed files, 33,045 additions, and 3,365 removals.

## Executive summary

Okena should present a large change as a review workspace, not only as a flat file diff.
The workspace should answer four questions:

1. What is the shape of this change?
2. Which behavior and contracts changed?
3. What evidence supports those changes?
4. What has the reviewer checked, and what should be sent back to the agent?

The product should have three trust layers:

1. **Deterministic inventory** built from Git and visible local rules.
2. **Structured review** built from tree-sitter and language-specific extractors.
3. **AI-assisted interpretation** that groups facts into a change story, with every claim linked back to evidence.

The ordinary line diff remains the final source of detail. The new layers help the reviewer decide which diffs to open and why.

## Product decision

Build the review experience as progressive lenses over one exact comparison:

```text
Review target
    ↓
Deterministic inventory
    ↓
Structured code facts
    ↓
AI-assisted story
    ↓
Line diff + local notes + agent handoff
```

Do not make tree-sitter highlighting migration, a persistent workspace index, or a full semantic resolver prerequisites for the review product.

This approach is best when Okena must remain useful without AI, must explain where every displayed fact came from, and must support local and remote repositories consistently.

It would be the wrong approach if reviewers consistently cannot orient themselves without a resolved cross-file call graph. In that case, call resolution would need to move earlier than the proposed structured-review phases.

### Decision axes

The product options differ on these axes, in priority order:

1. **Trust and inspectability:** Can a reviewer verify why an item appears?
2. **Orientation value:** Does the view reduce the time needed to form a correct mental model?
3. **Graceful availability:** Does the review still work without AI, GitHub, or language support?
4. **Remote parity:** Can the same product work when the repository lives behind an Okena daemon?
5. **Expansion cost:** Can structure, CallDiff, search, and more languages be added without replacing the review model?
6. **Operational cost:** How much indexing, invalidation, caching, and protocol surface is required before users receive value?

### Alternatives considered

#### Deterministic inventory only

Build Git facts, path rules, commit chronology, and the ordinary diff, but no syntax or AI layers.

**Best when:** Okena primarily needs a trustworthy large-diff browser and users do not need symbol-level orientation.

This is the cheapest and most available option. It cannot answer which contracts, functions, or calls changed, so reviewers still reconstruct code shape manually.

#### Structure-first review without AI

Build deterministic inventory plus tree-sitter outline, signatures, hotspots, and CallDiff. Do not add inferred chapters or intent.

**Best when:** Review trust is more important than narrative orientation, or AI availability and latency cannot be assumed.

This option provides most code-review value and remains inspectable. It may still leave a large coherent branch feeling like several disconnected structural changes.

#### Layered review workspace — recommended

Build deterministic inventory first, structured review second, and AI-assisted interpretation as an optional final layer.

**Best when:** Okena wants the strongest orientation experience without making AI a source of code truth or a hard dependency.

This has the highest product ceiling and preserves graceful fallback. It costs more product and UI design because provenance, partial coverage, and transitions between lenses must remain coherent.

### Recommendation confidence and reversibility

**Confidence:** High for the layered product model; medium for the order of CallDiff and AI-assisted chapters.

The decision is reversible because each lens consumes versioned review facts and the ordinary diff remains the fallback. AI can be disabled, language adapters can be added independently, and persistent indexing can remain absent until measured demand justifies it.

The recommendation is wrong if representative reviewers cannot identify important review locations from Inventory, Structure, and signatures, and require cross-file call paths before those lenses become useful.

## The problem

Git answers which lines changed. It does not explain the role those lines play in the change.

The Pletivo reference branch illustrates the problem:

| File role | Files | Changed lines | Share |
|---|---:|---:|---:|
| Implementation | 97 | 16,506 | 45.3% |
| Tests | 155 | 12,294 | 33.8% |
| Fixtures and examples | 105 | 3,928 | 10.7% |
| Documentation | 17 | 3,100 | 8.5% |
| CI and configuration | 11 | 582 | 1.7% |

Less than half of the volume is implementation. Twenty-one files are detected renames with an average similarity of 98.3%. A flat file tree presents implementation, supporting evidence, documentation, generated content, and mechanical moves as equivalent work.

A reviewer currently has to reconstruct the product story manually by reading commit messages, opening large files, identifying public seams, locating tests, and remembering what has already been checked.

## Target user and job

The primary user is a senior developer opening a large branch or AI-generated pull request after the implementation work is complete.

The user needs to:

- form an accurate mental model quickly;
- separate behavior from supporting volume;
- find changed contracts and high-impact code;
- inspect the evidence for important claims;
- track review coverage;
- attach questions to stable code context;
- send a concise, grounded correction bundle back to an agent.

The desired feel is a dense forensic workbench: calm, explicit about uncertainty, and optimized for scanning rather than presentation.

## Review targets

A review target is a dynamic user choice that resolves to one exact comparison.

Supported targets should include:

- working tree against the index or `HEAD`;
- staged changes against `HEAD`;
- one commit against its parent;
- a local branch against a selected base;
- a remote branch against a selected base;
- a GitHub pull request resolved to exact base and head object IDs.

The UI may show friendly branch or PR names, but every opened review must record the exact object IDs used for analysis. A moved ref should mark the review stale; it must not silently change the comparison under the reviewer.

Three-dot branch and PR comparisons must use the actual merge-base snapshot consistently for both line and structured comparisons.

## Trust and provenance model

Deterministic does not always mean exact. The product must distinguish source and derivation instead of presenting one confidence score.

| Source class | Examples | UI treatment |
|---|---|---|
| **Git fact** | object IDs, paths, statuses, line counts, commits, rename similarity | Verified source stamp |
| **Rule-derived** | file role from a path pattern, Conventional Commit parsing | Heuristic source stamp with the matching rule |
| **Syntax-derived** | symbols, signatures, enclosing function, call expressions | Language and parser source stamp; partial coverage visible |
| **External context** | PR description, labels, checks, reviews | Shown verbatim as author or GitHub context |
| **AI-inferred** | chapters, intent, causal explanation, suggested review order | Distinct inferred marker and clickable evidence |
| **Reviewer-authored** | notes, reviewed state, manual grouping | Local user state |

The product must never collapse these classes into one opaque “risk” or “confidence” number.

## Product vocabulary

- **Review target:** The branch, commit, working tree, or PR being reviewed.
- **Resolved comparison:** Exact base, head, and merge-base identities used by all lenses.
- **Inventory:** Deterministic facts about files, commits, and volume.
- **File role:** Implementation, test, fixture, snapshot, example, documentation, generated, vendor, lockfile, configuration, or unclassified.
- **Structure:** Packages, modules, files, symbols, signatures, and syntactic relationships.
- **Change chapter:** An AI-inferred or reviewer-edited group of related changes.
- **Evidence:** Tests, fixtures, snapshots, documentation, checks, and measurements that support a behavior change.
- **Attention item:** An explainable reason to inspect a specific change. It is not a risk score.
- **Anchor:** The exact context attached to a note or review state.
- **Coverage:** What the system could analyze and what the reviewer has explicitly checked.

## Workspace layout

The review workspace should keep one selected location synchronized across all lenses.

### Persistent regions

- **Target header:** Comparison, resolved identities, stale state, commit and file totals.
- **Primary navigation:** Inventory, Structure, Diff, Evidence, Commits, and optional Story.
- **Context sidebar:** Files, symbols, or chapters depending on the active lens.
- **Review notebook:** Local annotations, review state, and the agent handoff bundle.

Switching lenses should preserve the selected file, symbol, chapter, and nearest hunk whenever possible.

## Lens 1: Deterministic inventory

This lens must work without tree-sitter, GitHub, or AI.

### Change facts

- exact base, head, and merge-base identities;
- commit count and chronological commit ledger;
- added, removed, modified, renamed, copied, binary, mode-only, and submodule changes;
- lines added and removed per file;
- rename similarity;
- largest additions, removals, and total churn;
- directory and package aggregation;
- Conventional Commit type and scope aggregation when messages match the format.

### File-role classification

Classify changed files using ordered, inspectable path rules:

- implementation;
- test;
- fixture;
- snapshot;
- example or playground;
- documentation;
- generated;
- vendored;
- lockfile;
- CI or configuration;
- unclassified.

The UI must show the matching rule and allow the reviewer to inspect classification coverage. A deterministic heuristic must not look like a Git fact.

### Mechanical-change handling

- collapse high-similarity renames into one move statement;
- expose residual edits inside moved files;
- group generated, vendor, snapshot, and lockfile changes without hiding them;
- show exact collapsed counts and keep them in review coverage;
- allow one-click expansion into the ordinary file diff.

### GitHub context

When the comparison maps to a GitHub pull request, add:

- PR title and description;
- author, labels, milestone, and linked issues;
- exact GitHub base and head object IDs;
- checks and their current state;
- requested and completed reviews;
- unresolved review threads;
- PR commit list.

GitHub text is author-provided context. It must not be presented as a verified statement about the code. A mismatch between GitHub object IDs and the analyzed repository must be visible.

## Lens 2: Structured review

This lens uses tree-sitter plus language-specific extraction and comparison rules. It is syntax-aware, not automatically semantic.

### Hierarchical outline

Present the change at multiple levels:

```text
package
  module
    file
      type / class / trait
        function / method
```

At every level show:

- added, removed, modified, and moved items;
- added and removed lines inside the item;
- related hunks;
- analysis coverage and parser errors;
- supporting files attached to the same scope where a deterministic relationship exists.

### Symbol changes

For supported languages, identify:

- added and removed symbols;
- modified symbol bodies;
- moved symbols when matching is unambiguous;
- changed visibility or export status;
- changed parameters, return types, generic parameters, bounds, and modifiers;
- changed fields, variants, properties, and implemented interfaces or traits;
- the enclosing symbol for every changed hunk.

Ambiguous matches must remain add/remove or explicitly ambiguous. The product must not invent stable symbol identity across revisions.

### Signature lens

Provide a compact list of changed signatures before the reviewer opens full diffs.

Each row should show:

- old and new signatures;
- the changed portion emphasized;
- public or private visibility;
- callers or references only when the available analysis can support the claim;
- direct navigation to the relevant hunk;
- related tests when a deterministic or clearly labeled heuristic relationship exists.

### Function and type hotspots

Support explainable, sortable structural metrics:

- largest new functions and methods;
- largest modified functions by total size;
- functions with the most changed lines;
- functions with the most parameters;
- deepest syntactic nesting;
- largest types by fields, variants, or methods;
- files and modules containing the most changed symbols;
- new or modified public surface;
- changed symbols without linked test evidence.

These are measurements, not risk scores. The UI should state the sorting metric directly.

### Structure-aware diff

Offer a structure-aware alternative to the line diff:

- unchanged symbols collapsed;
- symbols shown in source order or grouped by change kind;
- moved symbols separated from body edits;
- signature changes separated from implementation changes;
- comment-only and formatting-only changes labeled when detection is reliable;
- direct fallback to the exact line diff for every item.

The product should call this “structure-aware” or “syntax-aware” unless semantic name resolution is actually available.

## Lens 3: CallDiff and call-flow changes

CallDiff should help a reviewer understand how changed functions interact without claiming a complete call graph.

The product direction is inspired by [calldiff](https://github.com/tanishqkancharla/calldiff): compare expanded call trees across two Git states, allow a symbol or file to act as the entrypoint, preserve call-site locations, and keep machine-readable output suitable for agents. Okena should integrate the same class of information into the review workspace rather than expose it only as a separate command output.

### Per-function CallDiff

For a selected function or method, show:

- calls added to its body;
- calls removed from its body;
- unchanged calls whose arguments changed;
- constructor, method, macro, and callback registration calls when supported by the language adapter;
- the surrounding control context, such as a condition, loop, error branch, or callback;
- navigation to the call expression and enclosing diff hunk.

### Call-flow view

Present a bounded graph or linear flow centered on selected changed symbols:

- changed symbol as the root;
- outgoing calls before and after;
- incoming references only when resolved or explicitly labeled as textual matches;
- cross-file edges with provenance and confidence class;
- filters for changed-only, same-file, same-package, and resolved-only edges;
- cycle and fan-out visualization where useful.

Version one may be syntactic and intra-file. Cross-file claims must distinguish:

- resolved edge;
- probable textual edge;
- ambiguous edge;
- unresolved callee.

The UI must not label textual callee matching as a semantic call graph.

A later reachability action may show every known path from one selected symbol to another. As with the rest of CallDiff, unresolved dynamic calls and ambiguous dispatch must remain visible limitations.

## Lens 4: Evidence

Evidence should be visible as support for behavior, not as equivalent review volume.

### Evidence inventory

- tests and named test cases changed by the review;
- fixtures, snapshots, examples, and playgrounds;
- documentation and architecture notes;
- CI and packaging checks;
- GitHub checks and review state when available.

### Evidence relationships

Attach evidence to implementation using progressively weaker mechanisms:

1. explicit repository metadata;
2. same symbol or imported symbol;
3. test target or module relationship;
4. file-name and path convention;
5. same commit;
6. AI-inferred relationship.

The relationship source must remain visible. Unlinked evidence should remain in a separate group rather than being hidden.

### Reviewer workflow

- open an implementation symbol and its related evidence side by side;
- mark evidence as inspected independently from implementation;
- find changed implementation without evidence;
- find large evidence changes supporting only small behavior changes;
- collapse snapshots and fixtures while preserving their counts and review state.

## Lens 5: AI-assisted Change Spine

The Change Spine is an optional interpretation over deterministic and structured facts.

It should:

- group commits, files, and symbols into a small number of causal chapters;
- propose a concise intent for each chapter;
- distinguish behavior, contracts, evidence, and mechanical changes;
- suggest an explainable review order;
- show the facts supporting every claim;
- allow the reviewer to rename, split, merge, or reject chapters;
- preserve a deterministic fallback when AI is unavailable.

AI must not invent code facts. It may interpret observed facts and external context. Every inferred statement must be visually distinct and traceable to source files, symbols, commits, PR text, or documentation.

For the Pletivo reference branch, a useful proposed spine is:

1. Extract the host-independent runtime and core.
2. Establish the Worker rendering path.
3. Define isolation and deployable artifacts.
4. Build the live workspace and harden contracts.
5. Qualify and document the Workers host.

This grouping is a product hypothesis, not repository truth.

## Review state and annotations

Annotations are local user data in the first version.

### Supported anchors

- review target or chapter;
- file;
- symbol;
- signature change;
- CallDiff edge;
- hunk;
- free line range.

An anchor should retain path, comparison side, line hint, byte range where available, excerpt, and surrounding context. Re-anchoring after refresh must be best-effort and visible. An uncertain note becomes orphaned; it must never silently move.

### Review states

Keep these states separate:

- **Seen:** The reviewer opened the item.
- **Reviewed:** The reviewer explicitly completed it.
- **Noted:** One or more annotations are attached.
- **Stale:** The underlying comparison changed.

Review coverage should be based on explicit reviewed items, not scroll position or files opened.

### Agent handoff

“Send to agent” should produce a review bundle containing:

- reviewer notes;
- exact resolved comparison;
- anchor evidence and current snippet;
- relevant line diff;
- enclosing symbol and signature change;
- related CallDiff changes;
- related tests and fixtures;
- chapter intent when selected, labeled as inferred;
- provenance for all included facts.

The agent should receive enough context to act without repeating the entire repository scan.

## Search and navigation extensions

The syntax foundation can later support navigation outside the active review:

- workspace symbol search;
- jump to symbol;
- file outline and breadcrumbs;
- changed-symbol search;
- search by signature or symbol kind;
- navigation from a review item to unchanged surrounding code.

These capabilities should reuse normalized syntax facts, but they should not force a persistent workspace index into the first review release. Review analysis only needs the changed comparison plus explicitly opened context.

## Language coverage

The product must expose coverage rather than imply completeness.

The first useful language set should cover both primary dogfood repositories:

- Rust for Okena;
- TypeScript and TSX for Pletivo;
- JavaScript where it follows the same adapter;
- Astro evaluated separately based on changed-file coverage and grammar quality.

Every language adapter needs contract fixtures for:

- broken and incomplete syntax;
- Unicode byte ranges;
- nested symbols;
- overloads or duplicate names;
- signatures;
- imports and call expressions;
- macros or generated syntax where relevant;
- changed-symbol matching across two revisions.

Unsupported files remain fully available in Inventory and Diff.

## Syntax highlighting relationship

Tree-sitter highlighting is a separate migration.

The structured-review foundation should produce normalized source ranges and facts without GPUI colors or theme types. Highlighting may later reuse the same language registry and parse result, but structured review must not wait for the current syntect highlighting paths to move.

Syntect can remain the fallback for languages without structured support.

## Performance and failure behavior

The review workspace must remain useful on large changes.

- show deterministic inventory before structured analysis finishes;
- analyze changed files before unrelated repository files;
- run source and Git analysis where the repository lives;
- keep remote transfers to compact review facts where possible;
- use bounded file-size, file-count, parser-time, and query-capture budgets;
- support cancellation and discard stale generations;
- never delay the first ordinary diff paint for structured analysis;
- show parsed, pending, skipped, unsupported, and failed counts;
- keep the ordinary diff available after every analysis failure.

Partial output must be explicit. Empty structured output must not mean “no structural changes” when analysis was unsupported or incomplete.

## Product phases

### Phase 0 — Exact comparison contract

- resolve every review target to exact object IDs;
- make merge-base semantics consistent between displayed diff and source snapshots;
- detect stale branch and PR targets;
- return coverage and provenance with review results.

**User value:** The review is trustworthy and reproducible.

### Phase 1 — Deterministic inventory

- change totals and status inventory;
- commit ledger;
- path and package aggregation;
- rename detection and residual edits;
- file-role rules;
- mechanical and supporting-change groups;
- provenance ledger;
- optional GitHub PR context.

**User value:** A large branch becomes scannable without AI or language parsing.

### Phase 2 — Structured outline and signatures

- language capability reporting;
- Rust and TypeScript/TSX outlines;
- changed enclosing symbols;
- signature and public-surface changes;
- largest and most-changed functions and types;
- structure-aware navigation to line diffs.

**User value:** The reviewer can inspect contracts and code shape before reading files linearly.

### Phase 3 — Review notebook and evidence

- local annotations;
- Seen and Reviewed states;
- symbol, signature, hunk, and range anchors;
- evidence inventory and deterministic relationships;
- agent handoff bundle.

**User value:** Review becomes a stateful workflow rather than a sequence of file opens.

### Phase 4 — CallDiff

- per-function added and removed calls;
- argument and control-context changes;
- bounded intra-file call flow;
- explicitly classified cross-file edges where available.

**User value:** Reviewers can understand changed interactions without reconstructing every call manually.

### Phase 5 — AI-assisted Change Spine

- inferred chapters and intent;
- evidence-backed attention suggestions;
- reviewer-editable grouping;
- provenance-preserving summaries.

**User value:** Large coherent changes become a short navigable story without sacrificing access to facts.

### Phase 6 — Broader navigation

- workspace symbol search;
- breadcrumbs and outline outside the review;
- optional background workspace index if measured demand justifies it;
- additional languages based on observed coverage gaps.

**User value:** The review syntax foundation improves everyday code navigation.

## Explicit non-goals for the first release

- editing, refactoring, merging, or conflict resolution;
- automatic approval or merge recommendation;
- an opaque risk score;
- a complete semantic call graph;
- rename claims when symbol matching is ambiguous;
- persistent whole-workspace indexing;
- go-to-definition and references across every language;
- cross-device annotation sync;
- GitHub comment publishing;
- tree-sitter highlighting migration;
- stable symbol identity across arbitrary revisions;
- hiding unsupported or unparsed files.

## Success criteria

### Reference-branch outcomes

On the Pletivo reference branch, a reviewer should be able to:

- see that implementation is less than half of total line volume;
- recognize the runtime and core extraction as moves rather than unrelated churn;
- identify the largest Worker implementation files;
- inspect the commit progression without opening 385 files;
- find changed public seams and large functions in supported languages;
- inspect related tests and fixtures without letting them dominate navigation;
- attach a note to a symbol or hunk and send it with sufficient evidence to an agent.

### Quality gates

- deterministic values match Git for a fixture matrix of working tree, staged, commit, branch, rename, delete, binary, and shallow-history comparisons;
- every displayed fact has a provenance class;
- structured navigation lands on the correct symbol and hunk;
- parser failure and unsupported coverage are visible;
- no AI-generated claim appears without source evidence;
- a stale comparison cannot silently retain Reviewed state;
- ordinary diff remains usable when every optional analysis layer fails.

### Product validation

Dogfood the workspace on large AI-generated changes in Okena and Pletivo. Observe:

- which lens reviewers open first;
- whether Inventory or Structure provides the first useful mental model;
- which hotspot measurements lead to real findings;
- whether CallDiff changes review decisions;
- whether reviewers naturally anchor notes to chapters, symbols, signatures, calls, or hunks;
- how often AI chapters are accepted, edited, or rejected;
- which unsupported languages materially block review.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| AI summaries create false confidence | Keep deterministic fallback, inferred markers, and clickable evidence |
| Tree-sitter output is described as semantic | Use syntax-aware terminology and classify unresolved edges |
| Symbol matching invents moves or renames | Preserve ambiguity; fall back to add/remove |
| Large reviews block the UI | Background analysis, budgets, cancellation, progressive results |
| Remote repositories transfer too much source | Analyze where repository data lives and return compact facts |
| Path rules misclassify files | Visible rules, unclassified state, local override |
| GitHub context becomes stale | Compare exact object IDs and surface mismatch |
| Notes move to the wrong code after refresh | Evidence-based re-anchoring and explicit orphan state |
| Supporting files are hidden too aggressively | Always show collapsed counts and include them in coverage |
| Early workspace indexing creates operational burden | Analyze changed files on demand; defer persistent indexing |

## Decision-relevant unknowns

- Do reviewers orient faster from deterministic areas and commits, structured symbols, or AI chapters?
- Which structural metrics predict useful review attention without becoming noise?
- Is intra-file CallDiff sufficient for the first useful release?
- How reliable are TypeScript/TSX symbol and call matchers on representative Pletivo changes?
- Does Astro coverage materially limit the Pletivo review experience?
- Which annotation anchor is most natural in practice?
- How often does a GitHub PR description contain useful intent that commits and code do not?

These questions should be answered through dogfooding before expanding into a persistent index or full cross-file resolution.

## Recommended next product slice

Build one vertical review slice over an exact local branch comparison:

1. Deterministic Inventory with provenance.
2. Structured outline and signature changes for Rust and TypeScript/TSX.
3. Largest and most-changed function lists.
4. Direct navigation from every structured fact to the ordinary line diff.
5. Local notes anchored to a symbol or hunk.
6. Agent bundle export for selected notes.

Keep the AI Change Spine and cross-file CallDiff behind later experiments. The slice validates whether deterministic structure materially improves review before committing to broader indexing or AI orchestration.

The recommendation should be reconsidered if reviewers reach relevant symbols but still cannot understand impact without cross-file call paths. In that case, move a bounded resolved CallDiff experiment ahead of AI-assisted chapters.
