# forgebot — POC Specification

> A self-hosted agentic coding bridge between Forgejo and opencode.

---

## 1. Overview

**forgebot** is a lightweight Rust daemon that listens for Forgejo webhooks and orchestrates opencode sessions to plan and implement work described in issues. It bridges the gap between a Forgejo issue tracker and opencode's non-interactive CLI, maintaining persistent session context from the first `@forgebot` mention through to a merged pull request.

### Core Design Principles

- **Webhook-only trigger surface for agent actions** — no web UI for agent interaction; a minimal setup UI is provided for repo registration only
- **Thin wrapper** — forgebot manages state and process lifecycle; opencode does all the coding work
- **Persistent sessions** — every run against an issue reuses the same opencode session so the agent retains full context of the work stream
- **Single binary deployment** — no container required; ships as one statically linked binary
- **Fail fast** — unknown states and unexpected inputs produce immediate, loud failures with Forgejo comments explaining what went wrong

---

## 2. Tech Stack

| Concern | Choice | Rationale |
|---|---|---|
| Language | Rust | Single binary, natural async process spawning, strong type safety for state machines |
| HTTP server | `axum` | Ergonomic async web framework, good middleware story for webhook HMAC verification |
| Database | SQLite via `sqlx` | Minimal ops overhead; enough for a single-node homelab app |
| Process execution | `tokio::process` | Async subprocess management; fire opencode and await completion without blocking |
| Config | TOML file + env var overrides | Simple, human-editable |
| Forgejo API client | `reqwest` + hand-rolled thin client | Only a handful of API calls needed; no need for a full SDK |

---

## 3. User-Facing Workflow

### 3.1 Plan Phase

1. User opens an issue on any watched Forgejo repository.
2. User (or anyone with repo access) posts a comment containing `@forgebot plan`.
3. forgebot receives the `issue_comment` webhook.
4. forgebot posts an acknowledgement comment: *"🤖 forgebot is thinking..."*
5. forgebot creates a git worktree for this issue (see §6).
6. forgebot invokes opencode in **plan agent mode** against that worktree, passing the issue title, body, and all comments as context.
7. opencode runs to completion, posting its plan as a Forgejo comment via the Forgejo shell helper (see §7).
8. forgebot updates session state to `idle`.

### 3.2 Build Phase

1. Discussion may continue on the issue. Any `@forgebot` comment that is not `plan` or a recognised keyword is ignored in the POC.
2. When ready, a user posts `@forgebot build`.
3. forgebot receives the `issue_comment` webhook.
4. forgebot posts: *"🤖 forgebot is working on it..."*
5. forgebot resumes the existing opencode session (same session ID as plan) in **build agent mode** against the same worktree.
6. opencode runs to completion. The agent is expected to implement the work and open a pull request using the Forgejo shell helper.
7. Forgejo fires a `pull_request.opened` webhook. forgebot receives it, parses the branch name (`agent/issue-{issue_id}`), and updates the SQLite row with the PR ID.
8. forgebot updates session state to `idle`.

### 3.3 PR Revision Phase

1. Reviewer leaves comments on the pull request, tagging `@forgebot` with revision instructions.
2. forgebot receives the `pull_request_review_comment` or `issue_comment` (PR comments) webhook.
3. forgebot looks up the session by PR ID. **If no session is found, fail hard**: post a comment saying "I don't have context for this PR" and stop.
4. If found, forgebot resumes the session in build mode with the review comments as the new prompt.
5. Agent makes changes, force-pushes to the existing branch, and comments on the PR when done.

---

## 4. Trigger Syntax

All triggers use `@forgebot` mentions in Forgejo comments.

| Comment contains | Context | Action |
|---|---|---|
| `@forgebot plan` | Issue comment | Start or re-run plan phase |
| `@forgebot build` | Issue comment | Start or re-run build phase |
| `@forgebot <anything else>` | Issue comment | Pass the full comment as a prompt to the current session (plan or build mode based on current state) |
| `@forgebot <anything>` | PR review comment | Resume session in build mode with the comment as prompt |

**Ignored entirely:**
- Comments from the forgebot Forgejo account itself (to prevent feedback loops)
- Webhooks for repositories not listed in config
- `@forgebot` mentions in issue/PR descriptions (not comments)

---

## 5. SQLite Schema

