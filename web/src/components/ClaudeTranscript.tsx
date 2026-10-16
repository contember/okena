import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ApiClaudeEntry } from "../api/types";

type ToolResultEntry = Extract<ApiClaudeEntry, { kind: "tool_result" }>;
type ToolUseEntry = Extract<ApiClaudeEntry, { kind: "tool_use" }>;

/** Distance from the bottom (px) within which new entries keep the view pinned to the end. */
const STICK_THRESHOLD_PX = 40;

export function ClaudeTranscript({
  entries,
  placeholder,
}: {
  entries: ApiClaudeEntry[];
  placeholder: string | null;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);

  const { resultsByToolUse, attachedResults } = useMemo(() => groupToolResults(entries), [entries]);

  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (element && stickToBottom.current) {
      element.scrollTop = element.scrollHeight;
    }
  }, [entries]);

  const onScroll = () => {
    const element = scrollRef.current;
    if (!element) return;
    const distance = element.scrollHeight - element.scrollTop - element.clientHeight;
    stickToBottom.current = distance < STICK_THRESHOLD_PX;
  };

  return (
    <div ref={scrollRef} onScroll={onScroll} className="min-h-0 flex-1 overflow-y-auto px-3 py-2">
      {entries.length === 0 && placeholder && (
        <div className="py-6 text-center text-[11px] text-[var(--ok-text-muted)]">{placeholder}</div>
      )}
      <div className="flex flex-col gap-2">
        {entries.map((entry, index) => {
          const key = `${index}:${entry.uuid}`;
          switch (entry.kind) {
            case "user":
              return <UserBubble key={key} text={entry.text} viaChannel={entry.origin === "channel"} />;
            case "assistant":
              return (
                <div key={key} className="whitespace-pre-wrap break-words text-[12px] text-[var(--ok-text)]">
                  {entry.text}
                </div>
              );
            case "tool_use":
              return <ToolUseRow key={key} toolUse={entry} result={resultsByToolUse.get(entry.tool_use_id)} />;
            case "tool_result":
              return attachedResults.has(entry) ? null : <ToolResultBlock key={key} result={entry} />;
          }
        })}
      </div>
    </div>
  );
}

/** Pairs each tool result with its tool use; results whose use is not loaded stay standalone. */
function groupToolResults(entries: ApiClaudeEntry[]) {
  const toolUseIds = new Set<string>();
  for (const entry of entries) {
    if (entry.kind === "tool_use") toolUseIds.add(entry.tool_use_id);
  }
  const resultsByToolUse = new Map<string, ToolResultEntry>();
  const attachedResults = new Set<ApiClaudeEntry>();
  for (const entry of entries) {
    if (entry.kind === "tool_result" && toolUseIds.has(entry.tool_use_id)) {
      resultsByToolUse.set(entry.tool_use_id, entry);
      attachedResults.add(entry);
    }
  }
  return { resultsByToolUse, attachedResults };
}

function UserBubble({ text, viaChannel }: { text: string; viaChannel: boolean }) {
  return (
    <div className="flex flex-col items-end">
      <div className="max-w-[85%] whitespace-pre-wrap break-words rounded-[6px] bg-[var(--ok-selection)] px-2.5 py-1.5 text-[12px] text-white">
        {text}
      </div>
      {viaChannel && <span className="mt-0.5 text-[10px] text-[var(--ok-text-muted)]">via remote</span>}
    </div>
  );
}

function ToolUseRow({ toolUse, result }: { toolUse: ToolUseEntry; result: ToolResultEntry | undefined }) {
  const [expanded, setExpanded] = useState(false);
  const failed = result?.is_error === true;

  return (
    <div className="min-w-0">
      <button
        className={`flex w-full min-w-0 items-center gap-1.5 rounded-[3px] px-1 py-0.5 text-left text-[11px] hover:bg-[var(--ok-hover)] ${
          failed ? "text-[var(--ok-red)]" : "text-[var(--ok-text-secondary)]"
        }`}
        disabled={!result}
        onClick={() => setExpanded((value) => !value)}
        title={toolUse.summary}
      >
        <span className="w-2 flex-shrink-0 text-[var(--ok-text-muted)]">{result ? (expanded ? "▾" : "▸") : "·"}</span>
        <span className="flex-shrink-0 font-bold">{toolUse.name}</span>
        {toolUse.summary && <span className="min-w-0 truncate">— {toolUse.summary}</span>}
      </button>
      {expanded && result && <ToolResultBlock result={result} />}
    </div>
  );
}

function ToolResultBlock({ result }: { result: ToolResultEntry }) {
  return (
    <pre
      className={`ml-3 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-[3px] border p-2 text-[11px] ${
        result.is_error
          ? "border-[var(--ok-red)]/50 text-[var(--ok-red)]"
          : "border-[var(--ok-border)] text-[var(--ok-text-secondary)]"
      }`}
    >
      {result.text}
    </pre>
  );
}
