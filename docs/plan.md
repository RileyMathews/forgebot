# forgebot — Implementation Checklist

Work through each phase in order. Each phase produces a runnable/testable increment.
Tick items off as you go. Refer to `forgebot-spec.md` for full detail on any item.

---

## Phase 1 — Project Scaffold

Get a compiling, runnable binary with nothing in it yet.

- [x] Initialise Cargo workspace (`cargo new forgebot`)
- [x] Add core dependencies to `Cargo.toml`: `axum`, `tokio` (full features), `sqlx` (sqlite, runtime-tokio), `reqwest` (json, tls), `serde` + `serde_json`, `toml`, `anyhow`, `tracing`, `tracing-subscriber`
- [x] Create `src/main.rs` — reads config file path from `--config` CLI flag (use `clap`), logs startup message, exits cleanly
- [x] Create `src/config.rs` — structs for all TOML sections (`[server]`, `[forgejo]`, `[opencode]`, `[database]`); load and validate on startup; env var overrides for `FORGEBOT_WEBHOOK_SECRET` and `FORGEBOT_FORGEJO_TOKEN`
- [x] Confirm: `cargo build` succeeds, binary runs and prints config load confirmation

---

## Phase 2 — Database

Schema and CRUD before anything else touches state.

- [x] Create `migrations/001_initial.sql` — `sessions` and `pending_worktrees` tables (spec §5)
- [x] Create `migrations/002_repos.sql` — `repos` table (spec §5)
- [x] Create `src/db.rs` — connect to SQLite via `sqlx`, run migrations on startup
- [x] Implement repo CRUD: `insert_repo`, `get_repo_by_full_name`, `list_repos`, `update_repo_env_loader`
- [x] Implement session CRUD: `insert_session`, `get_session_by_issue`, `get_session_by_pr`, `update_session_state`, `update_session_pr_id`
- [x] Implement `get_sessions_in_state` (used by startup crash recovery)
- [x] Confirm: migrations run cleanly, a quick manual insert/query via `sqlx` test or `sqlite3` CLI works

---

## Phase 3 — Forgejo API Client

The thin HTTP client forgebot uses to talk to Forgejo (separate from the tools opencode uses).

- [x] Create `src/forgejo/models.rs` — structs for API responses: `Issue`, `IssueComment`, `PullRequest`, `PullRequestReviewComment`, `Webhook`
- [x] Create `src/forgejo/mod.rs` — `ForgejoClient` struct wrapping `reqwest::Client`
- [x] Implement `get_issue(repo, issue_id)`
- [x] Implement `list_issue_comments(repo, issue_id)`
- [x] Implement `list_pr_review_comments(repo, pr_id)`
- [x] Implement `post_issue_comment(repo, issue_id, body)` — used by forgebot for lifecycle acknowledgements
- [x] Implement `post_pr_comment(repo, pr_id, body)` — used for error/busy comments on PRs
- [x] Implement `list_repo_webhooks(repo)` — used by setup UI to check webhook registration status
- [x] Implement `create_repo_webhook(repo, url, secret)` — used by setup UI one-click registration
- [x] Implement `check_token_permissions(repo)` — test call used by setup UI verify step
- [x] Confirm: write a quick integration test or `main.rs` throwaway that lists issues on a real repo

---

## Phase 4 — Webhook Server (Skeleton)

Get the HTTP server running and accepting webhooks, before implementing any handler logic.

- [x] Create `src/webhook/models.rs` — structs for all Forgejo webhook payloads: `IssueCommentPayload`, `PullRequestPayload`, `PullRequestReviewCommentPayload`; include the `X-Gitea-Event` header enum
- [x] Create `src/webhook/mod.rs` — axum router at `POST /webhook`; HMAC-SHA256 signature verification middleware that reads raw bytes, verifies against `config.server.webhook_secret`, returns 401 on failure before any further processing (spec §18 note 1)
- [x] Create `src/webhook/handlers.rs` — single dispatcher that reads `X-Gitea-Event` header and routes to stub handler functions; all stubs return 200 immediately
- [x] Wire router into `main.rs`, bind to configured host/port
- [x] Confirm: server starts, a correctly signed test webhook returns 200, a bad signature returns 401

---

## Phase 5 — Git Worktree Orchestration

Isolated per-issue working directories. Must work before opencode can be invoked.

- [x] Create `src/session/worktree.rs`
- [x] Implement `worktree_path(config, repo_full_name, issue_id) -> PathBuf` — derives the canonical path
- [x] Implement `create_worktree(repo_full_name, issue_id, default_branch)` — checks local bare clone exists at `<worktree_base>/<owner>_<repo>`, hard-fails with a descriptive error if not; runs `git worktree add <path> -b agent/issue-<id>`
- [x] Implement `remove_worktree(path)` — runs `git worktree remove --force <path>`
- [x] Implement `clone_exists(config, repo_full_name) -> bool` — probe used by setup UI and worktree creation
- [x] Confirm: manually create a local bare clone, call `create_worktree`, verify the branch and directory appear