```sql
CREATE TABLE repos (
    id              TEXT PRIMARY KEY,
    full_name       TEXT NOT NULL UNIQUE,  -- e.g. "alice/myrepo"
    default_branch  TEXT NOT NULL,
    env_loader      TEXT NOT NULL DEFAULT 'none'
        CHECK(env_loader IN ('nix', 'direnv', 'none')),
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE sessions (
    id                  TEXT PRIMARY KEY,  -- internal UUID
    repo_full_name      TEXT NOT NULL REFERENCES repos(full_name),
    issue_id            INTEGER NOT NULL,
    pr_id               INTEGER,           -- NULL until PR is opened
    opencode_session_id TEXT NOT NULL,     -- e.g. "ses_42_alice_myrepo"
    worktree_path       TEXT NOT NULL,     -- absolute path on disk
    state               TEXT NOT NULL      -- see §8 State Machine
        CHECK(state IN ('planning','building','idle','busy','error')),
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(repo_full_name, issue_id)
);

CREATE TABLE pending_worktrees (
    -- Used during cleanup tracking; worktrees to remove on PR close/merge
    session_id    TEXT NOT NULL REFERENCES sessions(id),
    worktree_path TEXT NOT NULL,
    scheduled_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
```

**Notes:**
- The `repos` table is the sole source of truth for watched repositories. Repos are added, edited, and removed exclusively through the setup UI. There is no TOML seeding.
- `env_loader` defaults to `none` when a repo is first registered and can be changed at any time via the UI.
- `opencode_session_id` is derived deterministically: `ses_{issue_id}_{repo_owner}_{repo_name}` (lowercased, special chars stripped). This avoids storing a random ID from opencode — on first run we *create* a session with this ID; on subsequent runs we resume it.
- `UNIQUE(repo_full_name, issue_id)` enforces one session per issue.

---

## 6. Git Worktree Management

Each issue gets an isolated git worktree so concurrent sessions on different issues cannot interfere with each other.

### Lifecycle

| Event | Action |
|---|---|
| First `@forgebot` trigger on an issue | `git worktree add <worktree_base>/<repo>/<issue_id> -b agent/issue-<issue_id>` |
| Subsequent triggers on same issue | Reuse existing worktree path from SQLite |
| PR merged or closed webhook | Schedule worktree for removal; run `git worktree remove --force <path>` |

### Paths

```
<worktree_base>/          # configurable, e.g. /var/lib/forgebot/worktrees/
  alice_myrepo/
    42/                   # issue #42's worktree
    91/                   # issue #91's worktree
```

### Branch Naming

Branch name is always `agent/issue-{issue_id}`. This is the **primary link** used to match an incoming `pull_request.opened` webhook back to its issue — forgebot parses the head branch name and extracts the issue ID. No reliance on PR body content or agent behavior.

**forgebot is responsible for creating the worktree and branch.** opencode operates inside it but does not manage the worktree itself.

---

## 7. Forgejo opencode Tools

opencode supports first-class custom tools defined as TypeScript files using the `tool()` helper from `@opencode-ai/plugin`. These are automatically discovered by opencode and presented to the agent as structured, typed, validated callable tools — not shell scripts or commands. The agent calls them the same way it calls built-in tools like `bash` or `edit`.

forgebot ships three Forgejo tools as TypeScript files in its global opencode config directory (see §14). They are auto-discovered by opencode at startup. The `FORGEBOT_*` environment variables injected by forgebot (see §9) are available to the tool `execute` functions via `process.env`.

### Tool Definitions

Each tool lives at `<opencode_config_dir>/tools/<name>.ts`. They share a `package.json` in the config dir declaring `@opencode-ai/plugin` as a dependency, which opencode installs via `bun install` on startup.

**`tools/comment-issue.ts`**
```typescript
import { tool } from "@opencode-ai/plugin"

export default tool({
  description: "Post a markdown comment on a Forgejo issue",
  args: {
    body: tool.schema.string().describe("Markdown content of the comment"),
  },
  async execute(args) {
    const { FORGEBOT_FORGEJO_URL, FORGEBOT_FORGEJO_TOKEN, FORGEBOT_REPO, FORGEBOT_ISSUE_ID } = process.env
    const res = await fetch(
      `${FORGEBOT_FORGEJO_URL}/api/v1/repos/${FORGEBOT_REPO}/issues/${FORGEBOT_ISSUE_ID}/comments`,
      {
        method: "POST",
        headers: {
          "Authorization": `token ${FORGEBOT_FORGEJO_TOKEN}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ body: args.body }),
      }
    )
    return res.ok ? "Comment posted." : `Failed: ${res.status} ${await res.text()}`
  },
})
```

**`tools/comment-pr.ts`**
```typescript
import { tool } from "@opencode-ai/plugin"

