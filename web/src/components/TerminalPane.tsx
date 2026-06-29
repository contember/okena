import { useEffect, useRef, useCallback, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { postAction } from "../api/client";
import { useApp } from "../state/store";

/** Minimum container dimensions (px) required for fit() to produce usable results. */
const MIN_FIT_WIDTH = 40;
const MIN_FIT_HEIGHT = 30;

function safeFit(fit: FitAddon, container: HTMLElement): boolean {
  const { width, height } = container.getBoundingClientRect();
  if (width < MIN_FIT_WIDTH || height < MIN_FIT_HEIGHT) return false;
  try {
    fit.fit();
    return true;
  } catch {
    return false;
  }
}

export function TerminalPane({
  terminalId,
  name,
  projectId,
  path,
  hideSplitActions,
}: {
  terminalId: string | null;
  name?: string;
  projectId: string;
  path: number[];
  hideSplitActions?: boolean;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const { ws, registry, state } = useApp();
  const resizeTimer = useRef<ReturnType<typeof setTimeout>>(null);
  // Incremented when a new xterm instance is created, so the registration
  // effect re-runs even if streamId hasn't changed (e.g. after split remount).
  const [termReady, setTermReady] = useState(0);

  // Keep a ref to streamMappings so closures always see the latest value
  const streamMappingsRef = useRef(state.streamMappings);
  streamMappingsRef.current = state.streamMappings;

  // Subscribe to terminal on mount (snapshot arrives automatically)
  useEffect(() => {
    if (!terminalId) return;
    ws.subscribe([terminalId]);
    return () => ws.unsubscribe([terminalId]);
  }, [terminalId, ws]);

  // Register in TerminalRegistry when streamId is available AND terminal is ready.
  // `termReady` ensures this re-runs after the xterm instance is (re)created.
  const streamId = terminalId ? state.streamMappings[terminalId] : undefined;

  useEffect(() => {
    if (streamId == null || !termRef.current) return;
    const term = termRef.current;
    registry.register(streamId, (data) => term.write(data));
    return () => registry.unregister(streamId);
  }, [streamId, registry, termReady]);

  // Send resize when terminal dimensions change
  const sendResize = useCallback(() => {
    if (!terminalId || !termRef.current) return;
    const { cols, rows } = termRef.current;
    if (cols > 1 && rows > 1) {
      ws.resize(terminalId, cols, rows);
    }
  }, [terminalId, ws]);

  // Actions
  const handleFocus = useCallback(() => {
    if (!terminalId) return;
    postAction({ action: "focus_terminal", project_id: projectId, terminal_id: terminalId }).catch(() => {});
  }, [terminalId, projectId]);

  const handleSplit = useCallback((direction: "horizontal" | "vertical") => {
    postAction({ action: "split_terminal", project_id: projectId, path, direction }).catch(() => {});
  }, [projectId, path]);

  const handleClose = useCallback(() => {
    if (!terminalId) return;
    postAction({ action: "close_terminal", project_id: projectId, terminal_id: terminalId }).catch(() => {});
  }, [terminalId, projectId]);

  const handleRename = useCallback(() => {
    if (!terminalId) return;
    const nextName = window.prompt("Rename terminal", name ?? "")?.trim();
    if (!nextName || nextName === name) return;
    postAction({
      action: "rename_terminal",
      project_id: projectId,
      terminal_id: terminalId,
      name: nextName,
    }).catch(() => {});
  }, [terminalId, projectId, name]);

  // Create xterm.js instance
  useEffect(() => {
    if (!containerRef.current) return;

    const term = new Terminal({
      fontSize: 14,
      fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', Menlo, Monaco, monospace",
      theme: {
        background: "#1e1e1e",
        foreground: "#cccccc",
        cursor: "#aeafad",
        selectionBackground: "#264f78",
        black: "#000000",
        red: "#cd3131",
        green: "#0dbc79",
        yellow: "#e5e510",
        blue: "#2472c8",
        magenta: "#bc3fbc",
        cyan: "#11a8cd",
        white: "#e5e5e5",
        brightBlack: "#666666",
        brightRed: "#f14c4c",
        brightGreen: "#23d18b",
        brightYellow: "#f5f543",
        brightBlue: "#3b8eea",
        brightMagenta: "#d670d6",
        brightCyan: "#29b8db",
        brightWhite: "#ffffff",
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
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        webgl.dispose(); // Falls back to canvas renderer
      });
      term.loadAddon(webgl);
    } catch {
      // WebGL not supported, canvas renderer is fine
    }

    const container = containerRef.current;
    safeFit(fit, container);
    sendResize();

    termRef.current = term;
    fitRef.current = fit;
    setTermReady((r) => r + 1);

    // Forward user input to server (prefer binary frames when streamId is available)
    if (terminalId) {
      term.onData((data) => {
        const sid = streamMappingsRef.current[terminalId];
        if (sid != null) {
          ws.sendBinaryInput(sid, data);
        } else {
          ws.sendText(terminalId, data);
        }
      });
    }

    // ResizeObserver for fit
    const observer = new ResizeObserver(() => {
      if (resizeTimer.current) clearTimeout(resizeTimer.current);
      resizeTimer.current = setTimeout(() => {
        if (safeFit(fit, container)) {
          sendResize();
        }
      }, 100);
    });
    observer.observe(container);

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
    <div className="terminal-pane flex h-full flex-col" onMouseDown={handleFocus}>
      <div className="terminal-header flex flex-shrink-0 items-center border-b px-2">
        <span className="min-w-0 flex-1 truncate text-[11px] text-[var(--ok-text-secondary)]">
          {name ?? "Terminal"}
        </span>
        <div className="flex items-center gap-0.5 ml-2">
          {!hideSplitActions && (
            <>
              <button
                onClick={(e) => { e.stopPropagation(); handleSplit("horizontal"); }}
                className="icon-button"
                title="Split horizontal"
                aria-label="Split horizontal"
              >
                &#x2500;
              </button>
              <button
                onClick={(e) => { e.stopPropagation(); handleSplit("vertical"); }}
                className="icon-button"
                title="Split vertical"
                aria-label="Split vertical"
              >
                &#x2502;
              </button>
            </>
          )}
          <button
            onClick={(e) => { e.stopPropagation(); handleRename(); }}
            className="icon-button"
            title="Rename terminal"
            aria-label="Rename terminal"
          >
            R
          </button>
          <button
            onClick={(e) => { e.stopPropagation(); handleClose(); }}
            className="icon-button icon-button-danger"
            title="Close terminal"
            aria-label="Close terminal"
          >
            &#x2715;
          </button>
        </div>
      </div>
      <div ref={containerRef} className="flex-1 min-h-0 overflow-hidden" />
    </div>
  );
}
