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

// The daemon writes this file as `.../claude-remote/preload.js` to a path
// without whitespace; the suffix also matches a copy at another location.
const OKENA_PRELOAD = /^--preload=.*[\\/]claude-remote[\\/]preload\.js$/;
const OKENA_PRELOAD_OPTION = /(^|\s)--preload=\S*[\\/]claude-remote[\\/]preload\.js(?=\s|$)/g;

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

function parseJson(value) {
  try {
    return JSON.parse(value);
  } catch {
    return undefined;
  }
}

function isObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// By shape, not by today's JSON: a claude started by an older preload
// carries an older hook list.
function isOwnSettings(value, okenaBin) {
  const settings = parseJson(value);
  if (!isObject(settings) || Object.keys(settings).join() !== "hooks" || !isObject(settings.hooks)) {
    return false;
  }
  const prefix = `${shellQuote(okenaBin)} claude-remote hook `;
  const commands = [];
  for (const entries of Object.values(settings.hooks)) {
    for (const entry of Array.isArray(entries) ? entries : [null]) {
      for (const hook of Array.isArray(entry?.hooks) ? entry.hooks : [null]) {
        commands.push(hook?.command);
      }
    }
  }
  return (
    commands.length > 0 &&
    commands.every((command) => typeof command === "string" && command.startsWith(prefix))
  );
}

function isOwnMcpConfig(value, okenaBin) {
  const config = parseJson(value);
  if (!isObject(config) || Object.keys(config).join() !== "mcpServers" || !isObject(config.mcpServers)) {
    return false;
  }
  const servers = config.mcpServers;
  return Object.keys(servers).join() === CHANNEL_SERVER && servers[CHANNEL_SERVER]?.command === okenaBin;
}

/**
 * `args` without the flags an earlier run of this preload added. `claude`
 * restarts itself (`/restart`) with `--settings` and `--mcp-config` carried
 * over but without the development channel, so they are added again in full.
 */
function withoutOwnFlags(args, okenaBin) {
  const own = new Map([
    ["--settings", (value) => isOwnSettings(value, okenaBin)],
    ["--mcp-config", (value) => isOwnMcpConfig(value, okenaBin)],
    ["--dangerously-load-development-channels", (value) => value === `server:${CHANNEL_SERVER}`],
  ]);
  const rest = [];
  for (let i = 0; i < args.length; i++) {
    const eq = args[i].indexOf("=");
    const flag = eq === -1 ? args[i] : args[i].slice(0, eq);
    const isOwn = own.get(flag);
    if (isOwn && eq !== -1 && isOwn(args[i].slice(eq + 1))) {
      continue;
    }
    if (isOwn && eq === -1 && i + 1 < args.length && isOwn(args[i + 1])) {
      i++;
      continue;
    }
    rest.push(args[i]);
  }
  return rest;
}

function isClaudeBinary(argv, execPath) {
  return argv[1] === CLAUDE_ENTRY && String(execPath).includes("claude");
}

/**
 * `BUN_OPTIONS` without this preload, the rest kept as written (Bun honours
 * quoted values), or undefined when nothing else remains.
 */
function withoutOkenaPreload(bunOptions) {
  if (bunOptions === undefined) {
    return undefined;
  }
  const rest = bunOptions.replace(OKENA_PRELOAD_OPTION, "");
  if (rest === bunOptions) {
    return bunOptions;
  }
  return rest.trim() === "" ? undefined : rest.trim();
}

/**
 * Stops this process from passing the preload on. Bun copies `BUN_OPTIONS`
 * into `process.execArgv`, and programs copy either one to their children.
 */
function dropOkenaPreload(proc) {
  const bunOptions = withoutOkenaPreload(proc.env.BUN_OPTIONS);
  if (bunOptions === undefined) {
    delete proc.env.BUN_OPTIONS;
  } else if (bunOptions !== proc.env.BUN_OPTIONS) {
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
  const userArgs = withoutOwnFlags(argv.slice(2, optionsEnd), okenaBin);
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
  return [...argv.slice(0, 2), ...userArgs, ...added, ...argv.slice(optionsEnd)];
}

function run(proc) {
  if (!isClaudeBinary(proc.argv, proc.execPath)) {
    dropOkenaPreload(proc);
    return;
  }
  // `claude` keeps the preload: it restarts itself (`/restart`, `/update`)
  // with its own environment and must reconnect.
  const next = okenaClaudeArgs(proc.argv, proc.env, proc.execPath);
  if (next) {
    proc.argv.splice(0, proc.argv.length, ...next);
    if (proc.env.OKENA_CLAUDE_REMOTE_DEBUG === "1") {
      proc.stderr.write(`okena-remote: argv ${JSON.stringify(proc.argv)}\n`);
    }
  }
}

try {
  run(process);
} catch {
  // A failing preload would break every Bun program in the pane.
}

if (typeof module !== "undefined") {
  module.exports = { okenaClaudeArgs, run, withoutOkenaPreload };
}