---

## Phase 6 — Environment Loader

Builds the subprocess environment before opencode is spawned.

- [x] Create `src/session/env_loader.rs`
- [x] Implement `load_env(loader_type, worktree_path) -> Result<HashMap<String, String>>` with three branches:
  - `none` — returns empty map (caller merges with process env)
  - `direnv` — spawns `direnv export json` in worktree, parses JSON output, returns map; hard-fails with full stderr on non-zero exit or parse error
  - `nix` — spawns `nix print-dev-env --json` in worktree, extracts only `type == "exported"` string variables, returns map; hard-fails on error; apply a 60-second timeout
- [x] Confirm: test each branch against a real worktree with a `.envrc` or `flake.nix`

---

## Phase 7 — opencode Config Directory Setup

Write the global opencode config (agent definition + TypeScript Forgejo tools) on startup.

- [x] Embed the `opencode-config/` template files into the binary using `include_str!` macros
- [x] Create `src/session/opencode.rs` — implement `setup_opencode_config_dir(config)`:
  - Creates `<opencode_config_dir>/tools/` and `<opencode_config_dir>/agents/` directories
  - Writes `package.json`, `agents/forgebot.md`, `tools/comment-issue.ts`, `tools/comment-pr.ts`, `tools/create-pr.ts` — skips any file that already exists
- [x] Call `setup_opencode_config_dir` from `main.rs` on startup, before the server begins accepting requests
- [x] Write the three TypeScript tool files (spec §7) into `opencode-config/tools/` in the source tree
- [x] Write `opencode-config/package.json` declaring `@opencode-ai/plugin` dependency
- [x] Write `opencode-config/agents/forgebot.md` (spec §14)
- [x] Confirm: run binary, verify all files appear at the configured path with correct content

---

## Phase 8 — opencode Subprocess Invocation

The core of forgebot — spawning opencode and managing its lifecycle.

- [ ] Implement `derive_session_id(repo_full_name, issue_id) -> String` — produces `ses_{issue_id}_{sanitized_owner}_{sanitized_repo}` (lowercase, non-alphanumeric stripped)
- [ ] Implement `build_prompt(phase, issue, comments, pr_review_comments) -> String` — constructs the full prompt string for each of the three phases using the templates in spec §11
- [ ] Implement `run_opencode(config, session_id, agent_mode, worktree_path, prompt, env_extras) -> Result<()>`:
  - Merges environment: process env + loader output + `FORGEBOT_*` vars + `OPENCODE_CONFIG_HOME`
  - Spawns `opencode run --session <id> --agent <plan|build> --cwd <path> --quiet "<prompt>"` via `tokio::process::Command`
  - Awaits exit; returns `Ok(())` on exit code 0, `Err` on non-zero
- [ ] Implement `dispatch_session(db, forgejo, config, trigger)` — the full orchestration sequence for a trigger: load env, build prompt, update state to planning/building, spawn opencode in background `tokio::spawn`, await exit, update state to idle or error, post error comment if needed (spec §18 notes 2 and 3)
- [ ] Implement startup crash recovery in `main.rs`: query `get_sessions_in_state(&["planning","building"])`, set each to `error`, post a comment on the issue explaining forgebot restarted mid-run
- [ ] Confirm: invoke `run_opencode` against a real repo worktree with a simple prompt and verify opencode runs and exits

---

## Phase 9 — Webhook Handlers (Full Logic)

Wire all the real handler logic now that the building blocks exist.

- [ ] Implement `issue_comment` handler:
  - Ignore if author == `config.forgejo.bot_username`
  - Ignore if repo not in `repos` table
  - Ignore if comment does not contain `@forgebot`
  - Look up or create session row
  - If state is `planning` or `building`: post busy comment, return 200
  - Parse keyword: `plan` -> set agent mode `plan`; `build` -> set agent mode `build`; anything else -> use current session state to pick mode
  - Post acknowledgement comment (`🤖 forgebot is thinking...` or `🤖 forgebot is working on it...`)
  - `tokio::spawn` -> `dispatch_session`
- [ ] Implement `pull_request` opened handler:
  - Parse head branch; extract issue ID from `agent/issue-<id>` pattern
  - Look up session by `(repo_full_name, issue_id)`; if not found, log and return 200 (not an error — may not be a forgebot PR)
  - Update session row with PR ID
- [ ] Implement `pull_request` closed/merged handler:
  - Parse head branch for `agent/issue-<id>` pattern; if no match, return 200
  - Insert into `pending_worktrees`
  - Call `remove_worktree` (or schedule it — for POC, remove inline is fine)
- [ ] Implement `pull_request_review_comment` handler:
  - Ignore if author == bot username
  - Ignore if comment does not contain `@forgebot`
  - Look up session by PR ID; if not found: post hard-fail comment, return 200
  - If state is `planning` or `building`: post busy comment, return 200
  - Post acknowledgement comment on PR
  - `tokio::spawn` -> `dispatch_session` in build mode with review comment prompt
