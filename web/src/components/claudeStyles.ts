import type { AgentLifecycle } from "../api/types";

export const TEXT_INPUT_CLASS =
  "w-full rounded-[3px] border border-[var(--ok-border)] bg-[var(--ok-canvas)] px-2 py-1.5 text-[16px] text-[var(--ok-text)] md:text-[12px] placeholder:text-[var(--ok-text-muted)] focus:border-[var(--ok-border-strong)] focus:outline-none";

const BUTTON_BASE_CLASS = "rounded-[3px] px-3 py-1.5 text-[12px] disabled:opacity-50";

export const PRIMARY_BUTTON_CLASS = `${BUTTON_BASE_CLASS} bg-[var(--ok-blue)] text-white hover:brightness-110`;

export const SECONDARY_BUTTON_CLASS = `${BUTTON_BASE_CLASS} border border-[var(--ok-border)] text-[var(--ok-text)] hover:bg-[var(--ok-hover)]`;

export const DANGER_BUTTON_CLASS = `${BUTTON_BASE_CLASS} border border-[var(--ok-red)]/60 text-[var(--ok-red)] hover:bg-[var(--ok-red)]/10`;

export const LIFECYCLE_COLOR: Record<AgentLifecycle, string> = {
  blocked: "var(--ok-red)",
  working: "var(--ok-yellow)",
  done: "var(--ok-green)",
  idle: "var(--ok-text-muted)",
};