export default tool({
  description: "Post a markdown comment on a Forgejo pull request",
  args: {
    pr_id: tool.schema.number().describe("The pull request number"),
    body: tool.schema.string().describe("Markdown content of the comment"),
  },
  async execute(args) {
    const { FORGEBOT_FORGEJO_URL, FORGEBOT_FORGEJO_TOKEN, FORGEBOT_REPO } = process.env
    const res = await fetch(
      `${FORGEBOT_FORGEJO_URL}/api/v1/repos/${FORGEBOT_REPO}/issues/${args.pr_id}/comments`,
      {
        method: "POST",
        headers: {
          "Authorization": `token ${FORGEBOT_FORGEJO_TOKEN}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ body: args.body }),
      }
    )
    return res.ok ? "Comment posted." : `Failed: ${res.status} ${await res.text()}`
  },
})
```

**`tools/create-pr.ts`**
```typescript
import { tool } from "@opencode-ai/plugin"

export default tool({
  description: "Open a pull request on Forgejo. The body must contain 'Closes #<issue_id>' on its own line.",
  args: {
    title: tool.schema.string().describe("Pull request title"),
    body: tool.schema.string().describe("Pull request body (markdown). Must include 'Closes #N' on its own line."),
    head: tool.schema.string().describe("Source branch name (e.g. agent/issue-42)"),
    base: tool.schema.string().describe("Target branch name (e.g. main)"),
  },
  async execute(args) {
    const { FORGEBOT_FORGEJO_URL, FORGEBOT_FORGEJO_TOKEN, FORGEBOT_REPO } = process.env
    const res = await fetch(
      `${FORGEBOT_FORGEJO_URL}/api/v1/repos/${FORGEBOT_REPO}/pulls`,
      {
        method: "POST",
        headers: {
          "Authorization": `token ${FORGEBOT_FORGEJO_TOKEN}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ title: args.title, body: args.body, head: args.head, base: args.base }),
      }
    )
    return res.ok ? "Pull request created." : `Failed: ${res.status} ${await res.text()}`
  },
})
```

### Discovery and Availability

opencode auto-discovers all `.ts` files in the global config `tools/` directory at startup. No agent configuration is required to make them available. They appear alongside built-in tools. The agent system prompt (§14) describes their purpose and expected usage patterns to improve consistency, but the tools are discoverable and callable by any agent without explicit instruction.

---

## 8. State Machine

Each session row has a `state` field:

```
           @forgebot plan/build received
                      |
                      v
[no session] ---- planning / building ---- opencode exits ---- idle
                      |                                          |
                  (busy check)                           next @forgebot
                      |                                          |
              state == busy?                                     v
                      |                              planning / building
                      v
              post "busy" comment
              drop webhook
```

| State | Meaning |
|---|---|
| `planning` | opencode is running in plan agent mode |
| `building` | opencode is running in build agent mode |
| `idle` | opencode has exited; session is ready for next trigger |
| `busy` | Synonym for planning/building used in busy-check logic |
| `error` | opencode exited non-zero; human intervention needed |

### Concurrency / Busy Handling

When a webhook arrives and the session state is `planning` or `building`:
1. forgebot posts a comment: *"🤖 forgebot is currently working on this issue. Please wait until it's done before sending another command."*
2. The webhook is dropped. No opencode invocation.

There is no queue in the POC. Queue-based handling is a v2 consideration.

---

## 9. opencode Invocation

### Session ID Strategy

opencode session IDs must start with `ses_`. forgebot derives a deterministic ID:

```
ses_{issue_id}_{sanitized_repo_owner}_{sanitized_repo_name}
# e.g. ses_42_alice_myrepo
```

On first invocation, opencode creates this session. On subsequent invocations, it resumes it. forgebot never needs to discover or store an opaque random session ID.

### CLI Invocations

**Plan phase:**
```bash
opencode run \
  --session ses_42_alice_myrepo \
  --agent plan \
  --cwd /var/lib/forgebot/worktrees/alice_myrepo/42 \
  --quiet \
  "<prompt constructed by forgebot — see §10>"
```

**Build phase:**
```bash
opencode run \
  --session ses_42_alice_myrepo \
  --agent build \
  --cwd /var/lib/forgebot/worktrees/alice_myrepo/42 \
  --quiet \
  "<prompt constructed by forgebot>"
```

**PR revision:**
```bash
opencode run \
  --session ses_42_alice_myrepo \
  --agent build \
  --cwd /var/lib/forgebot/worktrees/alice_myrepo/42 \
  --quiet \
  "<prompt constructed from PR review comments>"
