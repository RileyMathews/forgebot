# OpenCode API Cutover Runbook

This runbook documents the production cutover path from `opencode run` (CLI transport) to the OpenCode HTTP API transport.

## Defaults and Rollback Posture

- Default transport is `api` (`FORGEBOT_OPENCODE_TRANSPORT=api`).
- Default API endpoint is `http://127.0.0.1:4096` (`FORGEBOT_OPENCODE_API_BASE_URL`).
- CLI transport remains available as rollback compatibility mode.
- Rollback is immediate: set `FORGEBOT_OPENCODE_TRANSPORT=cli` and restart `forgebot`.

## Required Environment

Set these values explicitly in production even when defaults match:

```bash
FORGEBOT_OPENCODE_TRANSPORT=api
FORGEBOT_OPENCODE_API_BASE_URL=http://127.0.0.1:4096
# Optional when API is authenticated
FORGEBOT_OPENCODE_API_TOKEN=...
FORGEBOT_OPENCODE_API_TIMEOUT_SECS=30
```

## Health Checks

`forgebot` performs an OpenCode API startup health check when transport is `api`.

- Success log contains: `OpenCode API startup health check passed`.
- Failure blocks startup and should be treated as a cutover stop condition.

Operational checks:

```bash
systemctl status forgebot
journalctl -u forgebot -n 100 --no-pager
curl -fsS http://127.0.0.1:4096/global/health
```

## Canary and Promotion Flow

1. Run canary repos in API mode with real issue -> plan -> build traffic.
2. Confirm immediate session-link acknowledgement comments are posted.
3. Confirm trigger admission rejects overlapping requests with busy/retry messaging.
4. Promote from canary repos to all repos after successful repeated runs.

## Rollback Drill

Run this at least once before broad rollout:

1. Set `FORGEBOT_OPENCODE_TRANSPORT=cli`.
2. Restart `forgebot`.
3. Trigger `@forgebot` in a test issue and confirm issue -> PR flow still works.
4. Restore `FORGEBOT_OPENCODE_TRANSPORT=api` and restart.
5. Re-run a smoke issue to confirm recovery back to API mode.

## Known Limitations

- API mode remains fire-and-forget (`prompt_async`) with no internal event stream orchestration.
- Admission control is status-based and accepts a small TOCTOU race under low traffic.
- There is no per-issue queue/lock in this MVP.

## Branch Replacement Readiness (`feature/dev` -> `main`)

Go/no-go checklist:

- [ ] `just verify` passes on `feature/dev`.
- [ ] Local smoke issue -> plan -> build succeeds on latest `feature/dev` head.
- [ ] Production smoke issue -> plan -> build succeeds in API mode.
- [ ] Rollback drill (`api` -> `cli` -> `api`) succeeds in production-like environment.
- [ ] Runbook has no unknowns for on-call (env values, logs, rollback commands).

Replacement steps:

1. Freeze new merges to `main`.
2. Ensure `feature/dev` is up to date with required fixes.
3. Merge or fast-forward `main` to `feature/dev` per repo policy.
4. Verify webhook-driven smoke flow on `main` immediately after replacement.
5. Keep rollback transport override (`cli`) documented and available.
