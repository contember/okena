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

export function FilesPanel({ project }: { project: ApiProject }) {
  const [directory, setDirectory] = useState("");
  const [directoryState, setDirectoryState] = useState<DirectoryState>({ status: "idle", entries: [] });
  const [fileState, setFileState] = useState<FileState>({ status: "empty" });
  const [showIgnored, setShowIgnored] = useState(false);

  const sortedEntries = useMemo(() => directoryState.entries, [directoryState.entries]);

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
        const content = parseFileContent(payload);
        setFileState({ status: "loaded", path, content });
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
    setDirectory("");
    setFileState({ status: "empty" });
  }, [project.id]);

  useEffect(() => {
    loadDirectory();
  }, [loadDirectory]);

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
    <aside className="flex h-full w-80 flex-shrink-0 flex-col border-l border-zinc-800 bg-zinc-950">
      <div className="flex items-center gap-2 border-b border-zinc-800 px-3 py-2 text-xs">
        <span className="text-zinc-600">files</span>
        <span className="min-w-0 flex-1 truncate text-zinc-300">{directory || "."}</span>
        <button
          onClick={goUp}
          disabled={!directory}
          className="text-zinc-600 hover:text-zinc-300 disabled:opacity-30"
        >
          up
        </button>
        <button
          onClick={loadDirectory}
          className="text-zinc-600 hover:text-zinc-300"
          disabled={directoryState.status === "loading"}
        >
          {directoryState.status === "loading" ? "loading" : "refresh"}
        </button>
      </div>

      <label className="flex items-center gap-2 border-b border-zinc-900 px-3 py-1.5 text-xs text-zinc-500">
        <input
          type="checkbox"
          checked={showIgnored}
          onChange={(event) => setShowIgnored(event.currentTarget.checked)}
          className="h-3 w-3"
        />
        show ignored
      </label>

      <div className="h-44 flex-shrink-0 overflow-auto border-b border-zinc-800 px-2 py-1">
        {directoryState.status === "error" ? (
          <div className="px-1 py-1 text-xs text-red-400">{directoryState.message}</div>
        ) : sortedEntries.length === 0 ? (
          <div className="px-1 py-1 text-xs text-zinc-600">
            {directoryState.status === "loading" ? "Loading..." : "Empty directory"}
          </div>
        ) : (
          <div className="space-y-0.5">
            {sortedEntries.map((entry) => (
              <button
                key={`${entry.is_dir ? "d" : "f"}:${entry.name}`}
                onClick={() => openEntry(entry)}
                className="grid w-full grid-cols-[1rem_minmax(0,1fr)] gap-1 px-1 py-0.5 text-left text-xs text-zinc-500 hover:bg-zinc-900 hover:text-zinc-200"
              >
                <span className="text-zinc-600">{entry.is_dir ? "/" : "-"}</span>
                <span className="truncate">{entry.name}</span>
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        <FilePreview state={fileState} />
      </div>
    </aside>
  );
}

function FilePreview({ state }: { state: FileState }) {
  if (state.status === "empty") {
    return <div className="px-3 py-2 text-xs text-zinc-600">Select a file</div>;
  }
  if (state.status === "loading") {
    return <div className="px-3 py-2 text-xs text-zinc-600">Loading {state.path}...</div>;
  }
  if (state.status === "error") {
    return (
      <div className="px-3 py-2 text-xs">
        <div className="truncate text-zinc-500">{state.path}</div>
        <div className="mt-2 text-red-400">{state.message}</div>
      </div>
    );
  }

  return (
    <div className="text-xs">
      <div className="sticky top-0 border-b border-zinc-900 bg-zinc-950 px-3 py-2 text-zinc-500">
        <span className="block truncate">{state.path}</span>
      </div>
      <pre className="whitespace-pre-wrap break-words px-3 py-2 font-mono leading-5 text-zinc-300">
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
