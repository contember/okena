import { useCallback, useState } from "react";
import { postClaudeDecision } from "../api/client";
import type { ApiClaudeDecision, ApiClaudeDialog } from "../api/types";
import {
  toolInputDescription,
  toolInputHeadline,
  toolInputPlan,
  toolInputQuestions,
} from "../utils/claudeToolInput";
import { ClaudeQuestionCard } from "./ClaudeQuestionCard";
import {
  DANGER_BUTTON_CLASS,
  PRIMARY_BUTTON_CLASS,
  SECONDARY_BUTTON_CLASS,
  TEXT_INPUT_CLASS,
} from "./claudeStyles";

export interface DecisionSubmitter {
  submit: (decision: ApiClaudeDecision) => void;
  pending: boolean;
  error: string | null;
}

/** Sends one decision for a dialog; a dialog that is already gone is dismissed silently. */
function useDecision(terminalId: string, dialogId: string, onDone: (dialogId: string) => void): DecisionSubmitter {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = useCallback(
    (decision: ApiClaudeDecision) => {
      setPending(true);
      setError(null);
      postClaudeDecision(terminalId, dialogId, decision)
        .then(() => onDone(dialogId))
        .catch((e) => {
          setPending(false);
          setError(e instanceof Error ? e.message : "Failed to answer");
        });
    },
    [terminalId, dialogId, onDone],
  );

  return { submit, pending, error };
}

export function ClaudeDialogCard({
  terminalId,
  dialog,
  onDone,
}: {
  terminalId: string;
  dialog: ApiClaudeDialog;
  onDone: (dialogId: string) => void;
}) {
  const decision = useDecision(terminalId, dialog.id, onDone);

  if (dialog.tool_name === "AskUserQuestion") {
    const questions = toolInputQuestions(dialog.tool_input);
    if (questions) {
      return (
        <CardFrame title="Claude asks" error={decision.error}>
          <ClaudeQuestionCard questions={questions} decision={decision} />
        </CardFrame>
      );
    }
  }

  if (dialog.tool_name === "ExitPlanMode") {
    const plan = toolInputPlan(dialog.tool_input);
    if (plan !== null) {
      return (
        <CardFrame title="Plan approval" error={decision.error}>
          <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-[3px] bg-[var(--ok-canvas)] p-2 text-[12px]">
            {plan}
          </pre>
          <DenyRow
            decision={decision}
            approveLabel="Approve"
            denyLabel="Keep planning"
            placeholder="What to change (optional)"
          />
        </CardFrame>
      );
    }
  }

  return (
    <CardFrame title={dialog.tool_name} error={decision.error}>
      <ToolInputPreview dialog={dialog} />
      <DenyRow
        decision={decision}
        approveLabel="Allow"
        denyLabel="Deny"
        placeholder="Reason (optional)"
        allowAlways={dialog.permission_suggestions.length > 0}
      />
    </CardFrame>
  );
}

function CardFrame({
  title,
  error,
  children,
}: {
  title: string;
  error: string | null;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2 rounded-[var(--ok-radius)] border border-[var(--ok-yellow)]/40 bg-[var(--ok-panel-raised)] p-2">
      <h3 className="truncate text-[12px] font-bold text-[var(--ok-yellow)]">{title}</h3>
      {children}
      {error && <div className="text-[11px] text-[var(--ok-red)]">{error}</div>}
    </section>
  );
}

function ToolInputPreview({ dialog }: { dialog: ApiClaudeDialog }) {
  const headline = toolInputHeadline(dialog.tool_input);
  const description = toolInputDescription(dialog.tool_input);
  const json = JSON.stringify(dialog.tool_input, null, 2);

  if (headline === null) {
    return <pre className={PREVIEW_CLASS}>{json}</pre>;
  }
  return (
    <div className="flex flex-col gap-1">
      {description && <div className="text-[11px] text-[var(--ok-text-secondary)]">{description}</div>}
      <pre className={PREVIEW_CLASS}>{headline}</pre>
      <details className="text-[11px] text-[var(--ok-text-muted)]">
        <summary className="cursor-pointer">full input</summary>
        <pre className={PREVIEW_CLASS}>{json}</pre>
      </details>
    </div>
  );
}

const PREVIEW_CLASS =
  "max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-[3px] bg-[var(--ok-canvas)] p-2 text-[12px] text-[var(--ok-text)]";

function DenyRow({
  decision,
  approveLabel,
  denyLabel,
  placeholder,
  allowAlways = false,
}: {
  decision: DecisionSubmitter;
  approveLabel: string;
  denyLabel: string;
  placeholder: string;
  allowAlways?: boolean;
}) {
  const [message, setMessage] = useState("");
  const trimmed = message.trim();

  return (
    <div className="flex flex-col gap-2">
      <input
        className={TEXT_INPUT_CLASS}
        value={message}
        placeholder={placeholder}
        disabled={decision.pending}
        onChange={(event) => setMessage(event.target.value)}
      />
      <div className="flex flex-wrap gap-2">
        <button
          className={PRIMARY_BUTTON_CLASS}
          disabled={decision.pending}
          onClick={() => decision.submit({ decision: "allow", always: false })}
        >
          {approveLabel}
        </button>
        {allowAlways && (
          <button
            className={SECONDARY_BUTTON_CLASS}
            disabled={decision.pending}
            onClick={() => decision.submit({ decision: "allow", always: true })}
          >
            Always allow
          </button>
        )}
        <button
          className={DANGER_BUTTON_CLASS}
          disabled={decision.pending}
          onClick={() =>
            decision.submit(trimmed ? { decision: "deny", message: trimmed } : { decision: "deny" })
          }
        >
          {denyLabel}
        </button>
      </div>
    </div>
  );
}
