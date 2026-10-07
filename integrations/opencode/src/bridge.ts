import { execFile } from "node:child_process";
import { hostname } from "node:os";
import { promisify } from "node:util";
import { z } from "zod";
import type { Binding } from "./rpc.js";

const execute = promisify(execFile);
const briefingSchema = z.string().trim().min(1).max(6000);

export async function readBriefing(binding: Binding): Promise<string> {
  if (binding.hostname !== hostname()) {
    throw new Error("Okena and the OpenCode server must run on the same host");
  }
  const environment: NodeJS.ProcessEnv = {
    ...process.env,
    OKENA_PROFILE: binding.profile,
  };
  if (binding.configHome === null) {
    delete environment.XDG_CONFIG_HOME;
  } else {
    environment.XDG_CONFIG_HOME = binding.configHome;
  }
  const { stdout } = await execute(
    "okena",
    [
      "mission",
      "context",
      "--terminal",
      binding.terminalID,
      "--agent",
      "opencode",
      "--session-id",
      binding.sessionID,
    ],
    { env: environment, timeout: 3000, maxBuffer: 64 * 1024 },
  );
  return briefingSchema.parse(stdout);
}
