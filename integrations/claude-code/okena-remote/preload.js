// Okena "Claude remote" preload. The Okena daemon puts it into every new pane
// as BUN_OPTIONS="--preload=<this file>", so it runs before any Bun program in
// that pane. For an interactive Claude Code session it appends the flags that
// connect the session to Okena: hooks that report to the daemon, and the MCP
// channel server that delivers remote prompts. Any other Bun program only
// drops the preload from what its children inherit. See README.md next to
// this file.
"use strict";

// Entry module of the Bun-compiled `claude` binary.
const CLAUDE_ENTRY = "/$bunfs/root/cli";

// The daemon writes this file as `.../claude-remote/preload.js`; the suffix
// also matches a copy reached through another path or an older daemon.
const OKENA_PRELOAD = /^--preload=.*[\\/]claude-remote[\\/]preload\.js$/;

// Flags that leave argv unchanged:
// - no interactive session in this pane: print mode, version, help, and the
//   modes that move the session elsewhere (background, Desktop app, cloud,
//   a new tmux session);
// - Claude accepts only one --settings: the user's own wins over ours.
// (--mcp-config is not here: Claude merges repeated ones.)
const GUARDED_FLAGS = new Set([
  "-p",
  "--print",
  "-v",
  "--version",
  "-h",
  "--help",
  "--bg",
  "--background",
  "--desktop",
  "--cloud",
  "--environment",
  "--tmux",
  "--settings",
]);

// Short forms of guarded flags. Commander accepts them combined (`-cp`).
const GUARDED_SHORT_FLAGS = new Set(["p", "v", "h"]);

// Short flags that take a value (`claude --help`): in a cluster, the rest of
// the argument is that value (`-dapi` is `--debug api`).
const VALUE_SHORT_FLAGS = new Set(["d", "n", "r", "w"]);

// From `claude --help` (Claude Code 2.1.301), aliases included.
const SUBCOMMANDS = new Set([
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
]);

const CHANNEL_SERVER = "okena-remote";

// Hook events that only report to the daemon; `null` = event without a matcher.
const EVENT_HOOKS = [
  ["SessionStart", null],
  ["UserPromptSubmit", null],
  ["PreToolUse", "*"],
  ["PostToolUse", "*"],
  ["PostToolUseFailure", "*"],
  ["Stop", null],
  ["StopFailure", null],
  ["SessionEnd", null],
  ["SubagentStart", null],
  ["SubagentStop", null],
];

// The permission hook waits until a remote client answers, or the terminal
// answers and the daemon releases it.
const PERMISSION_HOOK_TIMEOUT_SECS = 86400;
const EVENT_HOOK_TIMEOUT_SECS = 10;

