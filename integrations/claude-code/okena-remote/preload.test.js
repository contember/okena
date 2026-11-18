import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const { okenaClaudeArgs, run, withoutOkenaPreload } = require("./preload.js");

const EXEC_PATH = "/home/u/.local/share/claude/versions/2.1.301";
const OKENA_BIN = "/opt/okena/okena";
const ENV = {
  OKENA_BIN,
  OKENA_TERMINAL_ID: "term-1",
  OKENA_CLAUDE_REMOTE_CHANNEL: "1",
};

function claudeArgv(...args) {
  return ["bun", "/$bunfs/root/cli", ...args];
}

function flagValue(argv, flag) {
  const index = argv.indexOf(flag);
  return index === -1 ? undefined : argv[index + 1];
}

describe("injection", () => {
  test("appends hooks and the channel to a plain interactive run", () => {
    const next = okenaClaudeArgs(claudeArgv(), ENV, EXEC_PATH);

    expect(next.slice(0, 2)).toEqual(claudeArgv());
    expect(next.slice(2)).toEqual([
      "--settings",
      expect.any(String),
      "--mcp-config",
      expect.any(String),
      "--dangerously-load-development-channels",
      "server:okena-remote",
    ]);
  });

  test("hooks call `okena claude-remote hook` for every event", () => {
    const settings = JSON.parse(flagValue(okenaClaudeArgs(claudeArgv(), ENV, EXEC_PATH), "--settings"));

    expect(settings.hooks.PermissionRequest).toEqual([
      {
        matcher: "*",
        hooks: [
          {
            type: "command",
            command: "'/opt/okena/okena' claude-remote hook permission",
            timeout: 86400,
          },
        ],
      },
    ]);
    const eventHook = {
      type: "command",
      command: "'/opt/okena/okena' claude-remote hook event",
      async: true,
      timeout: 10,
    };
    for (const event of [
      "SessionStart",
      "UserPromptSubmit",
      "Stop",
      "StopFailure",
      "SessionEnd",
      "SubagentStart",
      "SubagentStop",
    ]) {
      expect(settings.hooks[event]).toEqual([{ hooks: [eventHook] }]);
    }
    for (const event of ["PreToolUse", "PostToolUse", "PostToolUseFailure"]) {
      expect(settings.hooks[event]).toEqual([{ matcher: "*", hooks: [eventHook] }]);
    }
    expect(Object.keys(settings.hooks)).toHaveLength(11);
  });

  test("quotes OKENA_BIN for sh", () => {
    const env = { ...ENV, OKENA_BIN: "/opt/it's okena/okena" };
    const settings = JSON.parse(flagValue(okenaClaudeArgs(claudeArgv(), env, EXEC_PATH), "--settings"));

    expect(settings.hooks.Stop[0].hooks[0].command).toBe(
      "'/opt/it'\\''s okena/okena' claude-remote hook event",
    );
  });

  test("the user's own --mcp-config is kept and ours is added", () => {
    for (const userFlag of [["--mcp-config", "servers.json"], ["--mcp-config=servers.json"]]) {
      const next = okenaClaudeArgs(claudeArgv(...userFlag), ENV, EXEC_PATH);

      expect(next.slice(0, 2 + userFlag.length)).toEqual(claudeArgv(...userFlag));
      expect(next.filter((arg) => arg.startsWith("--mcp-config"))).toHaveLength(2);
      expect(next).toContain("--dangerously-load-development-channels");
    }
  });

  test("the MCP config runs the channel server from OKENA_BIN", () => {
    const next = okenaClaudeArgs(claudeArgv(), ENV, EXEC_PATH);

    expect(JSON.parse(flagValue(next, "--mcp-config"))).toEqual({
      mcpServers: {
        "okena-remote": { command: OKENA_BIN, args: ["claude-remote", "channel"] },
      },
    });
    expect(next).not.toContain("--channels");
  });

  test("without the channel setting only hooks are added", () => {
    for (const channel of ["0", undefined]) {
      const env = { ...ENV, OKENA_CLAUDE_REMOTE_CHANNEL: channel };
      const next = okenaClaudeArgs(claudeArgv("--model", "opus"), env, EXEC_PATH);

      expect(next).toEqual([...claudeArgv("--model", "opus"), "--settings", expect.any(String)]);
    }
  });

  test("keeps the user's arguments and appends after them", () => {
    const next = okenaClaudeArgs(claudeArgv("-c", "--model", "opus", "fix the tests"), ENV, EXEC_PATH);

    expect(next.slice(0, 6)).toEqual(claudeArgv("-c", "--model", "opus", "fix the tests"));
    expect(next[6]).toBe("--settings");
  });

  test("inserts before a `--` separator", () => {
    const next = okenaClaudeArgs(claudeArgv("--resume", "abc", "--", "-p looks like a flag"), ENV, EXEC_PATH);

    expect(next.slice(0, 4)).toEqual(claudeArgv("--resume", "abc"));
    expect(next[4]).toBe("--settings");
    expect(next.slice(-2)).toEqual(["--", "-p looks like a flag"]);
  });

  test("a subcommand name after `--` is a prompt, not a subcommand", () => {
    expect(okenaClaudeArgs(claudeArgv("--", "doctor"), ENV, EXEC_PATH)).not.toBeNull();
  });

  // `/restart` carries our --settings and --mcp-config but drops the channel.
  test("a restart that carried some of our flags gets all of them once", () => {
    const first = okenaClaudeArgs(claudeArgv("--model", "haiku"), ENV, EXEC_PATH);
    const ours = first.slice(4);
    const settings = flagValue(first, "--settings");
    const mcpConfig = flagValue(first, "--mcp-config");

    for (const carried of [
      ["--settings", settings, "--mcp-config", mcpConfig],
      [`--settings=${settings}`, `--mcp-config=${mcpConfig}`],
      ours,
    ]) {
      const restart = claudeArgv("--resume", "abc", ...carried, "--model", "haiku");

      expect(okenaClaudeArgs(restart, ENV, EXEC_PATH)).toEqual([
        ...claudeArgv("--resume", "abc", "--model", "haiku"),
        ...ours,
      ]);
    }
  });

  test("flags carried from an older preload are replaced", () => {
    const first = okenaClaudeArgs(claudeArgv(), ENV, EXEC_PATH);
    const older = JSON.parse(flagValue(first, "--settings"));
    delete older.hooks.StopFailure;
    const restart = claudeArgv("--resume", "abc", "--settings", JSON.stringify(older));

    expect(okenaClaudeArgs(restart, ENV, EXEC_PATH)).toEqual([...claudeArgv("--resume", "abc"), ...first.slice(2)]);
  });

  test("a restart with the channel now off drops our carried MCP config", () => {
    const first = okenaClaudeArgs(claudeArgv(), ENV, EXEC_PATH);
    const restart = claudeArgv(
      "--resume",
      "abc",
      "--settings",
      flagValue(first, "--settings"),
      "--mcp-config",
      flagValue(first, "--mcp-config"),
    );
    const env = { ...ENV, OKENA_CLAUDE_REMOTE_CHANNEL: "0" };

    expect(okenaClaudeArgs(restart, env, EXEC_PATH)).toEqual([
      ...claudeArgv("--resume", "abc"),
      "--settings",
      flagValue(first, "--settings"),
    ]);
  });

  test("settings or MCP config with anything else in it are the user's own", () => {
    const first = okenaClaudeArgs(claudeArgv(), ENV, EXEC_PATH);
    const settings = JSON.parse(flagValue(first, "--settings"));
    const mcpConfig = JSON.parse(flagValue(first, "--mcp-config"));
    const userSettings = [
      { ...settings, model: "opus" },
      { hooks: { ...settings.hooks, Stop: [{ hooks: [{ type: "command", command: "notify-send done" }] }] } },
      { hooks: {} },
    ];
    for (const user of userSettings) {
      expect(okenaClaudeArgs(claudeArgv("--settings", JSON.stringify(user)), ENV, EXEC_PATH)).toBeNull();
    }

    const userMcp = { mcpServers: { ...mcpConfig.mcpServers, github: { command: "gh-mcp" } } };
    const next = okenaClaudeArgs(claudeArgv("--mcp-config", JSON.stringify(userMcp)), ENV, EXEC_PATH);
    expect(next.slice(2, 4)).toEqual(["--mcp-config", JSON.stringify(userMcp)]);
    expect(next.filter((arg) => arg === "--mcp-config")).toHaveLength(2);
  });

  test("injecting twice adds nothing", () => {
    const first = okenaClaudeArgs(claudeArgv("-c"), ENV, EXEC_PATH);

    expect(okenaClaudeArgs(first, ENV, EXEC_PATH)).toEqual(first);
  });

  test("settings for another okena binary are the user's own", () => {
    const other = okenaClaudeArgs(claudeArgv(), { ...ENV, OKENA_BIN: "/other/okena" }, EXEC_PATH);

    expect(okenaClaudeArgs(other, ENV, EXEC_PATH)).toBeNull();
  });

  test("does not mutate the input", () => {
    const argv = claudeArgv("--model", "opus");
    okenaClaudeArgs(argv, ENV, EXEC_PATH);

    expect(argv).toEqual(claudeArgv("--model", "opus"));
  });
});

