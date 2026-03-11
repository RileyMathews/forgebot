//! opencode configuration directory setup and subprocess invocation
//!
//! This module handles:
//! 1. Writing the global opencode config directory on startup
//! 2. Spawning opencode subprocesses with proper environment
//! 3. Session orchestration and lifecycle management
//! 4. Crash recovery on startup

use crate::config::{Config, OpencodeConfig};
use crate::db::{
    DbPool, NewSession, Session, get_sessions_in_state, insert_session, update_session_state,
};
use crate::forgejo::ForgejoClient;
use crate::forgejo::models::{Issue, IssueComment, PullRequestReviewComment};
use crate::session::env_loader;
use crate::session::worktree;
use crate::session::{
    SESSION_BUSY_STATES, SessionAction, SessionState, SessionTrigger, build_prompt,
    derive_session_id, opencode_session_web_url,
};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashMap;
use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

// Template files embedded at compile time
const PACKAGE_JSON: &str = include_str!("../../opencode-config/package.json");
const AGENT_DEF: &str = include_str!("../../opencode-config/agents/forgebot.md");
const TOOL_COMMENT_ISSUE: &str = include_str!("../../opencode-config/tools/comment-issue.ts");
const TOOL_COMMENT_PR: &str = include_str!("../../opencode-config/tools/comment-pr.ts");
const TOOL_CREATE_PR: &str = include_str!("../../opencode-config/tools/create-pr.ts");

/// Sets up the opencode config directory with embedded template files.
///
/// This function is called once on startup. It creates the directory structure
/// and writes managed template files, overwriting any existing content.
///
/// # Arguments
/// * `config` - The opencode configuration containing the config_dir path
///
/// # Returns
/// * `Ok(())` on success
/// * `Err` on permission or I/O errors
pub fn setup_opencode_config_dir(config: &OpencodeConfig) -> Result<()> {
    let config_dir = &config.config_dir;

    info!(
        "Setting up opencode config directory at: {}",
        config_dir.display()
    );

    // Create the main config directory
    std::fs::create_dir_all(config_dir).with_context(|| {
        format!(
            "Failed to create config directory: {}",
            config_dir.display()
        )
    })?;

    // Create subdirectories
    let agents_dir = config_dir.join("agents");
    let tools_dir = config_dir.join("tools");

    std::fs::create_dir_all(&agents_dir).with_context(|| {
        format!(
            "Failed to create agents directory: {}",
            agents_dir.display()
        )
    })?;

    std::fs::create_dir_all(&tools_dir)
        .with_context(|| format!("Failed to create tools directory: {}", tools_dir.display()))?;

    // Define template files to write
    let files_to_write = [
        (
            config_dir.join("package.json"),
            PACKAGE_JSON,
            "package.json",
        ),
        (
            agents_dir.join("forgebot.md"),
            AGENT_DEF,
            "agents/forgebot.md",
        ),
        (
            tools_dir.join("comment-issue.ts"),
            TOOL_COMMENT_ISSUE,
            "tools/comment-issue.ts",
        ),
        (
            tools_dir.join("comment-pr.ts"),
            TOOL_COMMENT_PR,
            "tools/comment-pr.ts",
        ),
        (
            tools_dir.join("create-pr.ts"),
            TOOL_CREATE_PR,
            "tools/create-pr.ts",
        ),
    ];

    // Write each managed file, overwriting any existing content
    for (path, content, name) in &files_to_write {
        std::fs::write(path, content).with_context(|| {
            format!(
                "Failed to write opencode config file: {} at {}",
                name,
                path.display()
            )
        })?;
        info!("Wrote opencode config file: {}", name);
    }

    info!("opencode config directory setup complete");
    Ok(())
}

/// Parameters for running opencode
pub struct RunOpencodeParams<'a> {
    pub config: &'a Config,
    pub repo_full_name: &'a str,
    pub derived_session_id: &'a str,
    pub external_opencode_session_id: Option<&'a str>,
    pub agent_mode: &'a str,
    pub model: &'a str,
    pub worktree_path: &'a Path,
    pub prompt: &'a str,
    pub env_extras: HashMap<String, String>,
}

