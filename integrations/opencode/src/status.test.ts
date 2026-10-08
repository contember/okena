import { expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { reportStatus } from "./status.js";

test("status follows the current TTY pointer after reattachment and carries session identity", async () => {
  const directory = await mkdtemp(join(tmpdir(), "okena-status-"));
  try {
    const first = join(directory, "first");
    const second = join(directory, "second");
    const pointer = join(directory, "pointer");
    await writeFile(first, "");
    await writeFile(second, "");
    await writeFile(pointer, first + "\n");
    const pane = {
      terminalID: "terminal-1",
      profile: "default",
      configHome: null,
      ttyFile: pointer,
      tty: first,
    };
    const id = "ses_0123456789abABCDEFGHIJKLMN";
    await reportStatus(pane, id, "working");
    const working = await readFile(first, "utf8");
    const label = Buffer.from(
      JSON.stringify({ agent: "opencode", session_id: id }),
    ).toString("base64");
    expect(working).toBe(
      `\x1b]9001;st=working;tid=terminal-1;lbl=${label}\x1b\\`,
    );
    await writeFile(pointer, second + "\n");
    await reportStatus(pane, id, "done");
    expect(await readFile(first, "utf8")).toBe(working);
    expect(await readFile(second, "utf8")).toBe(
      `\x1b]9001;st=done;tid=terminal-1;lbl=${label}\x1b\\`,
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
