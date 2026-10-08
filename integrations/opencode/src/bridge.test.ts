import { afterEach, expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { hostname, tmpdir } from "node:os";
import { join } from "node:path";
import { readBriefing } from "./bridge.js";
import type { Binding } from "./rpc.js";

const originalEnvironment = { ...process.env };
let directory: string | undefined;

afterEach(async () => {
  process.env = { ...originalEnvironment };
  if (directory) await rm(directory, { recursive: true, force: true });
});

async function fixture(): Promise<{ binding: Binding; capture: string }> {
  directory = await mkdtemp(join(tmpdir(), "okena-opencode-test-"));
  await writeFile(
    join(directory, "okena"),
    `#!/usr/bin/env node
const fs = require("node:fs");
fs.writeFileSync(process.env.CAPTURE, JSON.stringify({
  args: process.argv.slice(2), profile: process.env.OKENA_PROFILE,
  configHome: process.env.XDG_CONFIG_HOME ?? null
}));
if (process.env.FAIL) { process.stdout.write("stale briefing"); process.exit(1); }
process.stdout.write(process.env.BRIEFING ?? "Okena mission context: Example");
`,
    { mode: 0o700 },
  );
  process.env.PATH = directory + ":" + process.env.PATH;
  const capture = join(directory, "capture.json");
  process.env.CAPTURE = capture;
  const binding: Binding = {
    sessionID: "ses_0123456789abABCDEFGHIJKLMN",
    terminalID: "terminal-1",
    profile: "pane-profile",
    configHome: join(directory, "config with spaces"),
    hostname: hostname(),
  };
  return { binding, capture };
}

test("context lookup uses the bound pane's profile, config path and exact identity", async () => {
  const { binding, capture } = await fixture();
  process.env.OKENA_PROFILE = "server-profile";
  process.env.XDG_CONFIG_HOME = "/server/config";
  expect(await readBriefing(binding)).toBe("Okena mission context: Example");
  expect(JSON.parse(await readFile(capture, "utf8"))).toEqual({
    args: [
      "mission",
      "context",
      "--terminal",
      "terminal-1",
      "--agent",
      "opencode",
      "--session-id",
      binding.sessionID,
    ],
    profile: "pane-profile",
    configHome: binding.configHome,
  });
  await readBriefing({ ...binding, configHome: null });
  expect(JSON.parse(await readFile(capture, "utf8"))).toEqual({
    args: [
      "mission",
      "context",
      "--terminal",
      "terminal-1",
      "--agent",
      "opencode",
      "--session-id",
      binding.sessionID,
    ],
    profile: "pane-profile",
    configHome: null,
  });
});

test("failed or malformed lookups cannot deliver stale or empty output", async () => {
  const { binding } = await fixture();
  process.env.FAIL = "1";
  await expect(readBriefing(binding)).rejects.toThrow();
  delete process.env.FAIL;
  for (const briefing of ["", " ", "a".repeat(6001)]) {
    process.env.BRIEFING = briefing;
    await expect(readBriefing(binding)).rejects.toThrow();
  }
});

test("a binding from another host cannot query a local daemon", async () => {
  const { binding, capture } = await fixture();
  await expect(
    readBriefing({ ...binding, hostname: "foreign-host" }),
  ).rejects.toThrow("same host");
  await expect(readFile(capture, "utf8")).rejects.toThrow();
});
