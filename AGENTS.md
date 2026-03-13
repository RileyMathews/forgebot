# Forgebot — Agent Instructions

Forgebot is a Rust daemon that bridges Forgejo webhooks to `opencode` (an AI coding agent). It listens for webhook events and orchestrates `opencode` sessions to automatically plan and implement work from Forgejo issues.

## Running the App for Local Testing

Use `process-compose` to build and run the app. It is available directly in the dev shell (`nix develop`).

```bash
# Start in detached mode (background)
process-compose up -D

# Check status of running processes
curl localhost:8080/processes

# Tear down
process-compose down
```

`prepare-runtime` and `build` must complete successfully before `run` starts. If either step fails, `process-compose down` is still required to clean up.

The app itself listens on port `8765` by default (`FORGEBOT_SERVER_PORT`).

### Local E2E Test Hygiene (Required)

Always run local Forgejo E2E tests with a clean runtime DB and clean up test webhook state afterward.

Before starting an E2E test:

```bash
process-compose down
rm -f "$HOME/.local/state/forgebot-local-dev/forgebot.db"
process-compose up -D
```

After finishing an E2E test:

```bash
# Stop local stack
process-compose down

# Remove local test DB
rm -f "$HOME/.local/state/forgebot-local-dev/forgebot.db"
```

Also remove any E2E webhook entries created in Forgejo for the test repo (for example `riley/terminal-config`) so later runs do not inherit stale delivery targets.

```bash
python - <<'PY'
import json, os, urllib.request

base = os.environ['FORGEBOT_FORGEJO_URL']
token = os.environ['FORGEBOT_FORGEJO_TOKEN']
repo = 'riley/terminal-config'
target_url = 'http://ds9:8765/webhook'

req = urllib.request.Request(
    f"{base}/api/v1/repos/{repo}/hooks",
    headers={'Authorization': f'token {token}', 'Accept': 'application/json'},
)
with urllib.request.urlopen(req) as r:
    hooks = json.load(r)

for hook in hooks:
    if hook.get('config', {}).get('url') == target_url:
        delete_req = urllib.request.Request(
            f"{base}/api/v1/repos/{repo}/hooks/{hook['id']}",
            method='DELETE',
            headers={'Authorization': f'token {token}', 'Accept': 'application/json'},
        )
        urllib.request.urlopen(delete_req).read()
PY
```

### Local E2E Smoke Test (Issue -> PR)

Use this flow to validate the full webhook-to-agent pipeline on local dev.

For agent-driven runs, execute these steps directly with commands/tool calls. Do not use `scripts/e2e_smoke_manual.py`, which is designed for human/manual checkpoints.

1. Start from a clean state using the hygiene steps above.
2. Add test repo in the UI (or via HTTP form):

```bash
curl -si -X POST http://127.0.0.1:8765/ui/repos \
  -d "full_name=riley/terminal-config&default_branch=main&env_loader=none"
```

3. Wait for clone status to become `ready`, then register webhook:

```bash
# Check clone status
sqlite3 "$HOME/.local/state/forgebot-local-dev/forgebot.db" \
  "select full_name, clone_status, clone_attempts, coalesce(clone_error,'') from repos;"

# Register webhook
curl -si -X POST http://127.0.0.1:8765/ui/repo/riley/terminal-config/webhook
```

4. Create an issue in Forgejo and trigger a collaborative session with an issue comment. Use the forgejo MCP to create the issue and comment. The comment must contain `@forgebot`.

5. Verify the bot posts an actual plan comment on the issue (not just acknowledgement/dispatch comments). In API mode, a session may return to `idle` before the asynchronous OpenCode run posts the plan, so poll issue comments until the plan appears.

6. If the plan asks follow-up questions, answer them on the issue and trigger another run with a new `@forgebot` comment. If the plan is sufficient, trigger another `@forgebot` comment asking it to proceed.

7. Verify a PR is created and linked back to the issue (for example, via `Closes #<issue>` or equivalent in the PR body/comment trail). Session state alone is insufficient in API mode; use MCP to confirm issue + PR side effects.

8. Always run the post-test cleanup steps from the hygiene section.

### When Unexpected Failures Happen During E2E

If the smoke test hits an unexpected failure (for example, missing plan comment, missing PR side effects, or any unclear asynchronous behavior), do **not** immediately run teardown/cleanup.

1. Leave `process-compose` services running so the operator can inspect live OpenCode/Forgebot session state.
2. Capture and report the issue/PR links, session IDs, and relevant log lines.
3. Wait for operator confirmation before running cleanup (`process-compose down`, DB removal, webhook cleanup).

## Development Shell

```bash
nix develop       # or: direnv allow
```

Required secrets go in `.envrc.secret` (not committed). Runtime sandbox paths are set only by `process-compose`, so normal shell usage of `opencode` keeps using your host config.

For a testing repository use the `riley/terminal-config` repository in forgejo.

## Build / Lint / Test Commands

