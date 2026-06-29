import { useCallback, useEffect, useState } from "react";
import { postAction } from "../api/client";
import type { ApiGitStatus, ApiProject, CiCheck, CiStatus, FileDiffSummary, PrState } from "../api/types";

const CHIP_CLASS = "inline-flex h-5 items-center gap-1 rounded-[3px] px-1.5 text-[11px] text-[var(--ok-text-secondary)] hover:bg-[var(--ok-hover)] hover:text-[var(--ok-text)]";

type LoadState =
  | { status: "idle"; files: FileDiffSummary[] }
  | { status: "loading"; files: FileDiffSummary[] }
  | { status: "error"; files: FileDiffSummary[]; message: string };

export function GitPanel({ project }: { project: ApiProject }) {
  const git = project.git_status ?? null;
  const [loadState, setLoadState] = useState<LoadState>({ status: "idle", files: [] });
  const [detailsOpen, setDetailsOpen] = useState(false);

  const hasChanges = Boolean(git && (git.lines_added > 0 || git.lines_removed > 0));

  const loadSummary = useCallback(async () => {
    if (!git) return;
    setLoadState((current) => ({ status: "loading", files: current.files }));
    try {
      const payload = await postAction({ action: "git_diff_summary", project_id: project.id });
      setLoadState({ status: "idle", files: parseFileDiffSummaries(payload) });
    } catch (error) {
      setLoadState({
        status: "error",
        files: [],
        message: error instanceof Error ? error.message : "Failed to load git changes",
      });
    }
  }, [project.id, git]);

  useEffect(() => {
    setLoadState({ status: "idle", files: [] });
    setDetailsOpen(false);
  }, [project.id]);

  useEffect(() => {
    if (hasChanges) {
      loadSummary();
    }
  }, [hasChanges, loadSummary]);

  if (!git || !git.branch) {
    return (
      <div className="soft-rule border-b bg-[var(--ok-panel)] px-3 py-2 text-[11px] text-[var(--ok-text-muted)]">
        git unavailable
      </div>
    );
  }

  return (
    <section className="panel-rule relative border-b bg-[var(--ok-panel)]">
      <div className="flex min-h-[32px] items-center gap-1 overflow-x-auto px-3 py-1.5">
        <button className={CHIP_CLASS} onClick={() => setDetailsOpen((open) => !open)} title="Git status">
          <span className="text-[var(--ok-text-muted)]">branch</span>
          <span className="max-w-32 truncate text-[var(--ok-text)]">{git.branch}</span>
        </button>

        {git.pr_info && (
          <a
            className={CHIP_CLASS}
            href={git.pr_info.url}
            target="_blank"
            rel="noreferrer"
            title={`PR #${git.pr_info.number} ${prStateLabel(git.pr_info.state)}`}
          >
            <span className={prStateClass(git.pr_info.state)}>pr</span>
            <span>#{git.pr_info.number}</span>
          </a>
        )}

        {git.ci_checks && (
          <button className={CHIP_CLASS} onClick={() => setDetailsOpen((open) => !open)} title={ciTooltip(git.ci_checks)}>
            <span className={ciClassName(git.ci_checks.status)}>{ciStatusLabel(git.ci_checks.status)}</span>
            <span>{git.ci_checks.passed}/{git.ci_checks.total}</span>
          </button>
        )}

        {hasChanges && (
          <button className={CHIP_CLASS} onClick={() => setDetailsOpen((open) => !open)} title="Working tree changes">
            {git.lines_added > 0 && <span className="text-[var(--ok-green)]">+{git.lines_added}</span>}
            {git.lines_removed > 0 && <span className="text-[var(--ok-red)]">-{git.lines_removed}</span>}
          </button>
        )}

        {aheadBehindParts(git).map((part) => (
          <button
            key={part.key}
            className={CHIP_CLASS}
            onClick={() => setDetailsOpen((open) => !open)}
            title={part.title}
          >
            <span className={part.className}>{part.label}</span>
          </button>
        ))}

        <button
          className={`${CHIP_CLASS} ml-auto`}
          onClick={() => {
            if (!detailsOpen && hasChanges && loadState.files.length === 0) {
              loadSummary();
            }
            setDetailsOpen((open) => !open);
          }}
        >
          details
        </button>
      </div>

      {detailsOpen && (
        <GitStatusPopover
          git={git}
          loadState={loadState}
          onRefresh={loadSummary}
          onClose={() => setDetailsOpen(false)}
        />
      )}
    </section>
  );
}

