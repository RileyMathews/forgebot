import { access } from "node:fs/promises";
import { constants as fs_constants } from "node:fs";
import { spawn } from "node:child_process";

const cache = new Map();
let init_logged = false;

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

      let env = cache.get(input.cwd);
      if (!env) {
        try {
          env = await detect_and_load(input.cwd);
          log_debug("loaded environment values", {
            cwd: input.cwd,
            keys: Object.keys(env).length,
          });
        } catch {
          log_debug("failed to load environment values; using empty env", {
            cwd: input.cwd,
          });
          env = {};
        }
        cache.set(input.cwd, env);
      }

      output.env = {
        ...output.env,
        ...env,
        FORGEBOT_SHELL_ENV_PLUGIN_ACTIVE: "1",
      };
    },
  };
};