/// Run opencode subprocess with the given parameters.
///
/// If external_opencode_session_id is provided, continues that session.
/// Otherwise, creates a new session with the derived_session_id as title.
///
/// # Returns
/// * `Ok(Some(session_id))` - the opencode session ID (captured or provided)
/// * `Ok(None)` - if we couldn't capture the session ID
/// * `Err` - if opencode fails
pub async fn run_opencode(params: RunOpencodeParams<'_>) -> Result<Option<String>> {
    let binary = &params.config.opencode.binary;
    let opencode_config_home = params.config.opencode.config_dir.clone();
    let repo_full_name = params.repo_full_name;
    let derived_session_id = params.derived_session_id;
    let agent_mode = params.agent_mode;
    let model = params.model;
    let worktree_path = params.worktree_path;
    let prompt = params.prompt;
    let env_extras = params.env_extras;

    debug!(
        "Spawning opencode: binary={}, derived_session_id={}, agent_mode={}, model={}",
        binary, derived_session_id, agent_mode, model
    );

    // Build environment
    let mut env_vars: HashMap<String, String> = HashMap::new();

    // 1. Start with process environment
    for (key, value) in std::env::vars() {
        env_vars.insert(key, value);
    }

    let base_path = env_vars
        .get("PATH")
        .cloned()
        .unwrap_or_else(|| "".to_string());

    // 2. Add env loader output (direnv/nix results), but keep runtime-critical
    // paths from the service environment. Nix dev shells often set HOME to
    // /homeless-shelter, which breaks Bun/opencode under hardened systemd.
    let mut blocked_runtime_overrides = Vec::new();
    for (key, value) in env_extras {
        if matches!(
            key.as_str(),
            "HOME"
                | "XDG_DATA_HOME"
                | "XDG_CONFIG_HOME"
                | "XDG_CACHE_HOME"
                | "BUN_INSTALL_CACHE_DIR"
                | "TMPDIR"
                | "TMP"
                | "TEMP"
                | "NIX_ENFORCE_PURITY"
                | "OPENCODE_CONFIG_DIR"
        ) {
            blocked_runtime_overrides.push(format!("{}={}", key, value));
            continue;
        }
        env_vars.insert(key, value);
    }

    // Keep service PATH entries available (git/opencode), even if env loader
    // provides a replacement PATH.
    let mut merged_path_entries: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    let current_path = env_vars
        .get("PATH")
        .cloned()
        .unwrap_or_else(|| "".to_string());
    for entry in current_path.split(':').chain(base_path.split(':')) {
        if entry.is_empty() {
            continue;
        }
        let entry_str = entry.to_string();
        if seen.insert(entry_str.clone()) {
            merged_path_entries.push(entry_str);
        }
    }
    if !merged_path_entries.is_empty() {
        env_vars.insert("PATH".to_string(), merged_path_entries.join(":"));
    }
    if !blocked_runtime_overrides.is_empty() {
        warn!(
            overrides = %blocked_runtime_overrides.join(", "),
            "Ignored env loader overrides for protected runtime variables"
        );
    }

    // 3. Set FORGEBOT_* vars (always win)
    env_vars.insert(
        "FORGEBOT_FORGEJO_URL".to_string(),
        params.config.forgejo.url.clone(),
    );
    env_vars.insert(
        "FORGEBOT_FORGEJO_TOKEN".to_string(),
        params.config.forgejo.token.clone(),
    );
    env_vars.insert("FORGEBOT_REPO".to_string(), repo_full_name.to_string());
    // Note: XDG_DATA_HOME and XDG_CONFIG_HOME are set by the systemd service
    // and inherited from the process environment. These control where opencode
    // looks for auth.json ($XDG_DATA_HOME/opencode/auth.json) and global config.
    // OPENCODE_CONFIG_DIR is the real variable for custom config directory.
    env_vars.insert(
        "OPENCODE_CONFIG_DIR".to_string(),
        opencode_config_home.display().to_string(),
    );

    // Configure non-interactive git HTTPS auth using the Forgejo token.
    // The token is already present in FORGEBOT_FORGEJO_TOKEN. This askpass script
    // returns bot username for username prompts and token for password prompts.
    let askpass_path = std::env::temp_dir().join("forgebot-git-askpass.sh");
    if let Some(parent) = askpass_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create directory for git askpass script at {}",
                parent.display()
            )
        })?;
    }
    std::fs::write(
        &askpass_path,
        r#"#!/bin/sh
prompt="$1"
case "$prompt" in
  *Username*|*username*)
    printf '%s\n' "${FORGEBOT_FORGEJO_BOT_USERNAME:-forgebot}"
    ;;
  *)
    printf '%s\n' "${FORGEBOT_FORGEJO_TOKEN:-}"
    ;;