describe("guards", () => {
  test("another Bun program", () => {
    expect(okenaClaudeArgs(["bun", "/home/u/app/index.ts"], ENV, "/home/u/.bun/bin/bun")).toBeNull();
    expect(okenaClaudeArgs(claudeArgv(), ENV, "/usr/local/bin/my-compiled-tool")).toBeNull();
  });

  test("missing Okena environment", () => {
    expect(okenaClaudeArgs(claudeArgv(), { ...ENV, OKENA_BIN: undefined }, EXEC_PATH)).toBeNull();
    expect(okenaClaudeArgs(claudeArgv(), { ...ENV, OKENA_BIN: "" }, EXEC_PATH)).toBeNull();
    expect(okenaClaudeArgs(claudeArgv(), { ...ENV, OKENA_TERMINAL_ID: undefined }, EXEC_PATH)).toBeNull();
  });

  test("claude spawned by claude", () => {
    expect(okenaClaudeArgs(claudeArgv(), { ...ENV, CLAUDECODE: "1" }, EXEC_PATH)).toBeNull();
  });

  test.each([
    ["-p"],
    ["--print"],
    ["-v"],
    ["--version"],
    ["-h"],
    ["--help"],
    ["--bg"],
    ["--background"],
    ["--desktop"],
    ["--cloud"],
    ["--tmux"],
    ["--tmux=classic"],
    ["--environment"],
    ["--environment=env_123"],
    ["--settings"],
    ["--settings={}"],
    ["-cp"],
    ["-cv"],
    ["-ch"],
  ])("flag %s", (flag) => {
    expect(okenaClaudeArgs(claudeArgv("--model", "opus", flag, "hi"), ENV, EXEC_PATH)).toBeNull();
  });

  test("short flag clusters without a guarded flag are injected", () => {
    expect(okenaClaudeArgs(claudeArgv("-cw"), ENV, EXEC_PATH)).not.toBeNull();
  });

  test.each(["-dapi", "-nhelper", "-rprev", "-wpatch", "-cdp", "-cnhi"])(
    "the value of a short flag in %s is not read as flags",
    (cluster) => {
      expect(okenaClaudeArgs(claudeArgv(cluster), ENV, EXEC_PATH)).not.toBeNull();
    },
  );

  test.each([
    "agents",
    "attach",
    "auth",
    "auto-mode",
    "doctor",
    "gateway",
    "import",
    "install",
    "logs",
    "mcp",
    "plugin",
    "plugins",
    "purge",
    "respawn",
    "rm",
    "setup-token",
    "stop",
    "kill",
    "ultrareview",
    "update",
    "upgrade",
  ])("subcommand %s", (subcommand) => {
    expect(okenaClaudeArgs(claudeArgv(subcommand, "list"), ENV, EXEC_PATH)).toBeNull();
    expect(okenaClaudeArgs(claudeArgv("--verbose", subcommand), ENV, EXEC_PATH)).toBeNull();
    expect(okenaClaudeArgs(claudeArgv("--model", "opus", subcommand, "list"), ENV, EXEC_PATH)).toBeNull();
  });

  test("a prompt that is not a subcommand is injected", () => {
    expect(okenaClaudeArgs(claudeArgv("update the docs"), ENV, EXEC_PATH)).not.toBeNull();
  });

  test("guarded flags after `--` are prompt text", () => {
    expect(okenaClaudeArgs(claudeArgv("--", "--help"), ENV, EXEC_PATH)).not.toBeNull();
  });
});

