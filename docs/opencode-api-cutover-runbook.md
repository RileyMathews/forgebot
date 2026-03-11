# OpenCode API Cutover Runbook

This runbook documents production operation with the OpenCode HTTP API transport.

## Defaults

- Default API endpoint is `http://127.0.0.1:4096` (`FORGEBOT_OPENCODE_API_BASE_URL`).

## Required Environment

Set these values explicitly in production even when defaults match:

```bash
FORGEBOT_OPENCODE_API_BASE_URL=http://127.0.0.1:4096
# Optional when API is authenticated
FORGEBOT_OPENCODE_API_TOKEN=...
FORGEBOT_OPENCODE_API_TIMEOUT_SECS=30
```

## Health Checks

`forgebot` performs an OpenCode API startup health check on startup.

- Success log contains: `OpenCode API startup health check passed`.
- Failure blocks startup and should be treated as a cutover stop condition.

Operational checks:

```bash
systemctl status forgebot
journalctl -u forgebot -n 100 --no-pager
curl -fsS http://127.0.0.1:4096/global/health
```

## Canary and Promotion Flow

1. Run canary repos with real issue -> plan -> implementation traffic.
2. Confirm immediate session-link acknowledgement comments are posted.
3. Confirm trigger admission rejects overlapping requests with busy/retry messaging.
4. Promote from canary repos to all repos after successful repeated runs.

## Known Limitations

- API mode remains fire-and-forget (`prompt_async`) with no internal event stream orchestration.
- Admission control is status-based and accepts a small TOCTOU race under low traffic.
- There is no per-issue queue/lock in this MVP.

## Branch Replacement Readiness (`feature/dev` -> `main`)

Go/no-go checklist:

- [ ] `just verify` passes on `feature/dev`.
- [ ] Local smoke issue -> plan -> build succeeds on latest `feature/dev` head.
- [ ] Production smoke issue -> plan -> implementation succeeds.
- [ ] Runbook has no unknowns for on-call (env values, logs, restart commands).

Replacement steps:

1. Freeze new merges to `main`.
2. Ensure `feature/dev` is up to date with required fixes.
3. Merge or fast-forward `main` to `feature/dev` per repo policy.
4. Verify webhook-driven smoke flow on `main` immediately after replacement.
5. Keep restart/health-check recovery steps documented and available.