esac
"#,
    )
    .with_context(|| {
        format!(
            "failed to write git askpass script at {}",
            askpass_path.display()
        )
    })?;
    std::fs::set_permissions(&askpass_path, std::fs::Permissions::from_mode(0o700)).with_context(
        || {
            format!(
                "failed to set executable permissions on {}",
                askpass_path.display()
            )
        },
    )?;

    env_vars.insert("GIT_TERMINAL_PROMPT".to_string(), "0".to_string());
    env_vars.insert(
        "GIT_ASKPASS".to_string(),
        askpass_path.display().to_string(),
    );
    env_vars.insert(
        "SSH_ASKPASS".to_string(),
        askpass_path.display().to_string(),
    );

    if let Ok(home) = std::env::var("HOME") {
        env_vars.insert("HOME".to_string(), home);
    }
    if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
        env_vars.insert("XDG_DATA_HOME".to_string(), xdg_data_home);
    }
    if let Ok(xdg_config_home) = std::env::var("XDG_CONFIG_HOME") {
        env_vars.insert("XDG_CONFIG_HOME".to_string(), xdg_config_home);
    }
    if let Ok(xdg_cache_home) = std::env::var("XDG_CACHE_HOME") {
        env_vars.insert("XDG_CACHE_HOME".to_string(), xdg_cache_home);
    }
    if let Ok(bun_cache_dir) = std::env::var("BUN_INSTALL_CACHE_DIR") {
        env_vars.insert("BUN_INSTALL_CACHE_DIR".to_string(), bun_cache_dir);
    }
    env_vars.insert(
        "TMPDIR".to_string(),
        std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string()),
    );
    env_vars.insert(
        "TMP".to_string(),
        std::env::var("TMP").unwrap_or_else(|_| "/tmp".to_string()),
    );
    env_vars.insert(
        "TEMP".to_string(),
        std::env::var("TEMP").unwrap_or_else(|_| "/tmp".to_string()),
    );

    // Resolve binary path from PATH if not an absolute path
    let binary_path = if binary.contains('/') {
        binary.to_string()
    } else {
        let path_var = env_vars.get("PATH").cloned().unwrap_or_default();
        let mut found = None;
        for dir in path_var.split(':') {
            let candidate = std::path::Path::new(dir).join(binary);
            if candidate.exists() {
                found = Some(candidate.to_string_lossy().to_string());
                break;
            }
        }
        found.unwrap_or_else(|| binary.to_string())
    };

    // Ensure worktree directory exists
    if !worktree_path.exists() {
        info!("Creating worktree directory: {}", worktree_path.display());
        std::fs::create_dir_all(worktree_path).with_context(|| {
            format!(
                "Failed to create worktree directory: {}",
                worktree_path.display()
            )
        })?;
    }

    // Build the command
    let mut cmd = Command::new(&binary_path);
    cmd.arg("run")
        .arg("--agent")
        .arg(agent_mode)
        .arg("--model")
        .arg(model)
        .arg("--title")
        .arg(derived_session_id);

    // If we have an external session ID, continue that session
    // Otherwise, opencode will create a new one
    if let Some(external_id) = params.external_opencode_session_id {
        cmd.arg("--session").arg(external_id);
        info!("Continuing opencode session: {}", external_id);
    } else {
        info!(
            "Creating new opencode session with title: {}",
            derived_session_id
        );
    }

    cmd.arg("--dir")
        .arg(worktree_path)
        .arg(prompt)
        .current_dir(worktree_path)
        .envs(&env_vars)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null()); // Ensure we don't block waiting for input

    info!(
        "Running opencode command: binary={}, resolved_path={}, worktree={}",
        binary,
        binary_path,
        worktree_path.display()
    );

    let output = cmd.output().await.with_context(|| {
        format!(
            "Failed to spawn opencode process: {} (resolved to {})",
            binary, binary_path
        )
    })?;

    let status = output.status;
    let exit_code = status.code().unwrap_or(-1);
    let stderr_collected = String::from_utf8_lossy(&output.stderr).to_string();

    // Try to capture the opencode session ID
    let captured_session_id = if status.success() {
        match capture_opencode_session_id(binary, derived_session_id).await {
            Ok(Some(id)) => {
                info!("Captured opencode session ID: {}", id);
                Some(id)
            }
            Ok(None) => {
                warn!("Could not capture opencode session ID");
                None
            }
            Err(e) => {
                error!("Failed to capture opencode session ID: {}", e);
                None
            }
        }
    } else {
        None
    };

    if status.success() {
        debug!("opencode exited successfully with code 0");
        Ok(captured_session_id)
    } else {
        error!(
            "opencode failed with exit code {}: stdout={}, stderr={}",
            exit_code,
            String::from_utf8_lossy(&output.stdout),
            stderr_collected
        );
        Err(anyhow!(
            "opencode process failed with exit code {}: stdout={}, stderr={}",
            exit_code,
            String::from_utf8_lossy(&output.stdout),
            stderr_collected
        ))
    }
}

