import { access } from "node:fs/promises";
import { constants as fs_constants } from "node:fs";
import { spawn } from "node:child_process";

const cache = new Map();
let init_logged = false;

const SECRET_NAME_PATTERN = /(token|secret|password|key|auth|cookie|session)/i;
const DEFAULT_ASKPASS_PATH = "/var/lib/forgebot/git-askpass.sh";
const NIX_ENV_BLOCKLIST = new Set([
  "HOME",
  "TMP",
  "TEMP",
  "TMPDIR",
  "TEMPDIR",
  "NIX_BUILD_TOP",
  "__structuredAttrs",
  "out",
  "outputs",
  "builder",
  "buildPhase",
  "phases",
  "buildInputs",
  "nativeBuildInputs",
  "propagatedBuildInputs",
  "propagatedNativeBuildInputs",
  "configureFlags",
  "cmakeFlags",
  "mesonFlags",
  "patches",
  "strictDeps",
  "preferLocalBuild",
  "depsBuildBuild",
  "depsBuildBuildPropagated",
  "depsBuildTarget",
  "depsBuildTargetPropagated",
  "depsHostHost",
  "depsHostHostPropagated",
  "depsTargetTarget",
  "depsTargetTargetPropagated",
  "doCheck",
  "doInstallCheck",
  "name",
]);

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

function build_always_injected_env() {
  const askpass = process.env.FORGEBOT_ASKPASS_PATH || DEFAULT_ASKPASS_PATH;

  return {
    GIT_ASKPASS: askpass,
    SSH_ASKPASS: askpass,
    GIT_TERMINAL_PROMPT: "0",
    FORGEBOT_GIT_ASKPASS_ACTIVE: "1",
  };
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

async function load_nix(cwd) {
  const raw = await run_json_command("nix", ["print-dev-env", "--json"], cwd);
  const env = {};
  const filtered_keys = [];

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
      if (NIX_ENV_BLOCKLIST.has(key)) {
        filtered_keys.push(key);
        continue;
      }
      env[key] = variable.value;
    }
  }

  if (filtered_keys.length > 0) {
    log_debug("filtered nix build/sandbox variables", {
      cwd,
      count: filtered_keys.length,
      keys: filtered_keys.sort(),
    });
  }

  return env;
}

async function detect_and_load(cwd) {
  const has_nix_files =
    (await path_exists(`${cwd}/flake.nix`)) ||
    (await path_exists(`${cwd}/shell.nix`)) ||
    (await path_exists(`${cwd}/default.nix`));

  if (await path_exists(`${cwd}/.envrc`)) {
    log_debug("detected .envrc; direnv disabled for POC", { cwd });
  }

  if (has_nix_files) {
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
      const always_env = build_always_injected_env();

      output.env = {
        ...output.env,
        ...env,
        ...always_env,
        FORGEBOT_SHELL_ENV_PLUGIN_ACTIVE: "1",
      };

      log_debug("injected always-on git auth env", {
        cwd: input.cwd,
        git_askpass: always_env.GIT_ASKPASS,
      });
    },
  };
};
