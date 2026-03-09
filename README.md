# forgebot

A self-hosted agentic coding bridge between [Forgejo](https://forgejo.org/) and [opencode](https://github.com/sst/opencode). forgebot listens for Forgejo webhooks and orchestrates opencode sessions to automatically plan and implement work from Forgejo issues.

## Overview

forgebot acts as a webhook receiver and session manager. When you mention `@forgebot` in a Forgejo issue comment, forgebot creates an isolated git worktree, invokes opencode with the issue context, and the agent produces either a detailed implementation plan or working code with a pull request. Each issue maintains its own session context across multiple interactions, preserving state between planning, building, and revision phases.

## Prerequisites

Before installing forgebot, ensure you have:

- A running **Forgejo instance** (self-hosted or compatible)
- **opencode** installed and available in your `PATH` (requires [bun](https://bun.sh/) for tool dependencies)
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
      url = "github:<owner>/forgebot";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  
  # ... rest of your configuration
}
```

### 2. Import the NixOS module

In your `configuration.nix`:

```nix
{ inputs, ... }:
{
  imports = [
    inputs.forgebot.nixosModules.forgebot
  ];
}
```

### 3. Configure the service

Add a minimal service configuration to your NixOS config:

```nix
services.forgebot = {
  enable = true;
  configFile = /etc/secrets/forgebot.toml;  # Or use a plain path string
};
```

For advanced configuration with environment variables:

```nix
services.forgebot = {
  enable = true;
  configFile = /etc/forgebot/forgebot.toml;
  dataDir = "/var/lib/forgebot";
  environment = {
    FORGEBOT_WEBHOOK_SECRET = "your-secret-here";
    FORGEBOT_FORGEJO_TOKEN = "your-token-here";
    RUST_LOG = "info";
  };
};
```

### 4. Create `forgebot.toml`

Copy `forgebot.toml.example` to your configuration path and customize:

```bash
sudo cp forgebot.toml.example /etc/forgebot/forgebot.toml
sudo $EDITOR /etc/forgebot/forgebot.toml
```

Key sections to configure:

- **`[server]`**: HTTP host, port, and webhook secret (can use `FORGEBOT_WEBHOOK_SECRET` env var)
- **`[forgejo]`**: Instance URL, API token (can use `FORGEBOT_FORGEJO_TOKEN` env var), bot username
- **`[opencode]`**: Binary path, worktree base directory, config directory paths
- **`[database]`**: SQLite database file location

See `forgebot.toml.example` for detailed comments on each setting.

### 5. Apply the configuration

```bash
sudo nixos-rebuild switch
```

### 6. Verify startup

Check the service logs for successful initialization:

```bash
journalctl -u forgebot -f
```

You should see messages indicating the database is initialized, migrations completed, and the server is listening.

### 7. Register repositories

Navigate to the setup UI at `http://<host>:8765/ui`:

1. Click **"Add Repository"**
2. Enter:
   - **Full repository name**: e.g., `alice/myrepo`
   - **Default branch**: e.g., `main`
   - **Environment loader**: Choose from None, direnv, or Nix
3. Click **"Add Repository"**

This inserts the repository into the database. The repo will appear in the list with an **"Incomplete"** status.

### 8. Clone watched repositories

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

### 9. Complete repository setup

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

4. **Create and edit `forgebot.toml`**:
   ```bash
   sudo cp forgebot.toml.example /etc/forgebot/forgebot.toml
   sudo $EDITOR /etc/forgebot/forgebot.toml
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
   
   ExecStart=/usr/local/bin/forgebot --config /etc/forgebot/forgebot.toml
   
   # Ensure opencode is in PATH
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

7. Follow steps 6-9 from the [Quick Start](#quick-start-primary-path-nixos) guide above.

## Usage

### Triggering the Agent

All interactions use `@forgebot` mentions in Forgejo comments.

#### Plan Phase — Analyze and Produce a Plan

In an issue comment, write:

```
@forgebot plan
```

forgebot will:
1. Create a worktree for the issue
2. Invoke opencode in **plan mode**
3. The agent analyzes the codebase and issue description
4. Post a detailed implementation plan as a Forgejo comment

#### Build Phase — Implement the Work

In the same issue (after planning), write:

```
@forgebot build
```

forgebot will:
1. Resume the existing session
2. Invoke opencode in **build mode**
3. The agent implements the previously created plan
4. Open a pull request when complete

#### PR Review Revision — Address Review Comments

Leave a review comment on the pull request:

```
@forgebot
Fix the linting errors and add more test coverage.
```

forgebot will:
1. Resume the session in build mode
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

## Troubleshooting

### Common Issues

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

## Architecture Notes

### Session ID Strategy

Each issue receives a deterministic session ID based on the issue number and repository name:

```
ses_{issue_id}_{owner}_{repo}
```

Example: `ses_42_alice_myrepo`

opencode creates and resumes sessions using these IDs, maintaining full context across plan, build, and revision phases. This means:
- You can `@forgebot plan` today and `@forgebot build` tomorrow
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