describe("withoutOkenaPreload", () => {
  const PRELOAD = "--preload=/home/u/.config/okena/profiles/default/claude-remote/preload.js";

  test("removes the preload and keeps inherited options as written", () => {
    expect(withoutOkenaPreload(`${PRELOAD} --smol`)).toBe("--smol");
    expect(withoutOkenaPreload(`--smol ${PRELOAD} --inspect`)).toBe("--smol --inspect");
    expect(withoutOkenaPreload(`${PRELOAD} --title="a   b"`)).toBe('--title="a   b"');
  });

  test("options without the preload are returned unchanged", () => {
    expect(withoutOkenaPreload("--smol   --inspect")).toBe("--smol   --inspect");
    expect(withoutOkenaPreload("")).toBe("");
  });

  test("nothing left is undefined", () => {
    expect(withoutOkenaPreload(PRELOAD)).toBeUndefined();
    expect(withoutOkenaPreload(` ${PRELOAD}  ${PRELOAD} `)).toBeUndefined();
    expect(withoutOkenaPreload(undefined)).toBeUndefined();
  });

  test("matches any location of the daemon's file, Windows paths included", () => {
    expect(withoutOkenaPreload("--preload=/run/user/1000/okena/work/claude-remote/preload.js")).toBeUndefined();
    expect(withoutOkenaPreload("--preload=C:\\Users\\u\\okena\\claude-remote\\preload.js")).toBeUndefined();
  });

  test("keeps other preloads", () => {
    for (const other of ["--preload=/app/preload.js", "--preload=/app/claude-remote/preload.ts", "-r", "./setup.js"]) {
      expect(withoutOkenaPreload(`${PRELOAD} ${other}`)).toBe(other);
    }
  });
});