function GitStatusPopover({
  git,
  loadState,
  onRefresh,
  onClose,
}: {
  git: ApiGitStatus;
  loadState: LoadState;
  onRefresh: () => void;
  onClose: () => void;
}) {
  const checks = git.ci_checks?.checks ?? [];

  return (
    <div className="absolute left-3 right-3 top-full z-40 mt-1 max-h-[520px] overflow-hidden border border-[var(--ok-border)] bg-[var(--ok-panel)]">
      <div className="project-header flex min-h-[34px] items-center gap-2 border-b px-3">
        <span className="text-[11px] text-[var(--ok-text-muted)]">git status</span>
        <span className="min-w-0 truncate text-[12px] text-[var(--ok-text)]">{git.branch ?? "detached"}</span>
        <button className="icon-button ml-auto" onClick={onClose} aria-label="Close git status">
          x
        </button>
      </div>

      <div className="max-h-[486px] overflow-auto px-3 py-3">
        <div className="grid grid-cols-2 gap-2 text-[11px]">
          <Metric label="added" value={`+${git.lines_added}`} valueClassName="text-[var(--ok-green)]" />
          <Metric label="removed" value={`-${git.lines_removed}`} valueClassName="text-[var(--ok-red)]" />
          <Metric label="ahead" value={`${git.ahead ?? 0}`} valueClassName="text-[var(--ok-green)]" />
          <Metric label="behind" value={`${git.behind ?? 0}`} valueClassName="text-[var(--ok-yellow)]" />
          <Metric label="unpushed" value={`${git.unpushed ?? 0}`} valueClassName="text-[var(--ok-blue)]" />
          <Metric label="base" value={git.review_base ?? "-"} valueClassName="text-[var(--ok-text)]" />
        </div>

        {git.pr_info && (
          <section className="soft-rule mt-3 border-t pt-3">
            <div className="mb-2 text-[10px] text-[var(--ok-text-muted)]">pull request</div>
            <a
              href={git.pr_info.url}
              target="_blank"
              rel="noreferrer"
              className="flex items-center gap-2 rounded-[3px] px-1 py-1 text-[11px] hover:bg-[var(--ok-hover)]"
            >
              <span className={prStateClass(git.pr_info.state)}>{prStateLabel(git.pr_info.state)}</span>
              <span className="text-[var(--ok-text)]">#{git.pr_info.number}</span>
              <span className="ml-auto text-[var(--ok-text-muted)]">open</span>
            </a>
          </section>
        )}

        {git.ci_checks && (
          <section className="soft-rule mt-3 border-t pt-3">
            <div className="mb-2 flex items-center gap-2 text-[10px] text-[var(--ok-text-muted)]">
              <span>checks</span>
              <span className={ciClassName(git.ci_checks.status)}>{ciTooltip(git.ci_checks)}</span>
            </div>
            {checks.length === 0 ? (
              <div className="text-[11px] text-[var(--ok-text-muted)]">No checks reported</div>
            ) : (
              <div className="space-y-0.5">
                {checks.map((check) => (
                  <CiRow key={`${check.workflow ?? "check"}:${check.name}`} check={check} />
                ))}
              </div>
            )}
          </section>
        )}

        <section className="soft-rule mt-3 border-t pt-3">
          <div className="mb-2 flex items-center gap-2 text-[10px] text-[var(--ok-text-muted)]">
            <span>diff summary</span>
            <button
              className="ml-auto rounded-[3px] px-1.5 py-0.5 hover:bg-[var(--ok-hover)] hover:text-[var(--ok-text)]"
              onClick={onRefresh}
              disabled={loadState.status === "loading"}
            >
              {loadState.status === "loading" ? "loading" : "refresh"}
            </button>
          </div>
          <DiffSummary state={loadState} />
        </section>
      </div>
    </div>
  );
}

function Metric({
  label,
  value,
  valueClassName,
}: {
  label: string;
  value: string;
  valueClassName: string;
}) {
  return (
    <div className="border border-[var(--ok-border-soft)] bg-[var(--ok-terminal-muted)] px-2 py-1.5">
      <div className="text-[10px] text-[var(--ok-text-muted)]">{label}</div>
      <div className={`mt-0.5 truncate text-[12px] font-bold ${valueClassName}`}>{value}</div>
    </div>
  );
}

function CiRow({ check }: { check: CiCheck }) {
  const content = (
    <>
      <span className={check.is_skipped ? "text-[var(--ok-text-muted)]" : ciClassName(check.status)}>
        {check.is_skipped ? "skip" : ciStatusLabel(check.status)}
      </span>
      <span className="min-w-0 flex-1 truncate text-[var(--ok-text)]">{check.name}</span>
      {check.workflow && <span className="hidden max-w-28 truncate text-[var(--ok-text-muted)] md:inline">{check.workflow}</span>}
      <span className="text-[var(--ok-text-muted)]">{formatElapsed(check.elapsed_ms)}</span>
    </>
  );

  if (check.link) {
    return (
      <a
        href={check.link}
        target="_blank"
        rel="noreferrer"
        title={check.description}
        className="flex items-center gap-2 rounded-[3px] px-1 py-1 text-[11px] hover:bg-[var(--ok-hover)]"
      >
        {content}
      </a>
    );
  }

  return (
    <div title={check.description} className="flex items-center gap-2 px-1 py-1 text-[11px]">
      {content}
    </div>
  );
}

