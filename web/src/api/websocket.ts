import type { WsInbound, WsOutbound } from "./types";
import {
  parseBinaryFrame,
  buildBinaryFrame,
  FRAME_TYPE_PTY,
  FRAME_TYPE_SNAPSHOT,
  FRAME_TYPE_INPUT,
} from "./types";
import { loadToken } from "../auth/token";

export type WsStatus = "connecting" | "connected" | "disconnected";
export type PtyDataHandler = (
  streamId: number,
  data: Uint8Array,
  isSnapshot: boolean,
) => void;
export type JsonHandler = (msg: WsOutbound) => void;
export type StatusHandler = (status: WsStatus) => void;
export type TerminalResizeMessage = Extract<
  WsOutbound,
  { type: "terminal_resized" | "resize_acknowledged" }
>;
type TerminalResizeHandler = (message: TerminalResizeMessage) => void;

type PendingResize = {
  cols: number;
  rows: number;
  claim: boolean;
};

export class WsManager {
  private ws: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectDelay = 1000;
  private disposed = false;
  private authenticated = false;
  private subscribedTerminals = new Set<string>();
  private pendingResizes = new Map<string, PendingResize>();
  private resizeHandlers = new Map<string, Set<TerminalResizeHandler>>();

  onPtyData: PtyDataHandler = () => {};
  onJson: JsonHandler = () => {};
  onStatus: StatusHandler = () => {};

  connect(): void {
    this.disposed = false;
    this.cleanup();
    this.onStatus("connecting");

    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${window.location.host}/v1/stream`;
    this.ws = new WebSocket(url);
    this.ws.binaryType = "arraybuffer";

    this.ws.onopen = () => {
      this.reconnectDelay = 1000;
      const token = loadToken();
      if (token) {
        this.sendJson({ type: "auth", token });
      }
    };

    this.ws.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer) {
        const frame = parseBinaryFrame(event.data);
        if (
          frame &&
          (frame.frameType === FRAME_TYPE_PTY ||
            frame.frameType === FRAME_TYPE_SNAPSHOT)
        ) {
          this.onPtyData(
            frame.streamId,
            frame.payload,
            frame.frameType === FRAME_TYPE_SNAPSHOT,
          );
        }
        return;
      }
      let msg: WsOutbound;
      try {
        msg = JSON.parse(event.data as string) as WsOutbound;
      } catch {
        return;
      }
      if (
        msg.type === "terminal_resized" ||
        msg.type === "resize_acknowledged"
      ) {
        for (const handler of this.resizeHandlers.get(msg.terminal_id) ?? []) {
          handler(msg);
        }
      }
      if (msg.type === "auth_ok") {
        this.authenticated = true;
        // A resize claim must reach the daemon before subscribe, otherwise the
        // initial snapshot is rendered with the previous owner's dimensions.
        this.flushPendingResizes();
        if (this.subscribedTerminals.size > 0) {
          this.sendJson({
            type: "subscribe",
            terminal_ids: [...this.subscribedTerminals],
          });
        }
        this.onStatus("connected");
      }
      this.onJson(msg);
    };

    this.ws.onclose = () => {
      this.authenticated = false;
      this.onStatus("disconnected");
      this.scheduleReconnect();
    };

    this.ws.onerror = () => {
      // onclose will fire after onerror
    };
  }

  subscribe(terminalIds: string[]): void {
    for (const id of terminalIds) {
      this.subscribedTerminals.add(id);
    }
    if (this.authenticated) {
      this.sendJson({ type: "subscribe", terminal_ids: terminalIds });
    }
  }

  unsubscribe(terminalIds: string[]): void {
    for (const id of terminalIds) {
      this.subscribedTerminals.delete(id);
      this.pendingResizes.delete(id);
    }
    if (this.authenticated) {
      this.sendJson({ type: "unsubscribe", terminal_ids: terminalIds });
    }
  }

  sendText(terminalId: string, text: string): void {
    if (this.authenticated) {
      this.sendJson({ type: "send_text", terminal_id: terminalId, text });
    }
  }

  /** Send terminal input as a binary frame (more efficient than JSON for keystrokes). */
  sendBinaryInput(streamId: number, text: string): void {
    if (!this.authenticated || this.ws?.readyState !== WebSocket.OPEN) return;
    const encoded = new TextEncoder().encode(text);
    this.ws.send(buildBinaryFrame(FRAME_TYPE_INPUT, streamId, encoded));
  }

  onTerminalResize(
    terminalId: string,
    handler: TerminalResizeHandler,
  ): () => void {
    const handlers = this.resizeHandlers.get(terminalId) ?? new Set();
    handlers.add(handler);
    this.resizeHandlers.set(terminalId, handlers);
    return () => {
      handlers.delete(handler);
      if (handlers.size === 0) this.resizeHandlers.delete(terminalId);
    };
  }

  resize(terminalId: string, cols: number, rows: number, claim = false): void {
    const resize = { cols, rows, claim };
    this.pendingResizes.set(terminalId, resize);
    if (this.authenticated) {
      this.sendResize(terminalId, resize);
    }
  }

  dispose(): void {
    this.disposed = true;
    this.cleanup();
  }

  private flushPendingResizes(): void {
    for (const [terminalId, resize] of this.pendingResizes) {
      this.sendResize(terminalId, resize);
    }
  }

  private sendResize(terminalId: string, resize: PendingResize): void {
    this.sendJson({
      type: "resize",
      terminal_id: terminalId,
      cols: resize.cols,
      rows: resize.rows,
      claim: resize.claim,
    });
  }

  private sendJson(msg: WsInbound): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  private cleanup(): void {
    this.authenticated = false;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.onopen = null;
      this.ws.onmessage = null;
      this.ws.onclose = null;
      this.ws.onerror = null;
      this.ws.close();
      this.ws = null;
    }
  }

  private scheduleReconnect(): void {
    if (this.disposed) return;
    this.reconnectTimer = setTimeout(() => {
      this.connect();
    }, this.reconnectDelay);
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, 30000);
  }
}
