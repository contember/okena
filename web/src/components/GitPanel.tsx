import { useCallback, useEffect, useMemo, useState } from "react";
import { postAction } from "../api/client";
import type { ApiGitStatus, ApiProject, FileDiffSummary } from "../api/types";

type LoadState =
  | { status: "idle"; files: FileDiffSummary[] }
  | { status: "loading"; files: FileDiffSummary[] }
  | { status: "error"; files: FileDiffSummary[]; message: string };

export function GitPanel({ project }: { project: ApiProject }) {
  const git = project.git_status ?? null;
  const [loadState, setLoadState] = useState<LoadState>({ status: "idle", files: [] });

  const hasChanges = Boolean(git && (git.lines_added > 0 || git.lines_removed > 0));
  const summaryLabel = useMemo(() => formatStatusSummary(git), [git]);

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
  }, [project.id]);

  useEffect(() => {
    if (hasChanges) {
      loadSummary();
    }
  }, [hasChanges, loadSummary]);

  if (!git) {
    return (
      <div className="border-b border-zinc-800 bg-zinc-950 px-3 py-2 text-xs text-zinc-600">
        git unavailable
      </div>
    );
  }

  return (
    <section className="border-b border-zinc-800 bg-zinc-950">
      <div className="flex items-center gap-3 px-3 py-2 text-xs">
        <div className="min-w-0 flex items-center gap-2">
          <span className="text-zinc-600">git</span>
          <span className="truncate text-zinc-200">{git.branch ?? "detached"}</span>
          {git.pr_info && (
            <a
              className="text-zinc-500 hover:text-zinc-200"
              href={git.pr_info.url}
              target="_blank"
              rel="noreferrer"
            >
              PR #{git.pr_info.number}
            </a>
          )}
          {git.ci_checks && <span className={ciClassName(git.ci_checks.status)}>{ciLabel(git)}</span>}
        </div>

        <div className="ml-auto flex items-center gap-2 text-zinc-500">
          <span>{summaryLabel}</span>
          {git.review_base && <span className="hidden md:inline">base {git.review_base}</span>}
          <button
            onClick={loadSummary}
            className="px-1.5 py-0.5 text-zinc-500 hover:bg-zinc-800 hover:text-zinc-200"
            disabled={loadState.status === "loading"}
          >
            {loadState.status === "loading" ? "loading" : "refresh"}
          </button>
        </div>
      </div>

      {(loadState.files.length > 0 || loadState.status === "error") && (
        <div className="max-h-32 overflow-auto border-t border-zinc-900 px-3 py-1.5">
          {loadState.status === "error" ? (
            <div className="text-xs text-red-400">{loadState.message}</div>
          ) : (
            <div className="grid grid-cols-[minmax(0,1fr)_auto_auto_auto] gap-x-3 gap-y-1 text-xs">
              {loadState.files.slice(0, 12).map((file) => (
                <FileRow key={file.path} file={file} />
              ))}
              {loadState.files.length > 12 && (
                <div className="col-span-4 text-zinc-600">+{loadState.files.length - 12} more files</div>
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function FileRow({ file }: { file: FileDiffSummary }) {
  return (
    <>
      <div className="truncate text-zinc-400">{file.path}</div>
      <div className="text-right text-green-500">+{file.added}</div>
      <div className="text-right text-red-400">-{file.removed}</div>
      <div className="text-right text-zinc-600">{file.is_new ? "new" : ""}</div>
    </>
  );
}

function formatStatusSummary(git: ApiGitStatus | null): string {
  if (!git) return "";
  const parts = [`+${git.lines_added}`, `-${git.lines_removed}`];
  if (git.ahead != null || git.behind != null) {
    parts.push(`ahead ${git.ahead ?? 0}`);
    parts.push(`behind ${git.behind ?? 0}`);
  }
  if (git.unpushed != null) {
    parts.push(`unpushed ${git.unpushed}`);
  }
  return parts.join(" / ");
}

function ciLabel(git: ApiGitStatus): string {
  const checks = git.ci_checks;
  if (!checks) return "";
  if (checks.total === 0) return checks.status.toLowerCase();
  return `${checks.status.toLowerCase()} ${checks.passed}/${checks.total}`;
}

function ciClassName(status: string): string {
  switch (status) {
    case "Success":
      return "text-green-500";
    case "Failure":
      return "text-red-400";
    default:
      return "text-yellow-500";
  }
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
