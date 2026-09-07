export type TerminalTakeoverStatus =
  | "ready"
  | "claiming"
  | "resizing"
  | "settling"
  | "blocked";

export type TerminalResizeRequest = {
  cols: number;
  rows: number;
  claim: boolean;
};

export type TerminalTakeover = {
  status: () => TerminalTakeoverStatus;
  requestResize: (
    cols: number,
    rows: number,
    forceClaim?: boolean,
  ) => TerminalResizeRequest | null;
  handleAcknowledgement: (
    cols: number,
    rows: number,
    accepted: boolean,
  ) => void;
  handleAuthorityChange: (serverOwns: boolean) => void;
  settle: () => void;
};

export function createTerminalTakeover(enabled: boolean): TerminalTakeover {
  let currentStatus: TerminalTakeoverStatus = "ready";
  let ownsResize = !enabled;
  let desired: { cols: number; rows: number } | null = null;

  return {
    status: () => currentStatus,
    requestResize(cols, rows, forceClaim = false) {
      if (!enabled) return { cols, rows, claim: false };
      if (currentStatus === "blocked" && !forceClaim) return null;

      desired = { cols, rows };
      const claim = forceClaim || !ownsResize;
      currentStatus = claim ? "claiming" : "resizing";
      return { cols, rows, claim };
    },
    handleAcknowledgement(cols, rows, accepted) {
      if (!accepted) {
        ownsResize = false;
        currentStatus = "blocked";
        return;
      }
      if (!desired || desired.cols !== cols || desired.rows !== rows) return;
      ownsResize = true;
      currentStatus = "settling";
    },
    handleAuthorityChange(serverOwns) {
      if (!enabled || !serverOwns) return;
      ownsResize = false;
      currentStatus = "blocked";
    },
    settle() {
      if (currentStatus === "settling") currentStatus = "ready";
    },
  };
}