function shellQuote(value) {
  return "'" + value.replace(/'/g, "'\\''") + "'";
}

function clusterHasGuardedFlag(cluster) {
  for (const flag of cluster) {
    if (GUARDED_SHORT_FLAGS.has(flag)) {
      return true;
    }
    if (VALUE_SHORT_FLAGS.has(flag)) {
      return false;
    }
  }
  return false;
}

function isGuardedFlag(arg) {
  const name = arg.startsWith("--") ? arg.split("=", 1)[0] : arg;
  if (GUARDED_FLAGS.has(name)) {
    return true;
  }
  if (/^-[A-Za-z]{2,}/.test(arg)) {
    return clusterHasGuardedFlag(arg.slice(1));
  }
  return false;
}

function hooksSettings(okenaBin) {
  const bin = shellQuote(okenaBin);
  const hooks = {
    PermissionRequest: [
      {
        matcher: "*",
        hooks: [
          {
            type: "command",
            command: `${bin} claude-remote hook permission`,
            timeout: PERMISSION_HOOK_TIMEOUT_SECS,
          },
        ],
      },
    ],
  };
  for (const [event, matcher] of EVENT_HOOKS) {
    const entry = {
      hooks: [
        {
          type: "command",
          command: `${bin} claude-remote hook event`,
          async: true,
          timeout: EVENT_HOOK_TIMEOUT_SECS,
        },
      ],
    };
    hooks[event] = [matcher === null ? entry : { matcher, ...entry }];
  }
  return { hooks };
}

function channelMcpConfig(okenaBin) {
  return {
    mcpServers: {
      [CHANNEL_SERVER]: { command: okenaBin, args: ["claude-remote", "channel"] },
    },
  };
}

function isClaudeBinary(argv, execPath) {
  return argv[1] === CLAUDE_ENTRY && String(execPath).includes("claude");
}

/**
 * `BUN_OPTIONS` without this preload: the remaining options, or undefined
 * when none remain. Bun splits the variable on whitespace, without quoting.
 */
function withoutOkenaPreload(bunOptions) {
  const rest = String(bunOptions ?? "")
    .split(/\s+/)
    .filter((arg) => arg !== "" && !OKENA_PRELOAD.test(arg));
  return rest.length === 0 ? undefined : rest.join(" ");
}

/**
 * Stops this process from passing the preload on. Bun copies `BUN_OPTIONS`
 * into `process.execArgv`, and programs copy either one to their children.
 */
function dropOkenaPreload(proc) {
  const bunOptions = withoutOkenaPreload(proc.env.BUN_OPTIONS);
  if (bunOptions === undefined) {
    delete proc.env.BUN_OPTIONS;
  } else {
    proc.env.BUN_OPTIONS = bunOptions;
  }
  const execArgv = proc.execArgv.filter((arg) => !OKENA_PRELOAD.test(arg));
  if (execArgv.length !== proc.execArgv.length) {
    proc.execArgv = execArgv;
  }
}

/**
 * The argv an interactive Claude Code session in an Okena pane should run
 * with, or null to leave `argv` unchanged.
 */
function okenaClaudeArgs(argv, env, execPath) {
  if (!isClaudeBinary(argv, execPath)) {
    return null;
  }
  const okenaBin = env.OKENA_BIN;
  if (!okenaBin || !env.OKENA_TERMINAL_ID || env.CLAUDECODE) {
    return null;
  }

  const separator = argv.indexOf("--", 2);
  const optionsEnd = separator === -1 ? argv.length : separator;
  const userArgs = argv.slice(2, optionsEnd);
  if (userArgs.some(isGuardedFlag)) {
    return null;
  }
  // Option values are not told apart from positionals (`--model opus mcp`),
  // so any subcommand name counts. A false match only skips the injection.
  if (userArgs.some((arg) => SUBCOMMANDS.has(arg))) {
    return null;
  }

  const added = ["--settings", JSON.stringify(hooksSettings(okenaBin))];
  if (env.OKENA_CLAUDE_REMOTE_CHANNEL === "1") {
    added.push(
      "--mcp-config",
      JSON.stringify(channelMcpConfig(okenaBin)),
      // Not also in --channels: the allowlist check then rejects the server.
      "--dangerously-load-development-channels",
      `server:${CHANNEL_SERVER}`,
    );
  }
  return [...argv.slice(0, optionsEnd), ...added, ...argv.slice(optionsEnd)];
}

try {
  if (isClaudeBinary(process.argv, process.execPath)) {
    // `claude` keeps the preload: it restarts itself (`--resume` relaunch,
    // update) with its own environment and must reconnect.
    const next = okenaClaudeArgs(process.argv, process.env, process.execPath);
    if (next) {
      process.argv.splice(0, process.argv.length, ...next);
      if (process.env.OKENA_CLAUDE_REMOTE_DEBUG === "1") {
        process.stderr.write(`okena-remote: argv ${JSON.stringify(process.argv)}\n`);
      }
    }
  } else {
    dropOkenaPreload(process);
  }
} catch {
  // A failing preload would break every Bun program in the pane.
}

if (typeof module !== "undefined") {
  module.exports = { okenaClaudeArgs, withoutOkenaPreload };
}
