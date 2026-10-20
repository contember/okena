import { useEffect, useRef, useState } from "react";
import { postAction, postClaudePrompt } from "../api/client";
import type { ApiClaudePromptDelivery } from "../api/types";
import { useCoarsePointer } from "../hooks/useCoarsePointer";
import { DANGER_BUTTON_CLASS, PRIMARY_BUTTON_CLASS, TEXT_INPUT_CLASS } from "./claudeStyles";

const NOTICE_TTL_MS = 4000;

type Notice = { tone: "info" | "error"; text: string };

const DELIVERY_NOTICE: Record<ApiClaudePromptDelivery, string> = {
  channel: "sent",
  pty: "typed into terminal",
  queued: "queued until Claude finishes",
};

export function ClaudeComposer({ terminalId, busy }: { terminalId: string; busy: boolean }) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  // A soft keyboard's Enter is the only way to type a newline, so it must not send.
  const enterSends = !useCoarsePointer();

  // On phones focusing would pop the soft keyboard over the transcript.
  useEffect(() => {
    if (enterSends) textareaRef.current?.focus();
  }, [enterSends]);

  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), NOTICE_TTL_MS);
    return () => clearTimeout(timer);
  }, [notice]);

  const send = async () => {
    const prompt = text.trim();
    if (!prompt || sending) return;
    setSending(true);
    try {
      const result = await postClaudePrompt(terminalId, prompt);
      if (result.status === "accepted") {
        setText("");
        setNotice({ tone: "info", text: DELIVERY_NOTICE[result.response.delivery] });
      } else {
        setNotice({ tone: "error", text: result.reason });
      }
    } catch (e) {
      setNotice({ tone: "error", text: e instanceof Error ? e.message : "Failed to send" });
    } finally {
      setSending(false);
    }
  };

  const interrupt = () => {
    postAction({ action: "send_special_key", terminal_id: terminalId, key: "Escape" }).catch((e) => {
      setNotice({ tone: "error", text: e instanceof Error ? e.message : "Failed to interrupt" });
    });
  };

  return (
    <div className="flex flex-shrink-0 flex-col gap-1.5 border-t border-[var(--ok-border)] p-2">
      {notice && (
        <div className={`text-[11px] ${notice.tone === "error" ? "text-[var(--ok-red)]" : "text-[var(--ok-green)]"}`}>
          {notice.text}
        </div>
      )}
      <div className="flex items-end gap-2">
        <textarea
          ref={textareaRef}
          className={`${TEXT_INPUT_CLASS} max-h-40 min-h-[38px] resize-none`}
          rows={2}
          value={text}
          placeholder="Message Claude"
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (enterSends && event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              send();
            }
          }}
        />
        <div className="flex flex-shrink-0 flex-col gap-1.5">
          {busy && (
            <button className={DANGER_BUTTON_CLASS} onClick={interrupt} title="Send Escape to the terminal">
              Interrupt
            </button>
          )}
          <button className={PRIMARY_BUTTON_CLASS} disabled={sending || !text.trim()} onClick={send}>
            Send
          </button>
        </div>
      </div>
    </div>
  );
}
