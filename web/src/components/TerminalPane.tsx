import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { postAction } from "../api/client";
import { useApp } from "../state/store";
import {
  createTerminalScrollLock,
  type TerminalScrollLock,
} from "../terminal/terminalScrollLock";
import {
  createTerminalTakeover,
  type TerminalTakeover,
  type TerminalTakeoverStatus,
} from "../terminal/terminalTakeover";

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
  mobile = false,
  externalInputEpoch = 0,
}: {
  terminalId: string | null;
  name?: string;
  projectId: string;
  path: number[];
  hideSplitActions?: boolean;
  mobile?: boolean;
  externalInputEpoch?: number;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const scrollLockRef = useRef<TerminalScrollLock | null>(null);
  const takeoverRef = useRef<TerminalTakeover | null>(null);
  const redrawObservedRef = useRef(false);
  const finishTakeoverRef = useRef<(force?: boolean) => void>(() => {});
  const { ws, registry, state } = useApp();
  const resizeTimer = useRef<ReturnType<typeof setTimeout>>(null);
  const revealTimer = useRef<ReturnType<typeof setTimeout>>(null);
  const screenReadyRef = useRef(!mobile);
  // Incremented when a new xterm instance is created, so the registration
  // effect re-runs even if streamId hasn't changed (e.g. after split remount).
  const [termReady, setTermReady] = useState(0);
  const [atBottom, setAtBottom] = useState(true);
  const [screenReady, setScreenReady] = useState(!mobile);
  const [takeoverStatus, setTakeoverStatus] =
    useState<TerminalTakeoverStatus>("ready");
  const previousExternalInputEpoch = useRef(externalInputEpoch);

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
    registry.register(streamId, (data, isSnapshot) => {
      if (isSnapshot) {
        screenReadyRef.current = false;
        setScreenReady(false);
        term.reset();
      }
      term.write(data, () => {
        const pinToBottom = () => {
          term.scrollToBottom();
          setAtBottom(true);
        };
        if (isSnapshot) {
          scrollLockRef.current?.follow(pinToBottom);
        } else {
          scrollLockRef.current?.pinAfterOutput(pinToBottom);
        }
        redrawObservedRef.current = true;
        window.requestAnimationFrame(() => finishTakeoverRef.current());
      });
    });
    return () => registry.unregister(streamId);
  }, [streamId, registry, termReady]);

  // Send resize when terminal dimensions change. Mobile claims once, then
  // uses ordinary owner resizes until the daemon reports authority loss.
  const sendResize = useCallback(
    (forceClaim = false) => {
      if (!terminalId || !termRef.current) return;
      const { cols, rows } = termRef.current;
      if (cols <= 1 || rows <= 1) return;

      const request = takeoverRef.current?.requestResize(
        cols,
        rows,
        forceClaim,
      ) ?? { cols, rows, claim: false };
      if (!request) return;

      if (mobile) {
        redrawObservedRef.current = false;
        if (revealTimer.current) clearTimeout(revealTimer.current);
        screenReadyRef.current = false;
        setScreenReady(false);
        setTakeoverStatus(takeoverRef.current?.status() ?? "claiming");
      }
      ws.resize(terminalId, request.cols, request.rows, request.claim);
    },
    [mobile, terminalId, ws],
  );

  const pinTerminalToBottom = useCallback(() => {
    termRef.current?.scrollToBottom();
    setAtBottom(true);
    if (mobile) window.scrollTo(0, 0);
  }, [mobile]);

  const takeOverTerminal = useCallback(() => sendResize(true), [sendResize]);

  const scrollToBottom = useCallback(() => {
    const scrollLock = scrollLockRef.current;
    if (scrollLock) {
      scrollLock.follow(pinTerminalToBottom);
    } else {
      pinTerminalToBottom();
    }
  }, [pinTerminalToBottom]);

  useEffect(() => {
    if (previousExternalInputEpoch.current === externalInputEpoch) return;
    previousExternalInputEpoch.current = externalInputEpoch;
    scrollLockRef.current?.handleInput(pinTerminalToBottom);
  }, [externalInputEpoch, pinTerminalToBottom]);

  // Actions
  const handleFocus = useCallback(() => {
    if (!terminalId) return;
    postAction({
      action: "record_project_activity",
      project_id: projectId,
    }).catch(() => {});
  }, [terminalId, projectId]);

  const handleSplit = useCallback(
    (direction: "horizontal" | "vertical") => {
      postAction({
        action: "split_terminal",
        project_id: projectId,
        path,
        direction,
      }).catch(() => {});
    },
    [projectId, path],
  );

  const handleClose = useCallback(() => {
    if (!terminalId) return;
    postAction({
      action: "close_terminal",
      project_id: projectId,
      terminal_id: terminalId,
    }).catch(() => {});
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

  // Create and size xterm before the passive subscription effect runs. This
  // lets mobile claim the correct PTY grid before the server renders a snapshot.
  useLayoutEffect(() => {
    if (!containerRef.current) return;

    screenReadyRef.current = !mobile;
    setScreenReady(!mobile);
    const term = new Terminal({
      fontSize: mobile ? 13 : 14,
      fontFamily:
        "'JetBrains Mono', 'Fira Code', 'Cascadia Code', Menlo, Monaco, monospace",
      theme: {
        background: mobile ? "#0f1113" : "#1e1e1e",
        foreground: mobile ? "#ececee" : "#cccccc",
        cursor: mobile ? "#4c9aff" : "#aeafad",
        selectionBackground: mobile ? "rgba(76, 154, 255, 0.25)" : "#264f78",
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
      scrollOnUserInput: true,
      cursorBlink: true,
    });

    const fit = new FitAddon();
    term.loadAddon(fit);

    const container = containerRef.current;
    term.open(container);
    setAtBottom(true);
    const scrollLock = createTerminalScrollLock(mobile);
    const takeover = createTerminalTakeover(mobile);
    scrollLockRef.current = scrollLock;
    takeoverRef.current = takeover;
    const pinInputToBottom = () => {
      term.scrollToBottom();
      setAtBottom(true);
      if (mobile) window.scrollTo(0, 0);
    };
    const finishTakeover = (force = false) => {
      if (!mobile || takeover.status() !== "settling") return;
      if (!force && !redrawObservedRef.current) return;
      takeover.settle();
      scrollLock.pinAfterResize(pinInputToBottom);
      setTakeoverStatus(takeover.status());
      screenReadyRef.current = true;
      setScreenReady(true);
    };
    finishTakeoverRef.current = finishTakeover;

    const stopResizeMessages = terminalId
      ? ws.onTerminalResize(terminalId, (message) => {
          if (message.type === "resize_acknowledged") {
            takeover.handleAcknowledgement(
              message.cols,
              message.rows,
              message.accepted,
            );
          } else if (message.server_owns) {
            takeover.handleAuthorityChange(true);
          } else {
            // Accepted resize broadcasts are a legacy-compatible implicit ACK.
            takeover.handleAcknowledgement(message.cols, message.rows, true);
          }

          const status = takeover.status();
          setTakeoverStatus(status);
          if (status === "blocked") {
            if (revealTimer.current) clearTimeout(revealTimer.current);
            screenReadyRef.current = false;
            setScreenReady(false);
            return;
          }
          if (status === "settling") {
            finishTakeover();
            if (revealTimer.current) clearTimeout(revealTimer.current);
            revealTimer.current = setTimeout(() => finishTakeover(true), 350);
          }
        })
      : () => {};

    const terminalInput = container.querySelector<HTMLTextAreaElement>(
      ".xterm-helper-textarea",
    );
    const handleInputFocus = () => scrollLock.handleFocus();
    const handleInputBlur = () => scrollLock.handleBlur();
    terminalInput?.addEventListener("focus", handleInputFocus);
    terminalInput?.addEventListener("blur", handleInputBlur);

    // xterm's onScroll does not identify whether a change came from the user
    // or from resize/reflow. Capture an upward touch/wheel intent before xterm
    // processes it so manual scrollback is never mistaken for a layout change.
    let touchStartY: number | null = null;
    const handleTouchStart = (event: TouchEvent) => {
      touchStartY = event.touches[0]?.clientY ?? null;
    };
    const handleTouchMove = (event: TouchEvent) => {
      const currentY = event.touches[0]?.clientY;
      if (
        touchStartY !== null &&
        currentY !== undefined &&
        currentY - touchStartY > 6
      ) {
        scrollLock.beginUserScroll();
        touchStartY = currentY;
      }
    };
    const handleWheel = (event: WheelEvent) => {
      if (event.deltaY < 0) scrollLock.beginUserScroll();
    };
    container.addEventListener("touchstart", handleTouchStart, {
      capture: true,
      passive: true,
    });
    container.addEventListener("touchmove", handleTouchMove, {
      capture: true,
      passive: true,
    });
    container.addEventListener("wheel", handleWheel, {
      capture: true,
      passive: true,
    });

    const scrollSubscription = term.onScroll(() => {
      const buffer = term.buffer.active;
      if (
        scrollLock.handleScroll(
          buffer.viewportY,
          buffer.baseY,
          pinInputToBottom,
        )
      ) {
        return;
      }
      setAtBottom(buffer.viewportY >= buffer.baseY);
    });

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

    // Store the xterm instance before the first resize. sendResize reads termRef,
    // so the previous order silently skipped the initial PTY resize. That left
    // a desktop-sized remote grid rendering into a phone-sized xterm surface.
    termRef.current = term;
    fitRef.current = fit;
    if (safeFit(fit, container)) {
      sendResize();
    }
    const initialFitFrame = window.requestAnimationFrame(() => {
      if (safeFit(fit, container)) {
        sendResize();
      }
    });
    setTermReady((r) => r + 1);

    // Forward user input to server (prefer binary frames when streamId is available)
    if (terminalId) {
      term.onData((data) => {
        scrollLock.handleInput(pinInputToBottom);
        const sid = streamMappingsRef.current[terminalId];
        if (sid != null) {
          ws.sendBinaryInput(sid, data);
        } else {
          ws.sendText(terminalId, data);
        }
      });
    }

    const fitAfterGeometryChange = () => {
      const bufferBeforeFit = term.buffer.active;
      const wasFollowing = scrollLock.isFollowing();
      const previousViewportY = bufferBeforeFit.viewportY;
      const marker =
        mobile && !wasFollowing
          ? term.registerMarker(
              previousViewportY -
                bufferBeforeFit.baseY -
                bufferBeforeFit.cursorY,
            )
          : undefined;

      const fitted = safeFit(fit, container);
      if (fitted) {
        sendResize();
        if (wasFollowing) {
          scrollLock.pinAfterResize(pinInputToBottom);
        } else {
          const markerLine = marker?.line ?? -1;
          const fallbackLine = Math.min(
            previousViewportY,
            term.buffer.active.baseY,
          );
          term.scrollToLine(markerLine >= 0 ? markerLine : fallbackLine);
          setAtBottom(false);
        }
      }
      marker?.dispose();
      return fitted;
    };

    // ResizeObserver coalesces the noisy iOS keyboard animation into one fit.
    // Every new generation cancels an older reveal, including open -> close.
    const observer = new ResizeObserver(() => {
      if (mobile) {
        if (revealTimer.current) clearTimeout(revealTimer.current);
        screenReadyRef.current = false;
        setScreenReady(false);
      }
      if (resizeTimer.current) clearTimeout(resizeTimer.current);
      resizeTimer.current = setTimeout(fitAfterGeometryChange, 100);
    });
    observer.observe(container);

    return () => {
      observer.disconnect();
      stopResizeMessages();
      terminalInput?.removeEventListener("focus", handleInputFocus);
      terminalInput?.removeEventListener("blur", handleInputBlur);
      container.removeEventListener("touchstart", handleTouchStart, true);
      container.removeEventListener("touchmove", handleTouchMove, true);
      container.removeEventListener("wheel", handleWheel, true);
      scrollSubscription.dispose();
      window.cancelAnimationFrame(initialFitFrame);
      if (resizeTimer.current) clearTimeout(resizeTimer.current);
      if (revealTimer.current) clearTimeout(revealTimer.current);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
      if (scrollLockRef.current === scrollLock) scrollLockRef.current = null;
      if (takeoverRef.current === takeover) takeoverRef.current = null;
      finishTakeoverRef.current = () => {};
    };
  }, [terminalId, ws, sendResize, mobile]);

  if (!terminalId) {
    return (
      <div className="flex items-center justify-center h-full text-zinc-600 text-sm">
        No terminal
      </div>
    );
  }

  return (
    <div
      className={`terminal-pane flex h-full flex-col ${mobile ? "mobile-terminal-pane" : ""}`}
      onMouseDown={handleFocus}
    >
      {!mobile && (
        <div className="terminal-header flex flex-shrink-0 items-center border-b px-2">
          <span className="min-w-0 flex-1 truncate text-[11px] text-[var(--ok-text-secondary)]">
            {name ?? "Terminal"}
          </span>
          <div className="flex items-center gap-0.5 ml-2">
            {!hideSplitActions && (
              <>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    handleSplit("horizontal");
                  }}
                  className="icon-button"
                  title="Split horizontal"
                  aria-label="Split horizontal"
                >
                  &#x2500;
                </button>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    handleSplit("vertical");
                  }}
                  className="icon-button"
                  title="Split vertical"
                  aria-label="Split vertical"
                >
                  &#x2502;
                </button>
              </>
            )}
            <button
              onClick={(e) => {
                e.stopPropagation();
                handleRename();
              }}
              className="icon-button"
              title="Rename terminal"
              aria-label="Rename terminal"
            >
              R
            </button>
            <button
              onClick={(e) => {
                e.stopPropagation();
                handleClose();
              }}
              className="icon-button icon-button-danger"
              title="Close terminal"
              aria-label="Close terminal"
            >
              &#x2715;
            </button>
          </div>
        </div>
      )}
      <div
        ref={containerRef}
        className={`flex-1 min-h-0 overflow-hidden ${
          mobile ? "mobile-terminal-surface" : ""
        } ${screenReady ? "mobile-terminal-surface-ready" : ""}`}
      />
      {mobile && !screenReady && (
        <div className="mobile-terminal-loading" role="status">
          {takeoverStatus === "blocked" ? (
            <div className="mobile-terminal-takeover">
              <strong>Desktop controls terminal size</strong>
              <span>Take control before showing this terminal.</span>
              <button type="button" onClick={takeOverTerminal}>
                Take over
              </button>
            </div>
          ) : (
            <span>
              {takeoverStatus === "claiming"
                ? "Taking over terminal…"
                : "Resizing terminal…"}
            </span>
          )}
        </div>
      )}
      {mobile && screenReady && !atBottom && (
        <button
          type="button"
          className="mobile-terminal-scroll-bottom"
          onClick={scrollToBottom}
          aria-label="Scroll terminal to bottom"
        >
          <span aria-hidden="true">↓</span>
          Bottom
        </button>
      )}
    </div>
  );
}