```

forgebot awaits process exit. Exit code 0: set state `idle`. Non-zero: set state `error`, post a comment on the issue/PR.

### Environment Variables Injected

```
FORGEBOT_FORGEJO_URL
FORGEBOT_FORGEJO_TOKEN
FORGEBOT_REPO
FORGEBOT_ISSUE_ID
FORGEBOT_PR_ID            # if in revision phase
OPENCODE_CONFIG_DIR       # points to <opencode_config_dir> from config (custom agents/tools)
```

---

## 10. Environment Loader

Before spawning opencode, forgebot builds the subprocess environment by checking the repo's `env_loader` setting from the `repos` SQLite table. This runs against the worktree directory for the session.

### Loader Strategies

**`none` (default)**
No special environment loading. opencode inherits forgebot's own process environment plus the `FORGEBOT_*` variables injected per §9.

**`direnv`**
Runs `direnv export json` in the worktree root. direnv reads the `.envrc` file present in the repo and outputs the environment it would inject as JSON. forgebot parses this and merges the resulting key/value pairs into the opencode subprocess environment.

```
direnv export json
# -> {"PATH": "/nix/store/...", "GOPATH": "/home/user/go", ...}
```

Requires: `direnv` installed on the host and a `.envrc` present in the repo root.

**`nix`**
Runs `nix print-dev-env --json` in the worktree root. This evaluates the repo's `flake.nix` dev shell and outputs its environment. forgebot extracts only the exported string variables (ignoring bash arrays and functions, which are not meaningful to pass to a subprocess) and merges them into the opencode environment.

```
nix print-dev-env --json
# -> {"variables": {"PKG_CONFIG_PATH": {...}, "buildInputs": {...}, ...}}
```

Requires: `nix` with flakes enabled installed on the host and a `flake.nix` present in the repo root. Note that the first run after a flake change may be slow while Nix evaluates and fetches dependencies.

### Merge Order

Environment variables are applied in this order, with later entries winning on conflict:

1. forgebot's inherited process environment
2. Env loader output (`direnv` or `nix`)
3. `FORGEBOT_*` variables (always win — never overridable by loader output)

### Hard Failure

If the env loader command is not found on PATH, exits non-zero, or produces unparseable output, forgebot **does not proceed with the session**. It sets the session state to `error`, posts a comment on the issue explaining what failed, and stops:

*"❌ forgebot: env loader `direnv` failed and the session cannot continue. Fix the loader configuration and re-trigger when ready. Error output: `<stderr from the failed command>`"*

This is intentional — a failed env loader means the agent would run without the tools it needs, producing useless or broken output. Failing loudly is better than silently running in a broken environment.

---

## 11. Prompt Construction



forgebot constructs the full prompt string passed to `opencode run`. It is responsible for fetching issue/PR data from the Forgejo API and packaging it.

### Plan Phase Prompt Template

```
You are working on issue #<issue_id> in the repository <repo_full_name>.

## Issue: <issue_title>

<issue_body>

## Comments so far:
<comment_1_author> (<timestamp>): <comment_1_body>
<comment_2_author> (<timestamp>): <comment_2_body>
...

## Your task:
Analyse the issue and all comments. Post a detailed implementation plan as a
comment on the issue using the `comment-issue` tool.
Do NOT make any code changes yet. Use plan agent mode only.
```

### Build Phase Prompt Template

```
You are continuing work on issue #<issue_id> in <repo_full_name>.
You have already produced a plan (see session history).

## New instructions / comments since last run:
<new_comments_since_last_run>

## Your task:
Implement the plan. When your implementation is complete and tests pass,
open a pull request using the `create-pr` tool with body containing `Closes #<issue_id>`.
Then use the `comment-issue` tool to post a summary of what was done.
```

### PR Revision Prompt Template

```
Your pull request #<pr_id> for issue #<issue_id> has received review comments.

## Review comments:
<reviewer> on <file>:<line> (<timestamp>): <comment_body>
...

## Your task:
Address the review comments. Make the necessary changes, then force-push
to branch agent/issue-<issue_id>. Use the `comment-pr` tool to post a
comment on the PR when done.
```

---

## 12. Webhook Events Handled

| Forgejo event | Trigger condition | Action |
|---|---|---|
| `issue_comment` created | Comment contains `@forgebot`, issue is open | Dispatch to plan or build handler |
| `pull_request` opened | Head branch matches `agent/issue-*` | Update session row with PR ID |
| `pull_request` closed/merged | Head branch matches `agent/issue-*` | Schedule worktree cleanup |
| `pull_request_review_comment` created | Comment contains `@forgebot` | Dispatch to revision handler |

All other events: respond 200, take no action.

### Webhook Security

All incoming webhooks must include a valid HMAC-SHA256 signature in the `X-Gitea-Signature` header (Forgejo uses the same header as Gitea). forgebot verifies this against a shared secret from config before processing any payload. Requests with missing or invalid signatures return 401 and are logged.

---

## 13. Configuration

Config file location (in order of precedence):
1. Path specified via `--config` CLI flag
2. `./forgebot.toml`
3. `~/.config/forgebot/forgebot.toml`
4. `/etc/forgebot/forgebot.toml`

```toml
[server]
host = "0.0.0.0"
port = 8765
webhook_secret = "your-forgejo-webhook-secret"

