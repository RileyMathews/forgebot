#!/usr/bin/env python3

import json
import os
import sqlite3
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, cast


FORGEBOT_BASE_URL = "http://127.0.0.1:8765"
WEBHOOK_TARGET_URL = "http://ds9:8765/webhook"
REPO_OWNER = "riley"
REPO_NAME = "terminal-config"
REPO_FULL_NAME = f"{REPO_OWNER}/{REPO_NAME}"
DEFAULT_BRANCH = "main"
ENV_LOADER = "none"
RUNTIME_ROOT = Path.home() / ".local/state/forgebot-local-dev"
DB_PATH = RUNTIME_ROOT / "forgebot.db"

ISSUE_TITLE = "Forgebot E2E smoke test (manual gates)"
ISSUE_BODY = (
    "This is an automated local Forgebot E2E smoke test issue.\n\n"
    "Goal: validate issue -> plan -> implementation -> PR workflow with manual checkpoints."
)
PLAN_COMMENT = "@forgebot please create a short plan for this smoke test issue."


def run_cmd(
    command: list[str], *, check: bool = True
) -> subprocess.CompletedProcess[str]:
    print(f"$ {' '.join(command)}")
    result = subprocess.run(command, text=True, capture_output=True)
    if result.stdout.strip():
        print(result.stdout.strip())
    if result.stderr.strip():
        print(result.stderr.strip(), file=sys.stderr)
    if check and result.returncode != 0:
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(command)}")
    return result


def forgejo_request(
    method: str,
    path: str,
    token: str,
    *,
    payload: dict | None = None,
) -> dict[str, Any] | list[dict[str, Any]]:
    base_url = os.environ["FORGEBOT_FORGEJO_URL"].rstrip("/")
    url = f"{base_url}{path}"
    data = None
    headers = {
        "Authorization": f"token {token}",
        "Accept": "application/json",
    }

    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"

    request = urllib.request.Request(url=url, method=method, data=data, headers=headers)
    try:
        with urllib.request.urlopen(request) as response:
            body = response.read()
    except urllib.error.HTTPError as error:
        details = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(
            f"Forgejo API error {error.code} on {path}: {details}"
        ) from error

    if not body:
        return {}
    decoded = json.loads(body.decode("utf-8"))
    if isinstance(decoded, dict):
        return cast(dict[str, Any], decoded)
    if isinstance(decoded, list):
        return cast(list[dict[str, Any]], decoded)
    raise RuntimeError(f"unexpected JSON response shape for {path}")


def forgebot_post(path: str, form: dict[str, str]) -> int:
    encoded = urllib.parse.urlencode(form).encode("utf-8")
    request = urllib.request.Request(
        url=f"{FORGEBOT_BASE_URL}{path}",
        method="POST",
        data=encoded,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
    )
    try:
        with urllib.request.urlopen(request) as response:
            return response.getcode()
    except urllib.error.HTTPError as error:
        details = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(
            f"Forgebot HTTP error {error.code} on {path}: {details}"
        ) from error


def wait_for_forgebot_ready(timeout_seconds: int = 180) -> None:
    deadline = time.time() + timeout_seconds
    request = urllib.request.Request(url=f"{FORGEBOT_BASE_URL}/", method="GET")

    while time.time() < deadline:
        try:
            with urllib.request.urlopen(request) as response:
                if response.getcode() == 200:
                    print("Forgebot HTTP server is ready.")
                    return
        except urllib.error.HTTPError as error:
            if error.code < 500:
                print(f"Forgebot HTTP server responded with {error.code}; continuing.")
                return
        except urllib.error.URLError:
            pass

        time.sleep(2)

    raise TimeoutError("timed out waiting for Forgebot HTTP server to become ready")


def wait_for_clone_ready(repo_full_name: str, timeout_seconds: int = 180) -> None:
    deadline = time.time() + timeout_seconds
    while time.time() < deadline:
        if not DB_PATH.exists():
            time.sleep(2)
            continue

        with sqlite3.connect(DB_PATH) as conn:
            row = conn.execute(
                """
                select clone_status, coalesce(clone_attempts, 0), coalesce(clone_error, '')
                from repos
                where full_name = ?
                """,
                (repo_full_name,),
            ).fetchone()

        if row is None:
            time.sleep(2)
            continue

        clone_status, clone_attempts, clone_error = row
        print(
            f"clone_status={clone_status} clone_attempts={clone_attempts} "
            f"clone_error={clone_error}"
        )

        if clone_status == "ready":
            return
        if clone_status == "failed":
            raise RuntimeError(f"repository clone failed: {clone_error}")

        time.sleep(2)

    raise TimeoutError("timed out waiting for repository clone to become ready")


