import type { WsInbound, WsOutbound } from "./types";
import { parsePtyFrame } from "./types";
import { loadToken } from "../auth/token";

export type WsStatus = "connecting" | "connected" | "disconnected";
export type PtyDataHandler = (streamId: number, data: Uint8Array) => void;
export type JsonHandler = (msg: WsOutbound) => void;
export type StatusHandler = (status: WsStatus) => void;

export class WsManager {
  private ws: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectDelay = 1000;
  private disposed = false;
  private subscribedTerminals = new Set<string>();

  onPtyData: PtyDataHandler = () => {};
  onJson: JsonHandler = () => {};
  onStatus: StatusHandler = () => {};

  connect(): void {
    if (this.disposed) return;
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
        const frame = parsePtyFrame(event.data);
        if (frame) {
          this.onPtyData(frame.streamId, frame.payload);
        }
        return;
      }
      const msg: WsOutbound = JSON.parse(event.data as string);
      if (msg.type === "auth_ok") {
        this.onStatus("connected");
        // Re-subscribe to previously subscribed terminals
        if (this.subscribedTerminals.size > 0) {
          this.subscribe([...this.subscribedTerminals]);
        }
      }
      this.onJson(msg);
    };

    this.ws.onclose = () => {
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
    this.sendJson({ type: "subscribe", terminal_ids: terminalIds });
  }

  unsubscribe(terminalIds: string[]): void {
    for (const id of terminalIds) {
      this.subscribedTerminals.delete(id);
    }
    this.sendJson({ type: "unsubscribe", terminal_ids: terminalIds });
  }

  sendText(terminalId: string, text: string): void {
    this.sendJson({ type: "send_text", terminal_id: terminalId, text });
  }

  resize(terminalId: string, cols: number, rows: number): void {
    this.sendJson({ type: "resize", terminal_id: terminalId, cols, rows });
  }

  dispose(): void {
    this.disposed = true;
    this.cleanup();
  }

  private sendJson(msg: WsInbound): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  private cleanup(): void {
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