[forgejo]
url = "https://git.example.com"
token = "your-forgejo-api-token"
# The Forgejo account forgebot posts comments as.
# Comments from this account are ignored to prevent loops.
bot_username = "forgebot"

[opencode]
binary = "opencode"          # or full path e.g. /usr/local/bin/opencode
worktree_base = "/var/lib/forgebot/worktrees"
# Global opencode config directory written by forgebot on first run.
# Contains the shared agent definition and Forgejo tool commands.
config_dir = "/var/lib/forgebot/opencode-config"

[database]
path = "/var/lib/forgebot/forgebot.db"
```

All sensitive values (`webhook_secret`, `token`) can be overridden with environment variables:
- `FORGEBOT_WEBHOOK_SECRET`
- `FORGEBOT_FORGEJO_TOKEN`

Repos are not declared in `forgebot.toml`. They are registered entirely through the setup UI and stored in SQLite. The TOML file is purely infrastructure config — connection details, paths, and credentials.

---

## 14. opencode Agent Configuration

forgebot maintains a **global opencode config directory** at the path set by `opencode.config_dir` in `forgebot.toml` (e.g. `/var/lib/forgebot/opencode-config`). This directory is written once on first startup and reused for every opencode invocation across all repos and sessions. Nothing is written into individual worktrees.

forgebot sets several environment variables to control where opencode looks for its configuration:

1. **`XDG_DATA_HOME`** - Controls where opencode stores its data files, including the critical `auth.json` file at `$XDG_DATA_HOME/opencode/auth.json` which contains API credentials.

2. **`XDG_CONFIG_HOME`** - Controls where opencode looks for global configuration at `$XDG_CONFIG_HOME/opencode/opencode.json`.

3. **`OPENCODE_CONFIG_DIR`** - Points to `<config_dir>/opencode/.opencode` for custom agents, tools, and plugins specific to forgebot.

The systemd service sets these to paths under the forgebot data directory, ensuring isolation from the user's personal opencode configuration.

### Directory Layout

```
<opencode_config_dir>/
├── package.json           # declares @opencode-ai/plugin dependency; bun installs on startup
├── agents/
│   └── forgebot.md        # the forgebot agent definition
└── tools/
    ├── comment-issue.ts   # Forgejo tool: post issue comment
    ├── comment-pr.ts      # Forgejo tool: post PR comment
    └── create-pr.ts       # Forgejo tool: open a pull request
```

forgebot writes all of these files on first startup if they do not exist. Existing files are left untouched so operators can customise them without having changes overwritten on restart. opencode runs `bun install` automatically at startup to install `@opencode-ai/plugin` from `package.json`.

### Agent Definition

**`agents/forgebot.md`**:

```markdown
---
description: "forgebot coding agent — implements Forgejo issues and responds to PR reviews"
tools:
  bash: true
  edit: true
  write: true
  webfetch: false
permissions:
  bash: allow
  edit: allow
---

You are forgebot, an autonomous coding agent working inside a git worktree.

## Forgejo Tools
You have the following custom tools available for interacting with Forgejo.
They are strongly typed and validated — prefer them over any other approach.

- `comment-issue` — post a markdown comment on the current issue
- `comment-pr` — post a markdown comment on a pull request (requires pr_id)
- `create-pr` — open a pull request (title, body, head branch, base branch)

Always post a comment-issue when you begin significant work and when you finish.
The issue ID is in $FORGEBOT_ISSUE_ID. The PR ID (if in revision phase) is in $FORGEBOT_PR_ID.

## Git
- Your branch is `agent/issue-$FORGEBOT_ISSUE_ID`. It already exists; do not create it.
- Always commit your changes with descriptive messages.
- Do not push unless you are opening a PR or responding to a PR review.

## Pull Requests
- Open a PR only when you believe the implementation is complete.
- PR body must contain `Closes #$FORGEBOT_ISSUE_ID` on its own line.
- Branch to PR against is the repo's default branch.