```bash
cargo build                          # debug build (SQLX_OFFLINE=true is set in .envrc)
cargo build --release                # release build
cargo clippy                         # linter — fix all warnings before committing
cargo fmt                            # formatter — always run before committing
cargo test                           # all tests
cargo test <name>                    # run a single test by name (substring match)
cargo test <name> -- --nocapture     # with stdout output
cargo test --test forgejo_integration -- --ignored  # integration tests (need live Forgejo)
sqlx migrate run                     # run DB migrations manually
```

`SQLX_OFFLINE=true` must be set when building — the `.envrc` sets this. The `process-compose.yaml` also sets it for the build process.

### Running a Single Test

```bash
cargo test test_parse_action_from_comment
cargo test test_derive_session_id
cargo test test_load_env_dispatcher_none
```

Test names are substring-matched, so a partial name works.

## Code Style

### Formatting and Linting

- `rustfmt` defaults — always run `cargo fmt` before committing
- `cargo clippy` must pass with zero warnings
- No `#[allow(clippy::...)]` without a comment explaining why

### Error Handling
We are moving away from using `anyhow` for error handling and moving towards more tightly controlled
internal error types. While working in the codebase if you see a use of `anyhow` near the code your working
in feel free to migrate it to a custom error type.

Feel free to use .expect("...") and fail hard when errors are truly unrecoverable cases.

### Imports

Group in this order with a blank line between groups:
1. `std::*`
2. Third-party crates (`anyhow`, `axum`, `serde`, `sqlx`, `tracing`, etc.)
3. `crate::*` internal modules

```rust
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{extract::State, response::Response};

use crate::config::Config;
use crate::db::DbPool;
```

### Naming Conventions

| Thing | Convention |
|---|---|
| Structs / Enums | `PascalCase` |
| Functions / variables / fields | `snake_case` |
| Constants | `SCREAMING_SNAKE_CASE` |
| Module files | `snake_case` |
| Serde API models | `#[serde(rename_all = "snake_case")]` |

### Logging

`tracing` macros are used throughout. Use structured field-value syntax:

```rust
info!(session_id = %id, repo = %repo, "Dispatching session");
warn!(path = %p, "Config file not found, using defaults");
error!(err = %e, "Failed to post comment");
```

Use `%` (Display) for strings and typed values, `?` (Debug) only when Display is unavailable.

Level guidelines:
- `info!` — lifecycle events, successful operations, config values at startup
- `warn!` — non-fatal issues, fallbacks, missing optional config
- `error!` — failures caught in-place that don't propagate
- `debug!` — verbose/noisy details not needed in production

### Module Structure

```
src/
├── main.rs            # Entry point only — wires modules together
├── lib.rs             # Re-exports all modules as pub (for integration tests)
├── config.rs          # All env var loading and config structs
├── db.rs              # DbPool type + all CRUD functions (flat, no sub-modules)
├── forgejo/
│   ├── mod.rs         # ForgejoClient + all API methods
│   └── models.rs      # API request/response structs
├── session/
│   ├── mod.rs         # SessionTrigger, comment helpers, prompt builders
│   ├── opencode.rs    # opencode subprocess orchestration + crash recovery
│   ├── worktree.rs    # git worktree helpers
│   └── env_loader.rs  # direnv/nix env loading
├── webhook/
│   ├── mod.rs         # AppState, HMAC verifier, router, start_server
│   ├── handlers.rs    # Webhook event handlers
│   └── models.rs      # Webhook payload structs
└── ui/
    ├── mod.rs          # UI router
    └── handlers.rs     # Askama template handlers
```

Each subdirectory: `mod.rs` for primary logic, `models.rs` for data structures.

### Database

`sqlx 0.8` with SQLite. Use raw SQL with `?1, ?2, ...` positional bind parameters. No `query!` / `query_as!` macros — use `query()` with manual `.get("column_name")` mapping.

```rust
sqlx::query(r#"SELECT id, status FROM sessions WHERE id = ?1"#)
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to fetch session {}", session_id))?;
```

Migrations live in `migrations/` as plain SQL files. They run automatically on every startup via `sqlx::migrate!`. Add new migrations as sequentially numbered files (`003_...sql`).

### Tests

Unit tests live at the bottom of each source file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_something() { ... }

    #[tokio::test]
    async fn test_async_thing() { ... }
}
```

- Use `std::env::temp_dir().join(format!("forgebot-test-{}", std::process::id()))` for filesystem isolation
- Clean up temp dirs manually at end of test
- Keep test helper structs/functions inside `mod tests`
- Integration tests go in `tests/` and are `#[ignore = "Requires ..."]` by default

### Async and Concurrency

- All I/O is `async` with `tokio`
- Webhooks are acknowledged immediately with HTTP 200; actual work is spawned with `tokio::spawn`
- `Arc<Config>` is passed everywhere — config is immutable after startup
- `DbPool` and `ForgejoClient` are cheap to clone (pooled/Arc-backed internally)

### Compile-Time Embedding

Static files bundled into the binary use `include_str!`:

```rust
const PACKAGE_JSON: &str = include_str!("../../opencode-config/package.json");
```
