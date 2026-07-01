import { useCallback, useEffect, useMemo, useState } from "react";
import { postAction } from "../api/client";
import type { ApiProject, DirectoryEntry } from "../api/types";

type DirectoryState =
  | { status: "idle"; entries: DirectoryEntry[] }
  | { status: "loading"; entries: DirectoryEntry[] }
  | { status: "error"; entries: DirectoryEntry[]; message: string };

type FileState =
  | { status: "empty" }
  | { status: "loading"; path: string }
  | { status: "loaded"; path: string; content: string }
  | { status: "error"; path: string; message: string };

export function FileViewerModal({
  project,
  onClose,
}: {
  project: ApiProject;
  onClose: () => void;
}) {
  const [directory, setDirectory] = useState("");
  const [directoryState, setDirectoryState] = useState<DirectoryState>({ status: "idle", entries: [] });
  const [fileState, setFileState] = useState<FileState>({ status: "empty" });
  const [showIgnored, setShowIgnored] = useState(false);

  const sortedEntries = useMemo(() => {
    return [...directoryState.entries].sort((a, b) => {
      if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
  }, [directoryState.entries]);

  const loadDirectory = useCallback(async () => {
    setDirectoryState((current) => ({ status: "loading", entries: current.entries }));
    try {
      const payload = await postAction({
        action: "list_directory",
        project_id: project.id,
        relative_path: directory,
        show_ignored: showIgnored,
      });
      setDirectoryState({ status: "idle", entries: parseDirectoryEntries(payload) });
    } catch (error) {
      setDirectoryState({
        status: "error",
        entries: [],
        message: error instanceof Error ? error.message : "Failed to list directory",
      });
    }
  }, [project.id, directory, showIgnored]);

  const readFile = useCallback(
    async (path: string) => {
      setFileState({ status: "loading", path });
      try {
        const payload = await postAction({
          action: "read_file",
          project_id: project.id,
          relative_path: path,
        });
        setFileState({ status: "loaded", path, content: parseFileContent(payload) });
      } catch (error) {
        setFileState({
          status: "error",
          path,
          message: error instanceof Error ? error.message : "Failed to read file",
        });
      }
    },
    [project.id],
  );

  useEffect(() => {
    loadDirectory();
  }, [loadDirectory]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const openEntry = useCallback(
    (entry: DirectoryEntry) => {
      const nextPath = joinPath(directory, entry.name);
      if (entry.is_dir) {
        setDirectory(nextPath);
        setFileState({ status: "empty" });
      } else {
        readFile(nextPath);
      }
    },
    [directory, readFile],
  );

  const goUp = useCallback(() => {
    setDirectory(parentPath(directory));
    setFileState({ status: "empty" });
  }, [directory]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/55 px-4 py-5"
      onMouseDown={onClose}
    >
      <section
        className="flex h-[min(760px,92vh)] w-[min(1120px,96vw)] flex-col border border-[var(--ok-border)] bg-[var(--ok-panel)]"
        onMouseDown={(event) => event.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={`Files for ${project.name}`}
      >
        <header className="project-header flex min-h-[44px] items-center gap-3 border-b px-3">
          <div className="min-w-0 flex-1">
            <div className="truncate text-[13px] font-bold text-[var(--ok-text)]">{project.name}</div>
            <div className="mt-0.5 truncate text-[10px] text-[var(--ok-text-muted)]">
              {directory || "."}
            </div>
          </div>
          <label className="flex items-center gap-2 text-[11px] text-[var(--ok-text-secondary)]">
            <input
              type="checkbox"
              checked={showIgnored}
              onChange={(event) => setShowIgnored(event.currentTarget.checked)}
              className="h-3 w-3"
            />
            ignored
          </label>
          <button
            className="icon-button"
            onClick={goUp}
            disabled={!directory}
            title="Parent directory"
            aria-label="Parent directory"
          >
            ..
          </button>
          <button
            className="icon-button"
            onClick={loadDirectory}
            disabled={directoryState.status === "loading"}
            title="Refresh directory"
            aria-label="Refresh directory"
          >
            R
          </button>
          <button className="icon-button icon-button-danger" onClick={onClose} aria-label="Close file viewer">
            x
          </button>
        </header>

        <div className="grid min-h-0 flex-1 grid-cols-[300px_minmax(0,1fr)]">
          <aside className="min-h-0 border-r border-[var(--ok-border)] bg-[var(--ok-terminal-muted)]">
            <div className="soft-rule border-b px-3 py-2 text-[10px] text-[var(--ok-text-muted)]">
              directory
            </div>
            <div className="h-full overflow-auto px-2 py-2">
              <DirectoryListing
                state={directoryState}
                entries={sortedEntries}
                onOpenEntry={openEntry}
              />
            </div>
          </aside>

          <main className="min-w-0 overflow-hidden bg-[var(--ok-terminal)]">
            <FilePreview state={fileState} />
          </main>
        </div>
      </section>
    </div>
  );
}

function DirectoryListing({
  state,
  entries,
  onOpenEntry,
}: {
  state: DirectoryState;
  entries: DirectoryEntry[];
  onOpenEntry: (entry: DirectoryEntry) => void;
}) {
  if (state.status === "error") {
    return <div className="px-1 py-1 text-[11px] text-[var(--ok-red)]">{state.message}</div>;
  }
  if (entries.length === 0) {
    return (
      <div className="px-1 py-1 text-[11px] text-[var(--ok-text-muted)]">
        {state.status === "loading" ? "Loading..." : "Empty directory"}
      </div>
    );
  }

  return (
    <div className="space-y-0.5 pb-8">
      {entries.map((entry) => (
        <button
          key={`${entry.is_dir ? "d" : "f"}:${entry.name}`}
          onClick={() => onOpenEntry(entry)}
          className="grid w-full grid-cols-[1rem_minmax(0,1fr)] gap-1 rounded-[3px] px-1 py-1 text-left text-[11px] text-[var(--ok-text-secondary)] hover:bg-[var(--ok-hover)] hover:text-[var(--ok-text)]"
        >
          <span className="text-[var(--ok-text-muted)]">{entry.is_dir ? "/" : "-"}</span>
          <span className="truncate">{entry.name}</span>
        </button>
      ))}
    </div>
  );
}

function FilePreview({ state }: { state: FileState }) {
  if (state.status === "empty") {
    return (
      <div className="flex h-full items-center justify-center text-[12px] text-[var(--ok-text-muted)]">
        Select a file
      </div>
    );
  }
  if (state.status === "loading") {
    return <div className="px-4 py-3 text-[12px] text-[var(--ok-text-muted)]">Loading {state.path}...</div>;
  }
  if (state.status === "error") {
    return (
      <div className="px-4 py-3 text-[12px]">
        <div className="truncate text-[var(--ok-text-secondary)]">{state.path}</div>
        <div className="mt-2 text-[var(--ok-red)]">{state.message}</div>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col text-[12px]">
      <div className="soft-rule flex min-h-[34px] items-center border-b bg-[var(--ok-terminal-muted)] px-3 text-[var(--ok-text-secondary)]">
        <span className="truncate">{state.path}</span>
        <span className="ml-auto text-[10px] text-[var(--ok-text-muted)]">
          {state.content.split("\n").length} lines
        </span>
      </div>
      <pre className="min-h-0 flex-1 overflow-auto whitespace-pre px-4 py-3 font-mono leading-5 text-[var(--ok-text)]">
        {state.content}
      </pre>
    </div>
  );
}

function parseDirectoryEntries(payload: unknown): DirectoryEntry[] {
  if (!Array.isArray(payload)) return [];
  return payload.flatMap((item) => {
    if (!isRecord(item)) return [];
    const name = item.name;
    const isDir = item.is_dir;
    if (typeof name !== "string" || typeof isDir !== "boolean") return [];
    return [{ name, is_dir: isDir }];
  });
}

function parseFileContent(payload: unknown): string {
  if (!isRecord(payload)) return "";
  return typeof payload.content === "string" ? payload.content : "";
}

function joinPath(base: string, name: string): string {
  return base ? `${base}/${name}` : name;
}

function parentPath(path: string): string {
  const parts = path.split("/").filter(Boolean);
  parts.pop();
  return parts.join("/");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