/// Capture the opencode session ID by querying the session list.
/// Looks for a session with the given title (which we set to our derived_session_id).
async fn capture_opencode_session_id(binary: &str, title: &str) -> Result<Option<String>> {
    // Query opencode session list
    let output = Command::new(binary)
        .arg("session")
        .arg("list")
        .arg("--format")
        .arg("json")
        .arg("-n")
        .arg("5") // Get 5 most recent sessions
        .output()
        .await
        .context("Failed to run opencode session list")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("opencode session list failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON output to find session with matching title
    // The output is an array of session objects
    // We need to find the one with title matching our derived_session_id
    match serde_json::from_str::<serde_json::Value>(&stdout) {
        Ok(sessions) => {
            if let Some(sessions_array) = sessions.as_array() {
                for session in sessions_array {
                    if let Some(session_title) = session.get("title").and_then(|t| t.as_str())
                        && session_title == title
                        && let Some(session_id) = session.get("id").and_then(|id| id.as_str())
                    {
                        return Ok(Some(session_id.to_string()));
                    }
                }
            }
            Ok(None)
        }
        Err(e) => {
            anyhow::bail!("Failed to parse opencode session list JSON: {}", e)
        }
    }
}

struct IssueContext {
    issue: Issue,
    issue_comments: Vec<IssueComment>,
    pr_review_comments: Vec<PullRequestReviewComment>,
}

async fn fetch_issue_context(
    forgejo: &ForgejoClient,
    trigger: &SessionTrigger,
) -> Result<IssueContext> {
    let issue = forgejo
        .get_issue(&trigger.repo_full_name, trigger.issue_id)
        .await
        .with_context(|| {
            format!(
                "failed to fetch issue {} for repo {}",
                trigger.issue_id, trigger.repo_full_name
            )
        })?;

    let issue_comments = match forgejo
        .list_issue_comments(&trigger.repo_full_name, trigger.issue_id)
        .await
    {
        Ok(comments) => comments,
        Err(e) => {
            warn!(
                "Failed to fetch issue comments for {}: {}",
                trigger.issue_id, e
            );
            Vec::new()
        }
    };

    let pr_review_comments = if trigger.action == SessionAction::Revision {
        if let Some(pr_id) = trigger.pr_id {
            match forgejo
                .list_pr_review_comments(&trigger.repo_full_name, pr_id)
                .await
            {
                Ok(comments) => comments,
                Err(e) => {
                    warn!("Failed to fetch PR review comments for PR {}: {}", pr_id, e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    Ok(IssueContext {
        issue,
        issue_comments,
        pr_review_comments,
    })
}

async fn get_or_create_session(
    db: &DbPool,
    trigger: &SessionTrigger,
    session_id: &str,
    worktree_path: &Path,
    existing_session: Option<Session>,
) -> Result<Session> {
    if let Some(session) = existing_session {
        return Ok(session);
    }

    let new_session = NewSession {
        id: uuid::Uuid::new_v4().to_string(),
        repo_full_name: trigger.repo_full_name.clone(),
        issue_id: trigger.issue_id as i64,
        pr_id: trigger.pr_id.map(|id| id as i64),
        opencode_session_id: session_id.to_string(),
        worktree_path: worktree_path.display().to_string(),
        state: SessionState::Idle.as_str().to_string(),
        mode: trigger.action.session_mode().as_str().to_string(),
    };
    insert_session(db, &new_session).await?;

    crate::db::get_session_by_issue(db, &trigger.repo_full_name, trigger.issue_id as i64)
        .await?
        .ok_or_else(|| anyhow!("Failed to retrieve newly created session"))
}

async fn post_acknowledgement(forgejo: &ForgejoClient, trigger: &SessionTrigger, session_id: &str) {
    let ack_msg = match trigger.action {
        SessionAction::Plan => format!(
            "🤖 forgebot is joining the discussion on this issue.\n\nSession: `{}`",
            session_id
        ),
        SessionAction::Build => format!(
            "🤖 forgebot is implementing this issue and preparing a PR.\n\nSession: `{}`",
            session_id
        ),
        SessionAction::Revision => format!(
            "🤖 forgebot is addressing review comments. Revising...\n\nSession: `{}`",
            session_id
        ),
    };

    if let Err(e) = forgejo
        .post_issue_comment(&trigger.repo_full_name, trigger.issue_id, &ack_msg)
        .await
    {
        warn!(
            repo = %trigger.repo_full_name,
            issue_id = %trigger.issue_id,
            err = %e,
            "Failed to post acknowledgement comment"
        );
    }
}

async fn load_existing_session(db: &DbPool, trigger: &SessionTrigger) -> Result<Option<Session>> {
    crate::db::get_session_by_issue(db, &trigger.repo_full_name, trigger.issue_id as i64).await
}

async fn reject_if_session_busy(
    forgejo: &ForgejoClient,
    trigger: &SessionTrigger,
    session_id: &str,
    existing_session: &Option<Session>,
) -> Result<()> {
    if let Some(session) = existing_session
        && session.state.is_busy()
    {
        info!(
            "Session {} is busy (state: {}), posting rejection comment",
            session_id, session.state
        );
        if let Err(e) = forgejo
            .post_issue_comment(
                &trigger.repo_full_name,
                trigger.issue_id,
                &format!(
                    "⚠️ forgebot is currently busy (state: {}). Please wait for the current operation to complete before triggering a new one.",
                    session.state
                ),
            )
            .await
        {
            warn!(
                repo = %trigger.repo_full_name,
                issue_id = %trigger.issue_id,
                session_id = %session.id,
                err = %e,
                "Failed to post busy-state rejection comment"
            );
        }
        bail!("session {} is busy in state {}", session.id, session.state);
    }

    Ok(())
}

async fn lookup_repo_record(db: &DbPool, trigger: &SessionTrigger) -> Result<crate::db::Repo> {
    crate::db::get_repo_by_full_name(db, &trigger.repo_full_name)
        .await?
        .ok_or_else(|| {
            anyhow!(
                "Repository {} not found in database",
                trigger.repo_full_name
            )
        })
}

async fn ensure_session_worktree(
    config: &Config,
    trigger: &SessionTrigger,
    default_branch: &str,
) -> Result<PathBuf> {
    let worktree_path =
        worktree::worktree_path(&config.opencode, &trigger.repo_full_name, trigger.issue_id);

    if !worktree_path.exists() {
        info!(
            repo = %trigger.repo_full_name,
            issue_id = %trigger.issue_id,
            path = %worktree_path.display(),
            "Creating worktree"
        );
        worktree::create_worktree(
            &config.opencode,
            &trigger.repo_full_name,
            trigger.issue_id,
            default_branch,
        )
        .await
        .with_context(|| {
            format!(
                "failed to create worktree for {} issue {}",
                trigger.repo_full_name, trigger.issue_id
            )
        })?;
    }

    Ok(worktree_path)
}

async fn load_env_or_fail(
    db: &DbPool,
    forgejo: &ForgejoClient,
    trigger: &SessionTrigger,
    session_id: &str,
    env_loader_name: &str,
    worktree_path: &Path,
    existing_session: &Option<Session>,
) -> Result<HashMap<String, String>> {
    match env_loader::load_env(env_loader_name, worktree_path).await {
        Ok(env) => Ok(env),
        Err(e) => {
            let error_str = e.to_string();
            error!(
                session_id = %session_id,
                repo = %trigger.repo_full_name,
                issue_id = %trigger.issue_id,
                env_loader = %env_loader_name,
                worktree_path = %worktree_path.display(),
                err = %error_str,
                "Environment loading failed"
            );
            if let Err(post_err) = forgejo
                .post_issue_comment(
                    &trigger.repo_full_name,
                    trigger.issue_id,
                    &format!(
                        "❌ forgebot: env loader '{}' failed and the session cannot continue. \
Fix the loader configuration and re-trigger when ready. \
Error output: {}",
                        env_loader_name, error_str
                    ),
                )
                .await
            {
                warn!(
                    repo = %trigger.repo_full_name,
                    issue_id = %trigger.issue_id,
                    session_id = %session_id,
                    err = %post_err,
                    "Failed to post env-loader failure comment"
                );
            }

            if let Some(session) = existing_session
                && let Err(update_err) =
                    update_session_state(db, &session.id, SessionState::Error).await
            {
                error!(
                    session_id = %session.id,
                    err = %update_err,
                    "Failed to set existing session to error state after env-loader failure"
                );
            }

            bail!(
                "environment loading failed for {}: {}",
                trigger.repo_full_name,
                error_str
            );
        }
    }
}

fn build_session_env(
    trigger: &SessionTrigger,
    env_extras: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut session_env = env_extras.clone();
    session_env.insert(
        "FORGEBOT_ISSUE_ID".to_string(),
        trigger.issue_id.to_string(),
    );
    if let Some(pr_id) = trigger.pr_id {
        session_env.insert("FORGEBOT_PR_ID".to_string(), pr_id.to_string());
    }
    session_env
}

fn external_session_id(session: &Session) -> Option<&str> {
    if session.opencode_session_id.starts_with("ses_") {
        None
    } else {
        Some(session.opencode_session_id.as_str())
    }
}

async fn handle_dispatch_success(
    db: &DbPool,
    forgejo: &ForgejoClient,
    config: &Config,
    trigger: &SessionTrigger,
    session_id: &str,
    session_record: &Session,
    captured_session_id: Option<String>,
) -> Result<()> {
    info!(
        session_id = %session_id,
        exit_code = 0,
        captured_session_id = ?captured_session_id,
        "Session completed successfully"
    );

    let should_post_web_link = session_record.opencode_session_id.starts_with("ses_");
    let mut effective_session_id = if should_post_web_link {
        None
    } else {
        Some(session_record.opencode_session_id.clone())
    };

    if let Some(new_session_id) = captured_session_id {
        if let Err(e) =
            crate::db::update_session_opencode_id(db, &session_record.id, &new_session_id).await
        {
            error!("Failed to update session with opencode ID: {}", e);
        }
        effective_session_id = Some(new_session_id);
    }

    let web_host = config.opencode.web_host.as_deref();
    match (
        should_post_web_link,
        web_host,
        effective_session_id.as_deref(),
    ) {
        (false, _, _) => {
            info!(
                repo = %trigger.repo_full_name,
                issue_id = %trigger.issue_id,
                session_id = %session_record.id,
                opencode_session_id = %session_record.opencode_session_id,
                "Skipping session Web UI link: existing external opencode session ID"
            );
        }
        (true, None, _) => {
            info!(
                repo = %trigger.repo_full_name,
                issue_id = %trigger.issue_id,
                session_id = %session_record.id,
                "Skipping session Web UI link: FORGEBOT_OPENCODE_WEB_HOST not configured"
            );
        }
        (true, Some(_), None) => {
            warn!(
                repo = %trigger.repo_full_name,
                issue_id = %trigger.issue_id,
                session_id = %session_record.id,
                "Skipping session Web UI link: missing effective opencode session ID"
            );
        }
        (true, Some(web_host), Some(opencode_id)) => {
            let web_url =
                opencode_session_web_url(web_host, &session_record.worktree_path, opencode_id);
            let web_ui_msg = format!("🔗 Opencode session Web UI: [{}]({})", web_url, web_url);
            if let Err(e) = forgejo
                .post_issue_comment(&trigger.repo_full_name, trigger.issue_id, &web_ui_msg)
                .await
            {
                warn!(
                    repo = %trigger.repo_full_name,
                    issue_id = %trigger.issue_id,
                    session_id = %session_record.id,
                    err = %e,
                    "Failed to post session Web UI link"
                );
            } else {
                info!(
                    repo = %trigger.repo_full_name,
                    issue_id = %trigger.issue_id,
                    session_id = %session_record.id,
                    web_url = %web_url,
                    "Posted session Web UI link"
                );
            }
        }
    }

    update_session_state(db, &session_record.id, SessionState::Idle).await?;

    let success_msg = match trigger.action {
        SessionAction::Plan => {
            "✅ Collaboration update posted. Add the build flag in a comment when you're ready for implementation and PR creation."
        }
        SessionAction::Build => "✅ Implementation complete! A pull request has been created.",
        SessionAction::Revision => "✅ Review comments addressed and changes pushed.",
    };
    if let Err(e) = forgejo
        .post_issue_comment(&trigger.repo_full_name, trigger.issue_id, success_msg)
        .await
    {
        warn!(
            repo = %trigger.repo_full_name,
            issue_id = %trigger.issue_id,
            session_id = %session_record.id,
            err = %e,
            "Failed to post success comment"
        );
    }

    Ok(())
}

async fn handle_dispatch_failure(
    db: &DbPool,
    forgejo: &ForgejoClient,
    trigger: &SessionTrigger,
    session_id: &str,
    session_record: &Session,
    error: anyhow::Error,
) -> Result<()> {
    let error_str = error.to_string();
    error!(
        session_id = %session_id,
        error = %error_str,
        "Session failed"
    );
    update_session_state(db, &session_record.id, SessionState::Error).await?;

    let error_msg = format!(
        "❌ Task failed. Error: {}\n\nSession set to error state. Please re-trigger when ready.",
        error_str
    );
    if let Err(post_err) = forgejo
        .post_issue_comment(&trigger.repo_full_name, trigger.issue_id, &error_msg)
        .await
    {
        warn!(
            repo = %trigger.repo_full_name,
            issue_id = %trigger.issue_id,
            session_id = %session_record.id,
            err = %post_err,
            "Failed to post failure comment"
        );
    }

    Err(error)
}

/// Main session orchestration function.
///
/// This is called from webhook handlers to dispatch a new session.
/// It handles the full lifecycle: loading env, building prompt,
/// spawning opencode, and updating state.
///
/// This function runs in a spawned task, so it can block without
/// holding up the webhook response.
///
/// # Arguments
/// * `db` - Database connection pool
/// * `forgejo` - Forgejo API client
/// * `config` - Forgebot configuration
/// * `trigger` - The session trigger event
pub async fn dispatch_session(
    db: &DbPool,
    forgejo: &ForgejoClient,
    config: &Config,
    trigger: SessionTrigger,
) -> Result<()> {
    let session_id = derive_session_id(&trigger.repo_full_name, trigger.issue_id);
    let new_state = trigger.action.state();

    info!(
        session_id = %session_id,
        agent_mode = %trigger.action.as_str(),
        repo = %trigger.repo_full_name,
        issue_id = %trigger.issue_id,
        "Dispatching session"
    );

    // 1. Fetch issue context from Forgejo
    let issue_context = fetch_issue_context(forgejo, &trigger).await?;

    // 2. Check if session already exists and reject if busy
    let existing_session = load_existing_session(db, &trigger).await?;
    reject_if_session_busy(forgejo, &trigger, &session_id, &existing_session).await?;

    // 3. Build prompt
    let prompt = build_prompt(
        trigger.action,
        &issue_context.issue,
        &issue_context.issue_comments,
        &issue_context.pr_review_comments,
        trigger.pr_id,
    );

    // 4. Look up repository metadata and ensure worktree exists
    let repo_record = lookup_repo_record(db, &trigger).await?;
    let worktree_path =
        ensure_session_worktree(config, &trigger, &repo_record.default_branch).await?;

    // 5. Load environment in the worktree using the repository's configured loader.
    let env_extras = load_env_or_fail(
        db,
        forgejo,
        &trigger,
        &session_id,
        &repo_record.env_loader,
        &worktree_path,
        &existing_session,
    )
    .await?;

    // 6. Get or create session record
    let session_record =
        get_or_create_session(db, &trigger, &session_id, &worktree_path, existing_session).await?;

    // 7. Post acknowledgement comment
    post_acknowledgement(forgejo, &trigger, &session_id).await;

    // 8. Update session state
    update_session_state(db, &session_record.id, new_state).await?;

    // 9. Determine agent mode
    let agent_mode = trigger.action.agent_mode();

    // 10. Set FORGEBOT_* env vars for this session
    let session_env = build_session_env(&trigger, &env_extras);

    // 11. Spawn opencode
    let external_session_id = external_session_id(&session_record);

    info!(
        session_id = %session_id,
        agent_mode = %agent_mode,
        worktree_path = %worktree_path.display(),
        has_external_session = external_session_id.is_some(),
        "Spawning opencode"
    );

    let opencode_result = run_opencode(RunOpencodeParams {
        config,
        repo_full_name: &trigger.repo_full_name,
        derived_session_id: &session_id,
        external_opencode_session_id: external_session_id,
        agent_mode,
        model: &config.opencode.model,
        worktree_path: &worktree_path,
        prompt: &prompt,
        env_extras: session_env,
    })
    .await;

    // 12. Handle result
    match opencode_result {
        Ok(captured_session_id) => {
            handle_dispatch_success(
                db,
                forgejo,
                config,
                &trigger,
                &session_id,
                &session_record,
                captured_session_id,
            )
            .await
        }
        Err(e) => {
            handle_dispatch_failure(db, forgejo, &trigger, &session_id, &session_record, e).await
        }
    }
}

/// Crash recovery: handle sessions that were in progress when forgebot restarted.
///
/// This function is called on startup before the server starts.
/// It finds all sessions in "planning", "building", or "revising" state and:
/// 1. Sets them to "error" state
/// 2. Posts a comment on the issue explaining what happened
///
/// # Arguments
/// * `db` - Database connection pool
/// * `forgejo` - Forgejo API client
/// * `config` - Forgebot configuration
///
/// # Returns
/// Always returns Ok(()) - failures are logged but don't block startup
pub async fn startup_crash_recovery(
    db: &DbPool,
    forgejo: &ForgejoClient,
    _config: &Config,
) -> Result<usize> {
    info!("Running startup crash recovery...");

    let stuck_sessions = match get_sessions_in_state(db, SESSION_BUSY_STATES).await {
        Ok(sessions) => sessions,
        Err(e) => {
            error!("Failed to query stuck sessions: {}", e);
            return Ok(0); // Non-blocking
        }
    };

    if stuck_sessions.is_empty() {
        info!("No stuck sessions found, crash recovery complete");
        return Ok(0);
    }

    let session_count = stuck_sessions.len();
    info!(
        session_count = %session_count,
        "Found sessions stuck in progress, recovering"
    );

    for session in stuck_sessions {
        warn!(
            session_id = %session.id,
            state = %session.state,
            repo = %session.repo_full_name,
            issue_id = %session.issue_id,
            "Recovering stuck session"
        );

        // Set state to error
        if let Err(e) = update_session_state(db, &session.id, SessionState::Error).await {
            error!(
                session_id = %session.id,
                error = %e,
                "Failed to set session to error state"
            );
            continue;
        }

        // Post recovery comment
        let recovery_msg =
            "⚠️ forgebot restarted mid-run. The session has been reset. Please retry your command.";

        if let Err(e) = forgejo
            .post_issue_comment(
                &session.repo_full_name,
                session.issue_id as u64,
                recovery_msg,
            )
            .await
        {
            error!(
                "Failed to post recovery comment for session {}: {}",
                session.id, e
            );
        } else {
            info!(
                session_id = %session.id,
                "Posted recovery comment"
            );
        }
    }

    info!(
        recovered_count = %session_count,
        "Crash recovery complete"
    );
    Ok(session_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: run_opencode tests would require mocking or actual opencode binary
    // Note: dispatch_session tests would require complex mocking of db and forgejo
    // Note: startup_crash_recovery tests would require a test database

    #[test]
    fn test_setup_opencode_config_dir_creates_files() {
        // Create a temporary directory for testing
        let temp_dir =
            std::env::temp_dir().join(format!("test-opencode-config-{}", std::process::id()));

        // Clean up any existing test directory
        let _ = std::fs::remove_dir_all(&temp_dir);

        let config = OpencodeConfig {
            binary: "opencode".to_string(),
            worktree_base: temp_dir.join("worktrees"),
            config_dir: temp_dir.clone(),
            git_binary: "git".to_string(),
            model: "opencode/kimi-k2.5".to_string(),
            web_host: None,
        };

        // First call should create all files
        setup_opencode_config_dir(&config).expect("Setup should succeed");

        // Verify all files exist
        assert!(temp_dir.join("package.json").exists());
        assert!(temp_dir.join("agents").join("forgebot.md").exists());
        assert!(temp_dir.join("tools").join("comment-issue.ts").exists());
        assert!(temp_dir.join("tools").join("comment-pr.ts").exists());
        assert!(temp_dir.join("tools").join("create-pr.ts").exists());

        // Verify content was written correctly
        let package_json_content = std::fs::read_to_string(temp_dir.join("package.json")).unwrap();
        assert!(package_json_content.contains("@opencode-ai/plugin"));

        let agent_content =
            std::fs::read_to_string(temp_dir.join("agents").join("forgebot.md")).unwrap();
        assert!(agent_content.contains("forgebot"));

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_setup_opencode_config_dir_overwrites_existing_files() {
        let temp_dir = std::env::temp_dir().join(format!(
            "test-opencode-config-existing-{}",
            std::process::id()
        ));

        // Clean up any existing test directory
        let _ = std::fs::remove_dir_all(&temp_dir);

        // Create the directory and a custom file first
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("package.json"), "custom content").unwrap();

        let config = OpencodeConfig {
            binary: "opencode".to_string(),
            worktree_base: temp_dir.join("worktrees"),
            config_dir: temp_dir.clone(),
            git_binary: "git".to_string(),
            model: "opencode/kimi-k2.5".to_string(),
            web_host: None,
        };

        // Setup should succeed and overwrite managed files
        setup_opencode_config_dir(&config).expect("Setup should succeed");

        // Verify managed content was restored
        let content = std::fs::read_to_string(temp_dir.join("package.json")).unwrap();
        assert!(content.contains("@opencode-ai/plugin"));

        // But other files should still be created
        assert!(temp_dir.join("agents").join("forgebot.md").exists());

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
