import { useEffect, useRef, useCallback } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { postAction } from "../api/client";
import { useApp } from "../state/store";

export function TerminalPane({
  terminalId,
  name,
}: {
  terminalId: string | null;
  name?: string;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const { ws, registry, state } = useApp();
  const resizeTimer = useRef<ReturnType<typeof setTimeout>>(null);

  // Subscribe to terminal on mount
  useEffect(() => {
    if (!terminalId) return;
    ws.subscribe([terminalId]);
    return () => ws.unsubscribe([terminalId]);
  }, [terminalId, ws]);

  // Fetch initial content
  useEffect(() => {
    if (!terminalId) return;
    postAction({ action: "read_content", terminal_id: terminalId })
      .then((content) => {
        if (content && termRef.current) {
          termRef.current.write(content);
        }
      })
      .catch(() => {});
  }, [terminalId]);

  // Register in TerminalRegistry when streamId is available
  const streamId = terminalId ? state.streamMappings[terminalId] : undefined;

  useEffect(() => {
    if (streamId == null || !termRef.current) return;
    const term = termRef.current;
    registry.register(streamId, (data) => term.write(data));
    return () => registry.unregister(streamId);
  }, [streamId, registry]);

  // Send resize when terminal dimensions change
  const sendResize = useCallback(() => {
    if (!terminalId || !termRef.current) return;
    const { cols, rows } = termRef.current;
    if (cols > 0 && rows > 0) {
      ws.resize(terminalId, cols, rows);
    }
  }, [terminalId, ws]);

  // Create xterm.js instance
  useEffect(() => {
    if (!containerRef.current) return;

    const term = new Terminal({
      fontSize: 14,
      fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', Menlo, Monaco, monospace",
      theme: {
        background: "#09090b",
        foreground: "#e4e4e7",
        cursor: "#e4e4e7",
        selectionBackground: "#3f3f46",
        black: "#18181b",
        red: "#ef4444",
        green: "#22c55e",
        yellow: "#eab308",
        blue: "#3b82f6",
        magenta: "#a855f7",
        cyan: "#06b6d4",
        white: "#e4e4e7",
        brightBlack: "#52525b",
        brightRed: "#f87171",
        brightGreen: "#4ade80",
        brightYellow: "#facc15",
        brightBlue: "#60a5fa",
        brightMagenta: "#c084fc",
        brightCyan: "#22d3ee",
        brightWhite: "#fafafa",
      },
      allowProposedApi: true,
      scrollback: 5000,
      cursorBlink: true,
    });

    const fit = new FitAddon();
    term.loadAddon(fit);

    term.open(containerRef.current);

    // Try WebGL renderer, fall back to canvas
    try {
      term.loadAddon(new WebglAddon());
    } catch {
      // WebGL not supported, canvas renderer is fine
    }

    fit.fit();

    termRef.current = term;
    fitRef.current = fit;

    // Forward user input to server
    if (terminalId) {
      term.onData((data) => {
        ws.sendText(terminalId, data);
      });
    }

    // ResizeObserver for fit
    const observer = new ResizeObserver(() => {
      if (resizeTimer.current) clearTimeout(resizeTimer.current);
      resizeTimer.current = setTimeout(() => {
        fit.fit();
        sendResize();
      }, 100);
    });
    observer.observe(containerRef.current);

    return () => {
      observer.disconnect();
      if (resizeTimer.current) clearTimeout(resizeTimer.current);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [terminalId, ws, sendResize]);

  if (!terminalId) {
    return (
      <div className="flex items-center justify-center h-full text-zinc-600 text-sm">
        No terminal
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full">
      {name && (
        <div className="flex-shrink-0 px-2 py-1 text-xs text-zinc-500 bg-zinc-900 border-b border-zinc-800 truncate">
          {name}
        </div>
      )}
      <div ref={containerRef} className="flex-1 min-h-0" />
    </div>
  );
}
