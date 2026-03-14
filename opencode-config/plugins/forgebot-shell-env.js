import { access } from "node:fs/promises";
import { constants as fs_constants } from "node:fs";
import { spawn } from "node:child_process";

const cache = new Map();
let init_logged = false;
let tool_hook_invocations = 0;

const SECRET_NAME_PATTERN = /(token|secret|password|key|auth|cookie|session)/i;

function log_debug(message, extra) {
  try {
    if (extra === undefined) {
      console.error(`[forgebot-shell-env] ${message}`);
      return;
    }
    console.error(`[forgebot-shell-env] ${message}`, extra);
  } catch {
    // no-op
  }
}

function redact_env_value(name, value) {
  if (typeof value !== "string") {
    return value;
  }

  if (SECRET_NAME_PATTERN.test(name)) {
    return `<redacted:${value.length}>`;
  }

  if (name === "PATH") {
    return value;
  }

  if (value.length > 160) {
    return `${value.slice(0, 160)}...<len:${value.length}>`;
  }

  return value;
}

function log_loaded_env_vars(cwd, env) {
  const keys = Object.keys(env).sort();
  log_debug("environment variable keys loaded", { cwd, count: keys.length, keys });

  for (const key of keys) {
    log_debug("environment variable loaded", {
      cwd,
      key,
      value: redact_env_value(key, env[key]),
    });
  }
}

function shell_quote(value) {
  return `'${String(value).replace(/'/g, `'"'"'`)}'`;
}

function command_preview(command) {
  if (typeof command !== "string") {
    return "<non-string>";
  }
  if (command.length <= 200) {
    return command;
  }
  return `${command.slice(0, 200)}...<len:${command.length}>`;
}

function resolve_tool_cwd(input, output) {
  if (typeof input?.cwd === "string" && input.cwd.length > 0) {
    return input.cwd;
  }
  if (typeof input?.args?.cwd === "string" && input.args.cwd.length > 0) {
    return input.args.cwd;
  }
  if (typeof output?.args?.cwd === "string" && output.args.cwd.length > 0) {
    return output.args.cwd;
  }
  return null;
}

async function path_exists(path) {
  try {
    await access(path, fs_constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

function run_json_command(command, args, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd,
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";

    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });

    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });

    child.on("error", (error) => {
      reject(error);
    });

    child.on("close", (code) => {
      if (code !== 0) {
        reject(
          new Error(
            `${command} ${args.join(" ")} failed with exit code ${code}: ${stderr.trim()}`,
          ),
        );
        return;
      }

      try {
        resolve(JSON.parse(stdout));
      } catch (error) {
        reject(new Error(`failed to parse ${command} JSON output: ${error}`));
      }
    });
  });
}

async function load_direnv(cwd) {
  const raw = await run_json_command("direnv", ["export", "json"], cwd);
  const env = {};

  for (const [key, value] of Object.entries(raw)) {
    if (typeof value === "string") {
      env[key] = value;
    }
  }

  return env;
}

async function load_nix(cwd) {
  const raw = await run_json_command("nix", ["print-dev-env", "--json"], cwd);
  const env = {};

  const variables = raw.variables;
  if (!variables || typeof variables !== "object") {
    return env;
  }

  for (const [key, variable] of Object.entries(variables)) {
    if (
      variable &&
      typeof variable === "object" &&
      variable.type === "exported" &&
      typeof variable.value === "string"
    ) {
      env[key] = variable.value;
    }
  }

  return env;
}

async function detect_and_load(cwd) {
  if (await path_exists(`${cwd}/.envrc`)) {
    log_debug("detected .envrc; using direnv", { cwd });
    return load_direnv(cwd);
  }

  if (
    (await path_exists(`${cwd}/flake.nix`)) ||
    (await path_exists(`${cwd}/shell.nix`)) ||
    (await path_exists(`${cwd}/default.nix`))
  ) {
    log_debug("detected nix shell files; using nix print-dev-env", { cwd });
    return load_nix(cwd);
  }

  log_debug("no env loader files detected", { cwd });
  return {};
}

async function load_env_for_cwd(cwd) {
  let env = cache.get(cwd);
  if (!env) {
    try {
      env = await detect_and_load(cwd);
      log_debug("loaded environment values", {
        cwd,
        keys: Object.keys(env).length,
        has_path: typeof env.PATH === "string" && env.PATH.length > 0,
      });
      log_loaded_env_vars(cwd, env);
    } catch {
      log_debug("failed to load environment values; using empty env", { cwd });
      env = {};
    }
    cache.set(cwd, env);
  }
  return env;
}

export const ForgebotShellEnv = async () => {
  log_debug("plugin initialized");
  return {
    "shell.env": async (input, output) => {
      if (!input.cwd || typeof input.cwd !== "string") {
        return;
      }

      if (!init_logged) {
        log_debug("shell.env hook invoked", { cwd: input.cwd });
        init_logged = true;
      }

      const env = await load_env_for_cwd(input.cwd);

      output.env = {
        ...output.env,
        ...env,
        FORGEBOT_SHELL_ENV_PLUGIN_ACTIVE: "1",
      };
    },
    "tool.execute.before": async (input, output) => {
      if (input.tool !== "bash") {
        return;
      }

      const cwd = resolve_tool_cwd(input, output);
      if (!cwd) {
        log_debug("tool.execute.before missing cwd; skipping PATH override", {
          input_cwd: input.cwd,
        });
        return;
      }

      const original_command = output?.args?.command;
      if (typeof original_command !== "string") {
        log_debug("tool.execute.before command missing; skipping PATH override", {
          cwd,
          command_type: typeof original_command,
        });
        return;
      }

      const env = await load_env_for_cwd(cwd);
      const path_value = env.PATH;
      if (typeof path_value !== "string" || path_value.length === 0) {
        log_debug("tool.execute.before no PATH from env loader; skipping", {
          cwd,
          env_keys: Object.keys(env).length,
        });
        return;
      }

      const rewritten_command =
        `export PATH=${shell_quote(path_value)}; export FORGEBOT_SHELL_ENV_PATH_OVERRIDE=1; ` +
        original_command;

      output.args.command = rewritten_command;
      tool_hook_invocations += 1;

      log_debug("tool.execute.before rewrote bash command with PATH override", {
        cwd,
        invocation: tool_hook_invocations,
        path_prefix: path_value.slice(0, 180),
        original_command: command_preview(original_command),
        rewritten_preview: command_preview(rewritten_command),
      });
    },
  };
};