function DiffSummary({ state }: { state: LoadState }) {
  if (state.status === "error") {
    return <div className="text-[11px] text-[var(--ok-red)]">{state.message}</div>;
  }
  if (state.files.length === 0) {
    return (
      <div className="text-[11px] text-[var(--ok-text-muted)]">
        {state.status === "loading" ? "Loading..." : "No file changes"}
      </div>
    );
  }

  return (
    <div className="grid grid-cols-[minmax(0,1fr)_auto_auto_auto] gap-x-3 gap-y-1 text-[11px]">
      {state.files.slice(0, 20).map((file) => (
        <FileRow key={file.path} file={file} />
      ))}
      {state.files.length > 20 && (
        <div className="col-span-4 text-[var(--ok-text-muted)]">+{state.files.length - 20} more files</div>
      )}
    </div>
  );
}

function FileRow({ file }: { file: FileDiffSummary }) {
  return (
    <>
      <div className="truncate text-[var(--ok-text-secondary)]">{file.path}</div>
      <div className="text-right text-[var(--ok-green)]">+{file.added}</div>
      <div className="text-right text-[var(--ok-red)]">-{file.removed}</div>
      <div className="text-right text-[var(--ok-text-muted)]">{file.is_new ? "new" : ""}</div>
    </>
  );
}

function aheadBehindParts(git: ApiGitStatus): Array<{ key: string; label: string; className: string; title: string }> {
  const ahead = git.ahead ?? 0;
  const behind = git.behind ?? 0;
  const unpushed = git.unpushed ?? 0;
  const base = git.review_base ?? "base";
  const parts: Array<{ key: string; label: string; className: string; title: string }> = [];

  if (ahead > 0) {
    parts.push({
      key: "ahead",
      label: `up ${ahead}`,
      className: "text-[var(--ok-green)]",
      title: `${ahead} commit${ahead === 1 ? "" : "s"} ahead of ${base}`,
    });
  }
  if (behind > 0) {
    parts.push({
      key: "behind",
      label: `down ${behind}`,
      className: "text-[var(--ok-yellow)]",
      title: `${behind} commit${behind === 1 ? "" : "s"} behind ${base}`,
    });
  }
  if (unpushed > 0 && git.unpushed !== git.ahead) {
    parts.push({
      key: "unpushed",
      label: `push ${unpushed}`,
      className: "text-[var(--ok-blue)]",
      title: `${unpushed} commit${unpushed === 1 ? "" : "s"} not pushed to origin/<branch>`,
    });
  }

  return parts;
}

function ciTooltip(checks: { status: CiStatus; passed: number; failed: number; pending: number; total: number }): string {
  switch (checks.status) {
    case "Success":
      return `${checks.passed}/${checks.total} checks passed`;
    case "Failure":
      return `${checks.failed} failed, ${checks.passed} passed of ${checks.total}`;
    case "Pending":
      return `${checks.pending} pending, ${checks.passed} passed of ${checks.total}`;
  }
}

function ciStatusLabel(status: CiStatus): string {
  switch (status) {
    case "Success":
      return "ok";
    case "Failure":
      return "fail";
    case "Pending":
      return "run";
  }
}

function ciClassName(status: CiStatus): string {
  switch (status) {
    case "Success":
      return "text-[var(--ok-green)]";
    case "Failure":
      return "text-[var(--ok-red)]";
    case "Pending":
      return "text-[var(--ok-yellow)]";
  }
}

function prStateLabel(state: PrState): string {
  switch (state) {
    case "Open":
      return "Open";
    case "Draft":
      return "Draft";
    case "Merged":
      return "Merged";
    case "Closed":
      return "Closed";
  }
}

function prStateClass(state: PrState): string {
  switch (state) {
    case "Open":
      return "text-[var(--ok-green)]";
    case "Draft":
      return "text-[var(--ok-text-muted)]";
    case "Merged":
      return "text-[#c678dd]";
    case "Closed":
      return "text-[var(--ok-red)]";
  }
}

function formatElapsed(ms: number | undefined): string {
  if (!ms) return "-";
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  return `${Math.floor(seconds / 60)}m${seconds % 60}s`;
}

function parseFileDiffSummaries(payload: unknown): FileDiffSummary[] {
  if (!Array.isArray(payload)) return [];
  return payload.flatMap((item) => {
    if (!isRecord(item)) return [];
    const path = item.path;
    const added = item.added;
    const removed = item.removed;
    const isNew = item.is_new;
    if (typeof path !== "string" || typeof added !== "number" || typeof removed !== "number" || typeof isNew !== "boolean") {
      return [];
    }
    return [{ path, added, removed, is_new: isNew }];
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
