# Agent Developer Experience Pain Points (Issue -> PR E2E)

Date: 2026-03-10

Repository used for E2E: `riley/terminal-config`

## What I ran

1. Started local stack with `process-compose up -D`.
2. Verified app/UI came up on `http://127.0.0.1:8765/ui`.
3. Configured watched repo in the UI and registered webhook.
4. Created Forgejo issue and posted `@forgebot plan` then `@forgebot build`.
5. Verified PR was created: `https://git.rileymathews.com/riley/terminal-config/pulls/3`.

## Pain points and friction

### 1) Existing repo in `pending` clone state can get stuck forever

- I started with `riley/terminal-config` already in the UI with clone status `pending` and no active clone task.
- `POST /ui/repo/:owner/:name/retry-clone` did nothing for `pending` rows (only works if status is `failed`).
- Result: setup looked blocked until I removed/re-added the repo.

Why this hurts agent UX:
- Agents do not get a clear recovery path from stale `pending` state.
- It feels like setup is broken even though a retry endpoint exists.

### 2) Retry endpoint behavior and naming mismatch

- Handler comment says "Retry a failed or pending clone", but DB update only matches `clone_status = 'failed'`.
- This mismatch made debugging harder and adds confusion when automating setup.

Why this hurts agent UX:
- Agent automation logic cannot trust status transition behavior implied by route docs/comments.

### 3) Forgejo webhook delivery did not naturally drive the flow in this local run

- Creating issue comments in Forgejo (`@forgebot plan`, `@forgebot build`) did not create sessions in local forgebot.
- To continue E2E, I had to manually post signed `issue_comment` webhook payloads to `POST /webhook`.

Why this hurts agent UX:
- Manual webhook simulation breaks a true push-button local E2E loop.
- Agents need extra out-of-band steps and custom scripting to proceed.

### 4) Webhook secret is rendered directly in the setup UI

- The repo setup page displays the raw webhook secret value in plaintext.

Why this hurts agent UX:
- Increases risk of accidental secret exposure in recordings/screenshots/log captures.
- Makes it harder to safely share debugging artifacts.

### 5) Limited first-class observability for webhook failures

- There is no obvious UI surface for "last webhook delivery received/failed" in forgebot.
- I had to inspect process-compose logs and database state manually to infer what happened.

Why this hurts agent UX:
- Agents lose time triangulating whether failures are network, signature, filtering, or session-dispatch issues.

## Notes on successful outcome

- After removing/re-adding the repo, clone completed and webhook registration succeeded.
- Plan and build completed when signed webhook payloads were sent directly to local forgebot.
- End-to-end issue-to-PR behavior did complete once webhook triggering was unblocked.

## Follow-up after binding server to `0.0.0.0`

I reran E2E after setting `FORGEBOT_SERVER_HOST=0.0.0.0` for local runtime.

Observed:
- Remote tailnet host could connect to `http://ds9:8765/webhook` (got `405 Method Not Allowed` on GET, which confirms TCP/HTTP reachability).
- Forgejo-triggered comments (`@forgebot plan`, `@forgebot build`) now reached forgebot without manual webhook simulation.
- Issue-to-PR E2E completed successfully via natural webhook delivery: `https://git.rileymathews.com/riley/terminal-config/pulls/5`.

## Remaining pain points (after host bind fix)

### 6) Easy-to-miss host binding mismatch

- A default `127.0.0.1` bind is fine for local browser testing but breaks webhook delivery from other tailnet hosts.
- The UI advertises `http://ds9:8765/webhook` even when server bind address is loopback-only.

Why this hurts agent UX:
- It creates a confusing false-positive setup state (URL looks right, DNS resolves, but connections are refused).
- Agents lose time debugging network paths instead of getting immediate configuration feedback.

### 7) No explicit connectivity preflight check for webhook endpoint

- Setup verifies token/binary/config, but not whether the configured webhook target is remotely reachable.

Why this hurts agent UX:
- A one-click "test webhook delivery" or "ingress reachable" check would catch the bind issue instantly.

## Cleanup performed for second run

- Removed local test DB: `~/.local/state/forgebot-local-dev/forgebot.db`.
- Removed Forgejo test webhook entry targeting `http://ds9:8765/webhook`.