## Constraints
- Do not modify files outside the current worktree.
- Do not install global packages or modify system config.
```

---

## 15. POC Scope — In / Out

### In Scope

- [ ] Rust binary `forgebot`
- [ ] Axum webhook server with HMAC verification
- [ ] SQLite state management (`repos`, `sessions`, `pending_worktrees`)
- [ ] Forgejo API client: get issue, list comments, list PR review comments, post comment, create PR (used by forgebot itself for lifecycle comments and webhook registration)
- [ ] Git worktree creation and cleanup orchestration
- [ ] opencode subprocess lifecycle management (spawn, await, state transitions)
- [ ] Global opencode config directory setup on startup (agent definition + TypeScript Forgejo tool files + package.json)
- [ ] `@forgebot plan` and `@forgebot build` on issues
- [ ] `@forgebot` on PR review comments (revision phase)
- [ ] PR opened webhook -> attach PR ID to session
- [ ] PR merged/closed webhook -> schedule worktree cleanup
- [ ] Busy-state rejection with comment
- [ ] Error state detection and comment
- [ ] TOML config file with env var overrides (no repo config in TOML)
- [ ] Per-repo environment loader configuration (`none`, `direnv`, `nix`) stored in SQLite, set via UI
- [ ] Environment loader execution (`direnv export json` / `nix print-dev-env --json`) before each opencode run, with hard failure on error
- [ ] Repo management entirely through the setup UI (add, view, configure)
- [ ] `flake.nix` exposing the forgebot package, a NixOS module, and a dev shell
- [ ] NixOS module (`nix/module.nix`) defining a systemd service with user/group/dataDir management
- [ ] README with full NixOS deployment instructions as the primary deployment path

### Out of Scope (POC)

- Agent-triggered web UI or dashboard (the setup UI is operator-only, not user-facing)
- Triggering from PR descriptions or issue descriptions (comments only)
- Sessions initiated from a PR that forgebot didn't create (hard fail)
- Prompt queueing / debouncing
- Multi-repo fan-out (repos are individually configured)
- Notification/alerting beyond Forgejo comments
- opencode session export/import or backup
- Support for non-default base branches (hardcoded to config `default_branch`)

---

## 16. Repo Setup UI

Because setting up a new repository involves several manual steps (cloning, configuring the Forgejo webhook, verifying the local clone is in place), forgebot exposes a minimal read-only web UI on the same port as the webhook server. This is an operator-facing tool — not user-facing — intended to make initial setup and health-checking straightforward without requiring the operator to inspect config files and the filesystem by hand.

### What It Is

A small set of HTML pages served by axum at a separate route prefix (`/ui`). No JavaScript framework — plain server-rendered HTML is sufficient. It is not protected by authentication in the POC (operator is assumed to be running this on a local/trusted network), but a config flag to disable it entirely should be provided.

### Pages

**`/ui` — Dashboard / Repo List**

Shows all repositories currently registered in the `repos` SQLite table and their setup status. For each repo:

- Repo full name and default branch
- Local clone status: present / missing (checked by probing `<worktree_base>/<owner>_<repo>/.git`)
- Forgejo webhook status: registered / not registered (checked by calling the Forgejo API to list webhooks on the repo and looking for one pointing at forgebot's webhook URL)
- Active session count: how many issues currently have a session row in SQLite
- A "Setup" button/link that goes to the per-repo setup page

At the bottom of the dashboard, an **Add Repository** form with three fields:
- Full repo name (e.g. `alice/myrepo`)
- Default branch (e.g. `main`)
- Environment loader — radio group: None / direnv / Nix dev shell

Submitting this form inserts a new row into the `repos` table and redirects to the per-repo setup page to complete the remaining steps.

**`/ui/repo/:owner/:name` — Per-Repo Setup Page**

Displays step-by-step setup instructions for a single repository, with live status indicators for each step:

1. **Clone the repository locally**
   - Shows the exact `git clone --bare` command the operator should run, with the correct target path
   - Status: green if the clone exists on disk, red with instructions if not

2. **Register the Forgejo webhook**
   - Shows the exact webhook URL (`https://<forgebot_host>/webhook`) and secret to enter in Forgejo
   - Provides a "Register webhook automatically" button that calls the Forgejo API to create the webhook without the operator having to do it manually
   - Status: green if webhook is registered and pointing at the correct URL, red otherwise

3. **Configure environment loader**
   - A radio group allowing the operator to select how forgebot should build the environment before running opencode for this repo:
     - ( ) **None** — use system environment as-is
     - ( ) **direnv** — source `.envrc` via `direnv export json` *(requires `direnv` on host and `.envrc` in repo root)*
     - ( ) **Nix dev shell** — evaluate `flake.nix` via `nix print-dev-env --json` *(requires `nix` with flakes enabled on host and `flake.nix` in repo root; first run may be slow)*
   - A **Save** button that POSTs the selection and updates the `env_loader` column in the `repos` table
   - A **Test environment** button (enabled once a loader other than `none` is saved) that runs the selected loader against the local clone immediately and returns either a list of the environment variable *keys* it found (values are not shown, as they may contain secrets) or the raw error output if the loader failed. This gives the operator confidence the loader works before triggering any agent runs.

4. **Verify configuration**
   - Confirms the Forgejo API token has sufficient permissions (checks by attempting a test API call: list issues on the repo)
   - Confirms `opencode` binary is found at the configured path
   - Confirms the global opencode config directory exists and contains the expected `agents/forgebot.md`, `tools/` TypeScript files, and `package.json`

**`/ui/sessions` — Active Sessions**