def prompt_yes_no(message: str) -> bool:
    while True:
        answer = input(f"{message} [y/n]: ").strip().lower()
        if answer == "y":
            return True
        if answer == "n":
            return False
        print("Please type 'y' or 'n'.")


def open_url(url: str) -> None:
    if not shutil_which("xdg-open"):
        print(f"xdg-open not found; open this URL manually: {url}")
        return

    try:
        run_cmd(["xdg-open", url], check=False)
    except Exception as error:
        print(f"Warning: failed to open browser automatically: {error}")
        print(f"Open this URL manually: {url}")


def shutil_which(binary: str) -> str | None:
    for path in os.environ.get("PATH", "").split(os.pathsep):
        candidate = Path(path) / binary
        if candidate.exists() and os.access(candidate, os.X_OK):
            return str(candidate)
    return None


def cleanup(token: str) -> None:
    print("Running cleanup...")
    run_cmd(["process-compose", "down"], check=False)
    DB_PATH.unlink(missing_ok=True)

    hooks_result = forgejo_request(
        "GET",
        f"/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/hooks",
        token,
    )
    if not isinstance(hooks_result, list):
        raise RuntimeError("expected hook list response from Forgejo API")

    removed = 0
    for hook in hooks_result:
        hook_url = hook.get("config", {}).get("url")
        if hook_url == WEBHOOK_TARGET_URL:
            hook_id = hook["id"]
            forgejo_request(
                "DELETE",
                f"/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/hooks/{hook_id}",
                token,
            )
            removed += 1
    print(f"Removed webhook entries: {removed}")


def main() -> int:
    forgejo_token = os.environ.get("FORGEJO_TOKEN")
    forgejo_url = os.environ.get("FORGEBOT_FORGEJO_URL")

    if not forgejo_token or not forgejo_url:
        print(
            "Missing required environment variables: "
            "FORGEBOT_FORGEJO_URL and FORGEJO_TOKEN",
            file=sys.stderr,
        )
        return 1

    print("== Forgebot E2E smoke test (manual gates) ==")

    run_cmd(["process-compose", "down"], check=False)
    DB_PATH.unlink(missing_ok=True)
    run_cmd(["process-compose", "up", "-D"])
    wait_for_forgebot_ready()

    print("Adding repository in Forgebot UI backend...")
    add_status = forgebot_post(
        "/repos",
        {
            "full_name": REPO_FULL_NAME,
            "default_branch": DEFAULT_BRANCH,
            "env_loader": ENV_LOADER,
        },
    )
    print(f"Add repo response status: {add_status}")

    print("Waiting for repository clone to become ready...")
    wait_for_clone_ready(REPO_FULL_NAME)

    print("Registering webhook...")
    webhook_status = forgebot_post(f"/repo/{REPO_OWNER}/{REPO_NAME}/webhook", {})
    print(f"Register webhook response status: {webhook_status}")

    issue_payload = {"title": ISSUE_TITLE, "body": ISSUE_BODY}
    issue_result = forgejo_request(
        "POST",
        f"/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/issues",
        forgejo_token,
        payload=issue_payload,
    )
    if not isinstance(issue_result, dict):
        raise RuntimeError("expected issue object response from Forgejo API")

    issue_number = int(issue_result["number"])
    issue_url = str(issue_result["html_url"])
    print(f"Created issue #{issue_number}: {issue_url}")

    forgejo_request(
        "POST",
        f"/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/issues/{issue_number}/comments",
        forgejo_token,
        payload={"body": PLAN_COMMENT},
    )
    print("Posted planning trigger comment.")

    open_url(issue_url)
    if not prompt_yes_no("confirm the bot has made a plan"):
        print("Plan confirmation was 'n'. Leaving runtime active for troubleshooting.")
        return 2

    proceed_comment = (
        "@forgebot proceed with implementation and open a PR for this issue. "
        f"Include `Closes #{issue_number}` in the PR body."
    )
    forgejo_request(
        "POST",
        f"/api/v1/repos/{REPO_OWNER}/{REPO_NAME}/issues/{issue_number}/comments",
        forgejo_token,
        payload={"body": proceed_comment},
    )
    print("Posted implementation trigger comment.")
    print(f"Issue URL: {issue_url}")
    print(f"PR list URL: {forgejo_url.rstrip('/')}/{REPO_OWNER}/{REPO_NAME}/pulls")

    if prompt_yes_no("confirm the bot has made a PR"):
        cleanup(forgejo_token)
        print("Smoke test completed successfully with cleanup.")
        return 0

    print("Final confirmation was 'n'. Leaving runtime active for troubleshooting.")
    return 3


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("Interrupted. Leaving runtime active for troubleshooting.")
        raise SystemExit(130)
    except Exception as error:
        print(f"ERROR: {error}", file=sys.stderr)
        raise SystemExit(1)
