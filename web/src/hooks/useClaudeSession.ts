import { useCallback, useEffect, useRef, useState } from "react";
import { getClaudeSession, getClaudeTranscript } from "../api/client";
import type { ApiClaudeEntry, ApiClaudeSession } from "../api/types";

const POLL_INTERVAL_MS = 1000;

export interface ClaudeSessionView {
  session: ApiClaudeSession | null;
  entries: ApiClaudeEntry[];
  /** True once a transcript request answered 404 (no readable transcript). */
  transcriptMissing: boolean;
  error: string | null;
  /** Hide a dialog locally right after it was answered, before the next poll drops it. */
  dismissDialog: (dialogId: string) => void;
}

/** Polls the Claude session and its transcript every second while mounted. */
export function useClaudeSession(terminalId: string): ClaudeSessionView {
  const [session, setSession] = useState<ApiClaudeSession | null>(null);
  const [entries, setEntries] = useState<ApiClaudeEntry[]>([]);
  const [transcriptMissing, setTranscriptMissing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dismissedDialogIds, setDismissedDialogIds] = useState<ReadonlySet<string>>(new Set());
  const cursorRef = useRef<number | null>(null);
  const sessionIdRef = useRef<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let inFlight = false;
    cursorRef.current = null;
    sessionIdRef.current = null;
    setSession(null);
    setEntries([]);
    setTranscriptMissing(false);
    setError(null);

    const poll = async () => {
      timer = null;
      // A hidden page stops here; the visibilitychange listener restarts the loop.
      if (document.hidden) return;
      inFlight = true;
      try {
        const [nextSession, page] = await Promise.all([
          getClaudeSession(terminalId),
          getClaudeTranscript(terminalId, cursorRef.current, sessionIdRef.current),
        ]);
        if (cancelled) return;
        setSession(nextSession);
        setError(null);
        if (page === null) {
          cursorRef.current = null;
          sessionIdRef.current = null;
          setEntries([]);
          setTranscriptMissing(true);
        } else {
          const pageSessionId = page.session_id ?? null;
          const replace = page.reset || pageSessionId !== sessionIdRef.current;
          cursorRef.current = page.cursor;
          sessionIdRef.current = pageSessionId;
          setTranscriptMissing(false);
          if (replace) {
            setEntries(page.entries);
          } else if (page.entries.length > 0) {
            setEntries((current) => [...current, ...page.entries]);
          }
        }
      } catch (e) {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : "Request failed");
      }
      inFlight = false;
      if (!cancelled) {
        timer = setTimeout(poll, POLL_INTERVAL_MS);
      }
    };

    const onVisibilityChange = () => {
      if (cancelled || document.hidden || inFlight) return;
      if (timer) clearTimeout(timer);
      poll();
    };

    document.addEventListener("visibilitychange", onVisibilityChange);
    poll();

    return () => {
      cancelled = true;
      document.removeEventListener("visibilitychange", onVisibilityChange);
      if (timer) clearTimeout(timer);
    };
  }, [terminalId]);

  const dismissDialog = useCallback((dialogId: string) => {
    setDismissedDialogIds((current) => new Set(current).add(dialogId));
  }, []);

  const visibleSession = session && dismissedDialogIds.size > 0
    ? { ...session, dialogs: session.dialogs.filter((dialog) => !dismissedDialogIds.has(dialog.id)) }
    : session;

  return { session: visibleSession, entries, transcriptMissing, error, dismissDialog };
}
