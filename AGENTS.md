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

`anyhow` is used exclusively. There are no custom error types (`thiserror` is not a dependency).

```rust
// Always return anyhow::Result
fn do_thing() -> anyhow::Result<()> { ... }

// Add context at every fallible call site
some_op().context("failed to do the thing")?;
some_op().with_context(|| format!("failed for {}", id))?;

// Early return
anyhow::bail!("something went wrong: {}", reason);

// Construct inline
Err(anyhow::anyhow!("unexpected state: {:?}", state))
```

HTTP handlers return `Result<Response, axum::response::Response>` — the `Err` variant is itself an HTTP error response, not a Rust error.

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
