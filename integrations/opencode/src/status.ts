import { open, readFile } from "node:fs/promises";
import { bindingSchema } from "./rpc.js";
import { z } from "zod";
import { isAbsolute } from "node:path";

export type AgentState = "clear" | "working" | "blocked" | "done" | "idle";
export const paneSchema = bindingSchema
  .omit({ sessionID: true, hostname: true })
  .extend({
    ttyFile: z.string().max(4096).refine(isAbsolute).optional(),
    tty: z.string().max(4096).refine(isAbsolute).optional(),
  });
export type Pane = z.infer<typeof paneSchema>;

export async function reportStatus(
  pane: Pane,
  sessionID: string,
  state: AgentState,
): Promise<void> {
  let device: string | undefined;
  if (pane.ttyFile) {
    device = (await readFile(pane.ttyFile, "utf8")).trim();
  } else {
    device = pane.tty;
  }
  if (!device) return;
  const label = Buffer.from(
    JSON.stringify({ agent: "opencode", session_id: sessionID }),
  ).toString("base64");
  const file = await open(device, "r+");
  try {
    await file.write(
      `\x1b]9001;st=${state};tid=${pane.terminalID};lbl=${label}\x1b\\`,
    );
  } finally {
    await file.close();
  }
}