A simple table of all rows in the `sessions` SQLite table, showing:

- Repo, issue ID, PR ID (if set), current state, worktree path, last updated timestamp
- No actions from this page in the POC — read-only

### What It Is Not

- Not a way to trigger agent runs
- Not a way to view session history or opencode output logs
- Not a way to edit connection config (Forgejo URL, token, etc. — those remain TOML only)
- Not accessible to issue authors or PR reviewers — purely for the operator setting up and monitoring the service

### Routes Added to Axum

```
GET  /ui                                   -> repo list dashboard
POST /ui/repos                             -> add repo (inserts into repos table, redirects to setup page)
GET  /ui/repo/:owner/:name                 -> per-repo setup page
POST /ui/repo/:owner/:name/webhook         -> register webhook via Forgejo API (form action)
POST /ui/repo/:owner/:name/env-loader      -> save env_loader selection to repos table
POST /ui/repo/:owner/:name/test-env        -> run env loader, return key list or error output
GET  /ui/sessions                          -> active sessions table
```

The existing `POST /webhook` route is unchanged.

---

## 17. Project Structure

```
forgebot/
├── Cargo.toml
├── flake.nix                 # Nix flake: package, NixOS module, dev shell
├── flake.lock
├── nix/
│   ├── module.nix            # NixOS module (systemd service, user/group, dataDir)
│   └── overlay.nix           # Nix package definition for forgebot binary
├── forgebot.toml.example
├── src/
│   ├── main.rs               # CLI entrypoint, config loading, startup (writes opencode config dir)
│   ├── config.rs             # Config structs, TOML parsing, env overrides
│   ├── db.rs                 # SQLite setup, migrations, session + repo CRUD
│   ├── webhook/
│   │   ├── mod.rs            # Axum router, HMAC verification middleware
│   │   ├── handlers.rs       # Route handlers per event type
│   │   └── models.rs         # Forgejo webhook payload structs
│   ├── forgejo/
│   │   ├── mod.rs            # reqwest-based API client
│   │   └── models.rs         # Forgejo API response structs
│   ├── session/
│   │   ├── mod.rs            # Session state machine, session lookup helpers
│   │   ├── worktree.rs       # Git worktree create/remove orchestration
│   │   ├── env_loader.rs     # direnv / nix / none env resolution (hard fail on error)
│   │   └── opencode.rs       # opencode subprocess spawn, prompt construction
│   └── ui/
│       ├── mod.rs            # Axum routes for /ui prefix
│       └── handlers.rs       # Dashboard, add repo, repo setup, sessions table, test-env handlers
├── opencode-config/          # Template files written to opencode_config_dir on startup
│   ├── package.json          # @opencode-ai/plugin dep; bun install run by opencode at startup
│   ├── agents/
│   │   └── forgebot.md       # Agent definition with Forgejo tool usage instructions
│   └── tools/
│       ├── comment-issue.ts  # Forgejo tool: post issue comment
│       ├── comment-pr.ts     # Forgejo tool: post PR comment
│       └── create-pr.ts      # Forgejo tool: open pull request
├── templates/                # Server-rendered HTML templates (askama or minijinja)
│   ├── base.html
│   ├── dashboard.html        # Includes add-repo form
│   ├── repo_setup.html       # Clone instructions, webhook button, env loader, verify
│   └── sessions.html
└── migrations/
    ├── 001_initial.sql       # sessions + pending_worktrees schema
    └── 002_repos.sql         # repos table
```

---

## 18. Key Implementation Notes for the Coding Agent

1. **HMAC verification must happen before any payload parsing.** Use axum middleware/extractor that reads the raw body bytes, verifies signature, then passes bytes to handlers for deserialization.

2. **All opencode invocations are blocking from forgebot's perspective.** Spawn via `tokio::process::Command`, `.await` on the child's exit status. The HTTP handler should return 200 to Forgejo immediately and dispatch the opencode work to a `tokio::spawn` task — do not make Forgejo wait for opencode to finish.

3. **State must be set to `planning`/`building` before spawning opencode**, not after. This prevents a race where a second webhook arrives in the tiny window between spawn and state update.

4. **Deterministic session IDs:** strip all non-alphanumeric characters from owner/repo names before constructing `ses_{issue_id}_{owner}_{repo}`. Keep it lowercase.

5. **Global opencode config dir is written on startup, not per worktree.** On startup, forgebot checks whether `<opencode_config_dir>/agents/forgebot.md` and the three command files exist. If any are missing, it writes them from the embedded templates in the `opencode-config/` source directory. Existing files are not overwritten, allowing operators to customise them.

6. **Forgejo comment loop prevention:** before dispatching any webhook, check if the comment author matches `config.forgejo.bot_username`. If so, return 200 and do nothing.