// Bun loads the preload from BUN_OPTIONS here exactly as in an Okena pane.
describe("children of a Bun program in a pane", () => {
  let dir;
  let parentScript;
  let preloadArg;

  const CHILD = "console.log(JSON.stringify({ execArgv: process.execArgv, bunOptions: process.env.BUN_OPTIONS ?? null }));";

  // How programs start Bun children: env built from process.env, the default
  // child_process env, and a supervisor that copies its own execArgv.
  const PARENT = `
const { spawnSync } = require("node:child_process");
const child = process.argv[2];
const run = (args, options) => JSON.parse(spawnSync(process.execPath, [...args, child], { encoding: "utf8", ...options }).stdout);
console.log(JSON.stringify({
  self: process.execArgv,
  envCopy: run([], { env: { ...process.env } }),
  inherited: run([]),
  execArgvCopy: run(process.execArgv, { env: { ...process.env } }),
}));
`;

  beforeAll(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), "okena-preload-"));
    const preload = path.join(dir, "claude-remote", "preload.js");
    fs.mkdirSync(path.dirname(preload));
    fs.copyFileSync(path.join(import.meta.dir, "preload.js"), preload);
    fs.writeFileSync(path.join(dir, "child.js"), CHILD);
    parentScript = path.join(dir, "parent.js");
    fs.writeFileSync(parentScript, PARENT);
    preloadArg = `--preload=${preload}`;
  });

  afterAll(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  function runParent(bunOptions) {
    const result = Bun.spawnSync([process.execPath, parentScript, path.join(dir, "child.js")], {
      env: { ...process.env, BUN_OPTIONS: bunOptions, OKENA_BIN: "/opt/okena/okena", OKENA_TERMINAL_ID: "term-1" },
      cwd: dir,
    });
    expect(result.stderr.toString()).toBe("");
    return JSON.parse(result.stdout.toString());
  }

  test("children get neither the preload nor an empty BUN_OPTIONS", () => {
    const out = runParent(preloadArg);

    expect(out.self).toEqual([]);
    for (const child of [out.envCopy, out.inherited, out.execArgvCopy]) {
      expect(child).toEqual({ execArgv: [], bunOptions: null });
    }
  });

  test("inherited runtime flags still reach the children", () => {
    const out = runParent(`${preloadArg} --smol`);

    expect(out.self).toEqual(["--smol"]);
    expect(out.envCopy).toEqual({ execArgv: ["--smol"], bunOptions: "--smol" });
    expect(out.inherited).toEqual({ execArgv: ["--smol"], bunOptions: "--smol" });
    expect(out.execArgvCopy.execArgv).toEqual(["--smol", "--smol"]);
  });
});

describe("run", () => {
  const PRELOAD = "--preload=/home/u/.config/okena/profiles/default/claude-remote/preload.js";

  function fakeProcess(argv, execPath) {
    return { argv, execPath, execArgv: [PRELOAD, "--smol"], env: { ...ENV, BUN_OPTIONS: `${PRELOAD} --smol` } };
  }

  test("claude keeps the preload for its restart and gets the flags", () => {
    const proc = fakeProcess(claudeArgv("--model", "opus"), EXEC_PATH);
    run(proc);

    expect(proc.argv).toEqual(okenaClaudeArgs(claudeArgv("--model", "opus"), ENV, EXEC_PATH));
    expect(proc.env.BUN_OPTIONS).toBe(`${PRELOAD} --smol`);
    expect(proc.execArgv).toEqual([PRELOAD, "--smol"]);
  });

  test("claude spawned by claude keeps the preload too", () => {
    const proc = fakeProcess(claudeArgv(), EXEC_PATH);
    proc.env.CLAUDECODE = "1";
    run(proc);

    expect(proc.argv).toEqual(claudeArgv());
    expect(proc.env.BUN_OPTIONS).toBe(`${PRELOAD} --smol`);
  });

  test("another Bun program drops it", () => {
    const proc = fakeProcess(["bun", "/home/u/app/index.ts"], "/home/u/.bun/bin/bun");
    run(proc);

    expect(proc.argv).toEqual(["bun", "/home/u/app/index.ts"]);
    expect(proc.env.BUN_OPTIONS).toBe("--smol");
    expect(proc.execArgv).toEqual(["--smol"]);
  });
});