- [ ] Confirm: end-to-end test — post `@forgebot plan` on a real issue, verify the full flow completes

---

## Phase 10 — Setup UI

Operator web interface for repo management. Comes after core agent flow so the critical path is working first.

- [ ] Add `askama` or `minijinja` to dependencies for server-side HTML rendering
- [ ] Create `templates/base.html` — minimal HTML shell, no JS framework
- [ ] Create `src/ui/mod.rs` — axum router for `/ui` prefix
- [ ] Create `src/ui/handlers.rs`

**Dashboard (`GET /ui`)**
- [ ] Query all rows from `repos` table
- [ ] For each repo: check clone exists (`clone_exists`), check webhook registered (`list_repo_webhooks`), count active sessions
- [ ] Render `templates/dashboard.html` — repo list with status indicators + Add Repository form (full_name, default_branch, env_loader radio group)
- [ ] `POST /ui/repos` — validate inputs, insert repo row, redirect to per-repo setup page

**Per-Repo Setup (`GET /ui/repo/:owner/:name`)**
- [ ] Create `templates/repo_setup.html`
- [ ] Step 1: clone status — green/red, show exact `git clone --bare` command
- [ ] Step 2: webhook — show webhook URL and secret; `POST /ui/repo/:owner/:name/webhook` calls `create_repo_webhook`, redirects back
- [ ] Step 3: env loader — radio group pre-selected from DB; `POST /ui/repo/:owner/:name/env-loader` updates `repos.env_loader`, redirects back
- [ ] Step 3b: Test environment button — `POST /ui/repo/:owner/:name/test-env` runs the loader against the local clone synchronously with a 30-second timeout; returns env var key list or error output inline
- [ ] Step 4: verify — token permissions check, opencode binary found, config dir files present

**Sessions (`GET /ui/sessions`)**
- [ ] Create `templates/sessions.html`
- [ ] Query all rows from `sessions`, render as table: repo, issue ID, PR ID, state, worktree path, last updated

- [ ] Confirm: full UI walkthrough — add a repo, register webhook, set env loader, run test-env, view sessions

---

## Phase 11 — Nix Packaging and NixOS Module

- [ ] Write `opencode-config/package.json` is already done (Phase 7) — confirm it is committed to the repo
- [ ] Create `flake.nix`:
  - Input: `nixpkgs`, `naersk` or `crane` for Rust builds, `flake-utils`
  - `packages.${system}.forgebot` — builds the binary
  - `nixosModules.forgebot` — imports `nix/module.nix`
  - `devShells.${system}.default` — shell with `rustc`, `cargo`, `rust-analyzer`, `sqlx-cli`, `opencode`
- [ ] Create `nix/overlay.nix` — Nix package derivation for forgebot binary
- [ ] Create `nix/module.nix` — NixOS module with options: `enable`, `package`, `configFile`, `dataDir`, `user`, `group` (spec §19)
  - Creates system user and group
  - Creates `dataDir` with correct ownership
  - Defines `systemd.services.forgebot` unit running as forgebot user, restarts on failure
  - Ensures `opencode` is in the service PATH
- [ ] Confirm: `nix build` produces a working binary; `nix flake check` passes

---

## Phase 12 — README and Documentation

- [ ] Write `forgebot.toml.example` covering all config sections with comments
- [ ] Write `README.md` with:
  - What forgebot is (2-3 sentences)
  - Prerequisites (Forgejo instance, opencode installed, Nix with flakes if using Nix env loader, bun for opencode tool dependencies)
  - NixOS deployment walkthrough (spec §19 steps 1-9, in order)
  - Manual repo setup steps (bare clone command)
  - Trigger syntax quick reference (`@forgebot plan`, `@forgebot build`, PR review tagging)
  - Brief note that other Linux deployments are possible but not officially documented
- [ ] Confirm: follow the README from scratch on a clean NixOS VM and verify it works end-to-end

---

## Phase 13 — Final Hardening

Last pass before calling the POC done.

- [ ] Audit all `unwrap()` and `expect()` calls — replace with proper error propagation or intentional panics with clear messages
- [ ] Confirm all webhook handlers return 200 before spawning background work (never block Forgejo waiting for opencode)
- [ ] Confirm bot comment loop prevention is in place on every handler that reads comments
- [ ] Confirm env loader hard-failure path posts to Forgejo and sets state to `error` (not just logs)
- [ ] Confirm busy-state rejection posts a Forgejo comment and returns 200
- [ ] Confirm crash recovery on startup posts a Forgejo comment for each stuck session
- [ ] Confirm HMAC verification rejects bad signatures before any payload deserialization
- [ ] Review `FORGEBOT_*` env var injection — confirm they always win over loader output
- [ ] Add basic `tracing` instrumentation to all major code paths (webhook received, session dispatched, opencode spawned, opencode exited, state transition)
- [ ] Test the full issue -> plan -> build -> PR -> review -> revision flow end-to-end on a real Forgejo instance