7. **Worktree creation requires the repo to be cloned locally first.** forgebot needs a local bare clone of each watched repo. When a first trigger arrives for a repo, forgebot checks for the clone at `<worktree_base>/<owner>_<repo>` and hard-fails with a Forgejo comment if it is missing. Cloning is not automated — the operator must run `git clone --bare` manually after registering a repo, as documented in the README and shown on the setup UI page.

8. **PR ID attachment is eventually consistent.** Between the moment opencode creates a PR and the moment forgebot receives the `pull_request.opened` webhook, the session row has no PR ID. This is fine — handle it gracefully if a PR review comment arrives before the webhook is processed (extremely unlikely, but log a warning and look up PR ID from Forgejo API as fallback).

9. **Repos are managed entirely through SQLite.** There is no TOML seeding. On startup forgebot does not touch the `repos` table. All repo registration happens through the UI's add-repo form.

10. **Env loader runs before the opencode `Command` is built** and its output is merged into the environment map before `FORGEBOT_*` vars are added. The `FORGEBOT_*` vars must always take final precedence. For the `nix` loader, the `variables` object in `nix print-dev-env --json` output contains entries of different types — only extract entries where `type == "exported"` and `value` is a plain string. On any failure (binary not found, non-zero exit, parse error), immediately set session state to `error`, post the error output to the Forgejo issue, and do not spawn opencode.

11. **The test-env UI route runs the loader synchronously and streams or returns the result directly** — no background task needed since it is an explicit operator action and the operator is waiting for the response. Cap execution time with a timeout (e.g. 30 seconds) and return an error if exceeded, since `nix print-dev-env` can hang on bad flakes.

---

## 19. Deployment — NixOS (Happy Path)

The primary supported deployment target for the POC is a NixOS VM. Other platforms (any Linux distro, macOS) should work given the single-binary nature of the app, but are not documented and not tested. The README must treat NixOS as the default and document it fully.

### What the Agent Should Build

The repository must ship a `nix/` directory containing everything needed for a first-class NixOS deployment:

```
nix/
├── module.nix        # NixOS module — forgebot as a systemd service
└── overlay.nix       # Nix package definition for the forgebot binary
flake.nix             # Flake exposing the package, NixOS module, and dev shell
```

**`flake.nix`** should expose:
- `packages.${system}.forgebot` — the compiled forgebot binary, built via `naersk` or `crane` (standard Rust-in-Nix builders)
- `nixosModules.forgebot` — the NixOS module from `nix/module.nix`
- `devShells.${system}.default` — a dev shell with `rustc`, `cargo`, `rust-analyzer`, `sqlx-cli`, and `opencode` available for local development

**`nix/module.nix`** should provide a NixOS module with options covering at minimum:

```nix
services.forgebot = {
  enable = mkEnableOption "forgebot";
  package = mkOption { ... };          # defaults to flake package
  configFile = mkOption { type = path; };  # path to forgebot.toml
  dataDir = mkOption { default = "/var/lib/forgebot"; };
  user = mkOption { default = "forgebot"; };
  group = mkOption { default = "forgebot"; };
};
```

The module should:
- Create a dedicated `forgebot` system user and group
- Create the `dataDir` with correct ownership
- Define a `systemd.services.forgebot` unit that runs `forgebot --config ${cfg.configFile}`, restarts on failure, and runs as the `forgebot` user
- Ensure `opencode` and any other runtime dependencies are in the service's `PATH` via `systemd.services.forgebot.environment` or `serviceConfig.Environment`

### README Deployment Instructions

The README must include a deployment section covering the NixOS path as the primary guide:

1. **Add forgebot to your flake inputs** — show the exact `inputs.forgebot.url` line
2. **Import the NixOS module** — show how to add it to `nixosModules` imports
3. **Configure the service** — show a minimal `services.forgebot` block with `enable = true` and `configFile` pointing to a sops-nix secret or a plain path
4. **Create `forgebot.toml`** — link to `forgebot.toml.example` and explain each required field (server, forgejo, opencode, database sections only — no repo config)
5. **Apply the configuration** — `nixos-rebuild switch`
6. **Register repos in the UI** — navigate to `http://<host>:8765/ui`, use the Add Repository form to register each repo with its default branch and env loader selection
7. **Clone watched repos** — the setup page shows the exact `git clone --bare` command; this must be run manually for each repo before forgebot will accept webhooks for it
8. **Complete setup in the UI** — use the per-repo setup page to register the Forgejo webhook (one-click) and run the env loader test if applicable
9. **Verify** — show how to check `journalctl -u forgebot` for startup logs

The README should also include a brief note that deployment on other Linux systems is possible by running the binary directly or writing a systemd unit by hand, but those paths are not officially documented for the POC.
