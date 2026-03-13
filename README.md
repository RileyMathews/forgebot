# forgebot

A self-hosted agentic coding bridge between [Forgejo](https://forgejo.org/) and [opencode](https://github.com/sst/opencode). forgebot listens for Forgejo webhooks and orchestrates opencode sessions to automatically plan and implement work from Forgejo issues.

## Overview

forgebot acts as a webhook receiver and session manager. When you mention `@forgebot` in a Forgejo issue comment, forgebot creates an isolated git worktree, invokes opencode with the issue context, and the agent produces either a detailed implementation plan or working code with a pull request. Each issue maintains its own session context across multiple interactions, preserving state between planning, building, and revision phases.

## Prerequisites

Before installing forgebot, ensure you have:

- A running **Forgejo instance** (self-hosted or compatible)
- **opencode** installed and available in your `PATH`
- **forgejo-mcp** installed and available in your `PATH` (NixOS module provides this by default)
- **NixOS** or **Linux** — NixOS is the primary deployment target; other Linux distributions work via manual binary deployment
- **Nix** with flakes enabled (if using the Nix environment loader feature)
- **Git** installed (for worktree management)
- **sqlite3** CLI (optional, useful for debugging the database)

## Quick Start (Primary Path: NixOS)

Follow these steps in order to deploy forgebot on NixOS:

### 1. Add forgebot to your NixOS flake inputs

In your `flake.nix`:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    
    forgebot = {
      url = "github:rileymathews/forgebot";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  
  # ... rest of your configuration
}
```

### 2. Import the NixOS module and configure

In your `configuration.nix`:

```nix
{ forgebot, ... }:
{
  imports = [
    forgebot.nixosModules.forgebot
  ];
  
  services.forgebot = {
    enable = true;
    forgejo.url = "https://git.example.com";
    secretsFilePath = "/run/secrets/forgebot";
  };
}
```

**For production deployments with sops-nix secret management:**

```nix
{ config, forgebot, ... }:
{
  imports = [
    forgebot.nixosModules.forgebot
  ];
  
  sops.secrets.forgebot = { };
  
  services.forgebot = {
    enable = true;
    forgejo.url = "https://git.example.com";
    secretsFilePath = config.sops.secrets.forgebot.path;

    # OPTIONAL: Override forgejo-mcp package used by opencode MCP integration
    # forgejoMcpPackage = pkgs.forgejo-mcp;
  };
}
```

**Optional server configuration** (if you want to customize):

```nix
services.forgebot = {
  enable = true;
  
  # Optional - customize server config (shown with defaults)
  server.host = "127.0.0.1";
  server.port = 8765;
  server.forgeBotHost = null;  # Set to public URL for production (e.g., "https://forgebot.example.com")

  # Optional - run opencode as a host web service
  opencodeWebServer.enabled = true;
  opencodeWebServer.port = 4096;
  opencodeWebServer.host = "http://127.0.0.1:4096";  # Used for session Web UI links
  forgejo.url = "https://git.example.com";
  
  # Required - path to secrets file
  secretsFilePath = "/run/secrets/forgebot";
};
```

The secrets file must contain:

```
FORGEBOT_WEBHOOK_SECRET=your-webhook-secret-here
FORGEBOT_FORGEJO_TOKEN=your-forgejo-api-token
```

### 2.1 Authenticate OpenCode via Web UI

forgebot no longer requires an `auth.json` bootstrap file. OpenCode authentication is handled directly in the OpenCode Web UI.

To obtain your OpenCode Zen API key:
1. Visit [OpenCode Zen](https://zen.opencode.ai) and sign up/login
2. Add billing details to your account
3. Generate an API key from your dashboard
4. Copy the key (starts with `sk-`)

**Update your NixOS configuration** (no credentials file option needed):

```nix
{ config, forgebot, ... }:
{
  imports = [
    forgebot.nixosModules.forgebot
  ];
  
  sops.secrets.forgebot = { };
  services.forgebot = {
    enable = true;
    forgejo.url = "https://git.example.com";
    secretsFilePath = config.sops.secrets.forgebot.path;

    # OPTIONAL: Choose a different model (default is opencode/kimi-k2.5)
    # opencode.model = "opencode/claude-sonnet-4-5";  # Better quality, higher cost
    # opencode.model = "opencode/claude-opus-4-6";  # Best quality, most expensive
    # opencode.model = "opencode/gpt-5";            # OpenAI via Zen
  };
}
```

After deployment, open the OpenCode Web UI and sign in there.

**Available models** (run `opencode models` to see all):
- `opencode/kimi-k2.5` (default) - Fast, cost-effective
- `opencode/claude-sonnet-4-5` - Balanced performance and cost
- `opencode/claude-opus-4-6` - Best quality, higher cost
- `opencode/gpt-5` - OpenAI GPT-5 via Zen

All other configuration values use sensible defaults:
- `FORGEBOT_SERVER_HOST`: `127.0.0.1`
- `FORGEBOT_SERVER_PORT`: `8765`
- `FORGEBOT_FORGEBOT_HOST`: `http://<server_host>:<server_port>` (see note below)
- `FORGEBOT_FORGEJO_BOT_USERNAME`: `forgebot`
- `FORGEBOT_OPENCODE_BINARY`: `opencode`
- `FORGEBOT_OPENCODE_WORKTREE_BASE`: `/var/lib/forgebot/worktrees`
- `FORGEBOT_OPENCODE_CONFIG_DIR`: `/var/lib/forgebot/opencode-config`
- `FORGEBOT_OPENCODE_MODEL`: `opencode/kimi-k2.5`
- `FORGEBOT_OPENCODE_API_BASE_URL`: `http://127.0.0.1:4096`
- `FORGEBOT_OPENCODE_API_TIMEOUT_SECS`: `30`
- `FORGEBOT_OPENCODE_WEB_HOST`: *(unset)* (when set, forgebot posts session Web UI links)
- `FORGEBOT_DATABASE_PATH`: `/var/lib/forgebot/forgebot.db`

**Important**: `FORGEBOT_FORGEBOT_HOST` should be set to your public-facing URL for production deployments (e.g., `https://forgebot.example.com`). If not set, it defaults to `http://<server_host>:<server_port>`, which may not be accessible from the internet if the server is bound to localhost.

### 3. Apply the configuration

```bash
sudo nixos-rebuild switch
```

The forgebot module will:
1. Create the forgebot user and group
2. Set up the data directory structure
3. Start the forgebot service with environment variables configured

### 4. Verify startup

Check the service logs for successful initialization:

```bash
journalctl -u forgebot -f
```

You should see messages indicating the database is initialized, migrations completed, and the server is listening.

### 5. Register repositories

Navigate to the setup UI at `http://<host>:8765/ui`:

1. Click **"Add Repository"**
2. Enter:
   - **Full repository name**: e.g., `alice/myrepo`
   - **Default branch**: e.g., `main`
   - **Environment loader**: Choose from None, direnv, or Nix
3. Click **"Add Repository"**

This inserts the repository into the database. The repo will appear in the list with an **"Incomplete"** status.

### 6. Clone watched repositories

For each registered repository, the setup page at `/ui/repo/:owner/:name` shows the exact clone command:

```bash
git clone --bare https://git.example.com/<owner>/<repo>.git <worktree_base>/<owner>_<repo>
```

**Important**: This step must be done manually. forgebot requires the local bare clone to create worktrees but will not clone repositories itself. The UI will show **"Missing"** until the clone is present.

Example:

```bash
cd /var/lib/forgebot/worktrees
git clone --bare https://git.example.com/alice/myrepo.git alice_myrepo
```

### 7. Complete repository setup

Back in the UI at `/ui/repo/:owner/:name`, complete all four steps:

1. **Clone Status**: Should show green checkmark if the bare clone exists
2. **Webhook**: Click **"Register webhook automatically"** to create the Forgejo webhook
3. **Environment Loader**: Select your loader and click **"Save"**, then optionally click **"Test environment"** to verify
4. **Dependencies**: Verify all green checks for token, opencode binary, and config files

Once all checks pass, the repository is ready to receive `@forgebot` commands.

## Manual Deployment (Other Linux)

For non-NixOS systems, follow this alternative deployment path:

> **Note**: This path is not officially tested for the POC, but should work on any recent Linux distribution with systemd.

1. **Download or build the forgebot binary** (see [Building from Source](#building-from-source))

2. **Create a system user**:
   ```bash
   sudo useradd -r -s /bin/false forgebot
   ```

3. **Create data directories**:
   ```bash
   sudo mkdir -p /var/lib/forgebot/{worktrees,opencode-config}
   sudo chown -R forgebot:forgebot /var/lib/forgebot
   ```

4. **Set environment variables** and run:
    ```bash
    export FORGEBOT_WEBHOOK_SECRET="your-webhook-secret-here"
    export FORGEBOT_FORGEJO_URL="https://git.example.com"
    export FORGEBOT_FORGEJO_TOKEN="your-forgejo-api-token"

    # forgejo-mcp must also be available in PATH for opencode MCP calls
    
    # Optional - override defaults
    export FORGEBOT_SERVER_HOST="127.0.0.1"
    export FORGEBOT_SERVER_PORT="8765"
    export FORGEBOT_FORGEBOT_HOST="https://forgebot.example.com"  # Set for production!
    export FORGEBOT_FORGEJO_BOT_USERNAME="forgebot"
    
    forgebot
    ```

5. **Write a systemd service unit** at `/etc/systemd/system/forgebot.service`:

   ```ini
   [Unit]
   Description=Forgebot — Forgejo webhook bridge to opencode
   After=network-online.target
   Wants=network-online.target

   [Service]
   Type=simple
   User=forgebot
   Group=forgebot
   WorkingDirectory=/var/lib/forgebot
   
   ExecStart=/usr/local/bin/forgebot
   
   # Required environment variables
   Environment="FORGEBOT_WEBHOOK_SECRET=your-webhook-secret"
   Environment="FORGEBOT_FORGEJO_URL=https://git.example.com"
   Environment="FORGEBOT_FORGEJO_TOKEN=your-api-token"
   
    # Optional - defaults shown
    Environment="FORGEBOT_SERVER_HOST=127.0.0.1"
    Environment="FORGEBOT_SERVER_PORT=8765"
    Environment="FORGEBOT_FORGEBOT_HOST=https://forgebot.example.com"
    Environment="FORGEBOT_FORGEJO_BOT_USERNAME=forgebot"
    
    # On systemd, use absolute paths for git and opencode to avoid PATH issues
    Environment="FORGEBOT_GIT_BINARY=/usr/bin/git"
    Environment="FORGEBOT_OPENCODE_BINARY=/usr/local/bin/opencode"
    
    # Ensure opencode and forgejo-mcp are in PATH
    Environment="PATH=/usr/local/bin:/usr/bin:/bin"
   Environment="RUST_LOG=info"
   
   Restart=on-failure
   RestartSec=10
   
   # Security hardening
   NoNewPrivileges=true
   ProtectSystem=strict
   ProtectHome=true
   PrivateTmp=true

   [Install]
   WantedBy=multi-user.target
   ```

6. **Start the service**:
   ```bash
   sudo systemctl daemon-reload
   sudo systemctl enable --now forgebot
   ```

7. Follow steps 5-8 from the [Quick Start](#quick-start-primary-path-nixos) guide above.

## Environment Variable Reference

forgebot is configured entirely through environment variables:

### Required Environment Variables

These must be set or forgebot will exit with an error:

| Variable | Description | Example |
|----------|-------------|---------|
| `FORGEBOT_WEBHOOK_SECRET` | Webhook secret for HMAC verification (must match Forgejo webhook settings) | `openssl rand -hex 32` |
| `FORGEBOT_FORGEJO_TOKEN` | API token for Forgejo authentication | Create in Forgejo Settings → Applications |

**Note**: `FORGEBOT_FORGEJO_URL` is also required, but on NixOS it should be set via the `forgejo.url` option. For manual deployments, it must be set as an environment variable.

### Optional Environment Variables

These have sensible defaults if not set:

| Variable | Default | Description |
|----------|---------|-------------|
| `FORGEBOT_SERVER_HOST` | `127.0.0.1` | Host address to bind HTTP server |
| `FORGEBOT_SERVER_PORT` | `8765` | TCP port to listen on |
| `FORGEBOT_FORGEBOT_HOST` | `http://<server_host>:<server_port>` | Public-facing URL where forgebot is accessible. Used for webhook URLs displayed in the UI and for registering webhooks with Forgejo. For production, set this to your public HTTPS URL (e.g., `https://forgebot.example.com`). |
| `FORGEBOT_FORGEJO_URL` | *(required)* | Base URL of your Forgejo instance — set via `forgejo.url` in NixOS, or as env var for manual deployments |
| `FORGEBOT_FORGEJO_BOT_USERNAME` | `forgebot` | Username that forgebot operates as |
| `FORGEBOT_OPENCODE_BINARY` | `opencode` | Path to opencode binary |
| `FORGEBOT_OPENCODE_WORKTREE_BASE` | `/var/lib/forgebot/worktrees` | Base directory for git worktrees |
| `FORGEBOT_OPENCODE_CONFIG_DIR` | `/var/lib/forgebot/opencode-config` | Directory for opencode config files |
| `FORGEBOT_OPENCODE_API_BASE_URL` | `http://127.0.0.1:4096` | Base URL for the OpenCode API server |
| `FORGEBOT_OPENCODE_API_TOKEN` | *(unset)* | Optional bearer token for OpenCode API authentication |
| `FORGEBOT_OPENCODE_API_TIMEOUT_SECS` | `30` | HTTP timeout for OpenCode API requests |
| `FORGEBOT_OPENCODE_WEB_HOST` | *(unset)* | Public base URL for opencode Web UI. When set, forgebot comments a direct session link after it captures the opencode session ID. |
| `FORGEBOT_GIT_BINARY` | `git` | Path to git binary. Set this to an absolute path (e.g., `/usr/bin/git`) if running under systemd or other environments with minimal PATH. |
| `FORGEBOT_DATABASE_PATH` | `/var/lib/forgebot/forgebot.db` | Path to SQLite database |

### Other Environment Variables

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Controls tracing/log output verbosity: `info`, `debug`, `trace`, `warn` |
| `RUST_BACKTRACE` | Enables Rust stack traces on panic: `1` or `full` |

## Usage

### Triggering the Agent

All interactions use `@forgebot` mentions in Forgejo comments.

#### Issue Flow — Plan Then Implement

In an issue comment, write:

```
@forgebot
```

forgebot will:
1. Create a worktree for the issue
2. Invoke opencode using the `forgebot` agent
3. The agent posts at least one planning comment on the issue
4. Based on issue context and follow-up comments, the agent proceeds to implementation and opens a PR

If you want to push the workflow forward, leave another `@forgebot` comment (for example: "@forgebot proceed with implementation").

#### PR Review Revision — Address Review Comments

Leave a review comment on the pull request:

```
@forgebot
Fix the linting errors and add more test coverage.
```

forgebot will:
1. Resume the same issue session
2. Use your review comment as the prompt
3. Make the requested changes
4. Force-push to the existing PR branch

### Environment Loaders

Configure the environment loader per-repository in the setup UI. This controls how dependencies are loaded before the agent runs:

| Loader | Description | Requirements |
|--------|-------------|--------------|
| **None** | Use system environment as-is | Default option, no additional requirements |
| **direnv** | Source `.envrc` from repository root | `direnv` must be installed and available |
| **Nix** | Evaluate `flake.nix` via `nix print-dev-env` | Nix with flakes enabled, `flake.nix` in repo root |

**Testing**: The setup UI includes a **"Test environment"** button that runs a lightweight check to verify the loader works before you trigger the agent.

## Building from Source

### Prerequisites

- Rust toolchain (1.75+ recommended)
- Nix (with flakes enabled) — for Nix builds
- pkg-config, openssl, sqlite3 development headers

### Build Commands

Using Nix flake (recommended):

```bash
git clone https://github.com/<owner>/forgebot.git
cd forgebot
nix build
# Binary will be at ./result/bin/forgebot
```

Using Cargo directly:

```bash
git clone https://github.com/<owner>/forgebot.git
cd forgebot
cargo build --release
# Binary will be at ./target/release/forgebot
```

### Development Shell

Enter a development environment with all dependencies:

```bash
nix flake develop
# or
nix shell
```

Available commands in the dev shell:
- `cargo build` — Build the project
- `cargo test` — Run tests
- `cargo clippy` — Run linter
- `sqlx migrate` — Run database migrations

### Local End-to-End Testing

Use `process-compose` for local E2E runs. It prepares an isolated runtime under `~/.local/state/forgebot-local-dev` and then starts the app:

```bash
process-compose up -D
```

The runtime setup creates isolated XDG directories for OpenCode state and sets `HOME` to the runtime root so embedded OpenCode does not read your host config.

The local stack starts both `forgebot` and `opencode serve` so API transport smoke tests run with production-like routing.

For a semi-automated manual-gated smoke test (issue -> plan -> proceed -> PR), run:

```bash
just e2e-smoke-manual
```

The script opens the issue in your browser and pauses for manual `y/n` confirmation at planning and PR checkpoints. It only performs cleanup if you confirm `y` at the final PR checkpoint.

To clean all local test artifacts:

```bash
rm -rf "$HOME/.local/state/forgebot-local-dev"
```

## Troubleshooting

For API cutover and rollback procedures, see `docs/opencode-api-cutover-runbook.md`.

### Common Issues

#### "ERROR: FORGEBOT_WEBHOOK_SECRET environment variable is required but not set"

- The two required secret environment variables must be set: `FORGEBOT_WEBHOOK_SECRET`, `FORGEBOT_FORGEJO_TOKEN`
- `FORGEBOT_FORGEJO_URL` must also be set, either via NixOS `forgejo.url` option or as an environment variable
- Check that your secrets file (if using `secretsFilePath`) is properly formatted and readable by the forgebot user
- Verify systemd loaded the environment file: `systemctl show forgebot --property=EnvironmentFile`

#### "env loader direnv failed"

- Ensure `.envrc` exists in the repository root
- Verify `direnv` is installed: `which direnv`
- Test manually: `cd /path/to/repo && direnv export json`

#### "env loader nix failed"

- Ensure `flake.nix` exists in the repository root
- Verify Nix is installed with flakes: `nix --version` and `nix flake --help`
- First evaluation may be slow while Nix downloads dependencies
- Test manually: `cd /path/to/repo && nix print-dev-env`

#### Webhook not registering

- Check Forgejo token permissions — the token must have API access to the repository
- Verify the bot user has admin or write access to create webhooks
- Check forgebot logs for HTTP error responses from Forgejo

#### Worktree creation fails

- Ensure the bare clone exists at the expected path shown in the setup UI
- Verify the forgebot user has read/write permissions on the worktree base directory
- Check that the repository default branch in the UI matches the actual default branch

#### opencode not found

- Ensure opencode is in the service PATH
- On NixOS, verify `opencodePackage` is set or available in nixpkgs
- Test: `sudo -u forgebot which opencode`

### Getting Help

For detailed logs:

```bash
# NixOS
journalctl -u forgebot -f

# Other systemd systems
sudo journalctl -u forgebot -f
```

Check the database for session state:

```bash
sqlite3 /var/lib/forgebot/forgebot.db ".tables"
sqlite3 /var/lib/forgebot/forgebot.db "SELECT * FROM sessions WHERE state = 'error';"
```

## Configuration Reference (Legacy)

The `forgebot.toml.example` file in the repository shows the legacy TOML configuration format. **This is for reference only** — new deployments should use environment variables exclusively as described above.

## Architecture Notes

### Session ID Strategy

Each issue receives a deterministic session ID based on the issue number and repository name:

```
ses_{issue_id}_{owner}_{repo}
```

Example: `ses_42_alice_myrepo`

opencode creates and resumes sessions using these IDs, maintaining full context across issue and PR interactions. This means:
- You can `@forgebot` multiple times on the same issue and keep context
- PR review comments resume the same session
- Session state persists across forgebot restarts

### Git Worktrees

Each issue gets its own isolated git worktree to prevent concurrent sessions from interfering:

```
<worktree_base>/<owner>_<repo>/         # Bare clone
├── <issue_number>/                     # Worktree for issue N
│   ├── .git                            # Git metadata
│   └── [repo files]
└── <issue_number_2>/                   # Worktree for issue M
    └── ...
```

### No Queue

The current POC does not implement command queuing. If forgebot is already working on an issue when a new `@forgebot` command arrives, it will reject the new trigger with a "currently working" comment. Wait for the current operation to complete before issuing new commands.

## Contributing

This is a proof-of-concept (POC). Known limitations for future versions include:

- **Command queueing / debouncing** — Handle multiple concurrent triggers gracefully
- **Multi-repo fan-out** — Plan work that spans multiple repositories in a single session
- **Non-Forgejo platforms** — Extend to GitHub, GitLab, or other Git hosting platforms
- **Persistent session export/import** — Backup and migrate session state between deployments
- **Webhook authentication improvements** — IP allowlists, mTLS, or other verification methods

Bug reports and contributions are welcome. Please open an issue to discuss significant changes before submitting a pull request.

## License

[Add appropriate license here — MIT recommended for open source]

---

**Built with**: Rust, Axum, SQLx, and the Nix ecosystem  
**Integrates with**: Forgejo, opencode
