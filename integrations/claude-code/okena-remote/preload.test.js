import { describe, expect, test } from "bun:test";

const { okenaClaudeArgs } = require("./preload.js");

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
      "SessionEnd",
      "SubagentStart",
      "SubagentStop",
    ]) {
      expect(settings.hooks[event]).toEqual([{ hooks: [eventHook] }]);
    }
    for (const event of ["PreToolUse", "PostToolUse", "PostToolUseFailure"]) {
      expect(settings.hooks[event]).toEqual([{ matcher: "*", hooks: [eventHook] }]);
    }
    expect(Object.keys(settings.hooks)).toHaveLength(10);
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
