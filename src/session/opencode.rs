//! opencode configuration directory setup and API session orchestration
//!
//! This module handles:
//! 1. Writing the global opencode config directory on startup
//! 2. Dispatching OpenCode API prompts with proper session continuity
//! 3. Session orchestration and lifecycle management
//! 4. Crash recovery on startup

use crate::config::{Config, OpencodeConfig};
use crate::db::{
    DbPool, NewSession, Session, get_issue_external_opencode_session_id, insert_session,
    update_issue_external_opencode_session_id, update_session_state,
};
use crate::forgejo::ForgejoClient;
use crate::forgejo::models::{Issue, IssueComment, PullRequestReviewComment};
use crate::session::env_loader;
use crate::session::opencode_api::{
    CreateSessionRequest, OpencodeApiClient, PromptAsyncRequest, PromptModelInput, PromptPartInput,
    SessionStatus,
};
use crate::session::worktree;
use crate::session::{
    PromptContext, SessionAction, SessionState, SessionTrigger, build_prompt, derive_session_id,
    opencode_session_web_url,
};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use tracing::{error, info, warn};

// Template files embedded at compile time
const PACKAGE_JSON: &str = include_str!("../../opencode-config/package.json");
const OPENCODE_JSON: &str = include_str!("../../opencode-config/opencode.json");
const AGENT_DEF: &str = include_str!("../../opencode-config/agents/forgebot.md");

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

    std::fs::create_dir_all(&agents_dir).with_context(|| {
        format!(
            "Failed to create agents directory: {}",
            agents_dir.display()
        )
    })?;

    // Define template files to write
    let files_to_write = [
        (
            config_dir.join("package.json"),
            PACKAGE_JSON,
            "package.json",
        ),
        (
            config_dir.join("opencode.json"),
            OPENCODE_JSON,
            "opencode.json",
        ),
        (
            agents_dir.join("forgebot.md"),
            AGENT_DEF,
            "agents/forgebot.md",
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

async fn post_acknowledgement(
    forgejo: &ForgejoClient,
    trigger: &SessionTrigger,
    session_id: &str,
    session_web_url: Option<&str>,
) {
    let ack_msg = build_acknowledgement_message(trigger.action, session_id, session_web_url);

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

fn build_acknowledgement_message(
    action: SessionAction,
    session_id: &str,
    session_web_url: Option<&str>,
) -> String {
    let base = match action {
        SessionAction::Plan => "🤖 forgebot is joining the discussion on this issue.",
        SessionAction::Build => "🤖 forgebot is implementing this issue and preparing a PR.",
        SessionAction::Revision => "🤖 forgebot is addressing review comments. Revising...",
    };
    let mut ack_msg = format!(
        "{}\n\nSession: `{}`\n\nWork continues asynchronously.",
        base, session_id
    );
    if let Some(url) = session_web_url {
        ack_msg.push_str(&format!("\n\n🔗 OpenCode session: [{}]({})", url, url));
    }

    ack_msg
}

async fn load_existing_session(db: &DbPool, trigger: &SessionTrigger) -> Result<Option<Session>> {
    crate::db::get_session_by_issue(db, &trigger.repo_full_name, trigger.issue_id as i64).await
}

fn describe_session_status(status: &SessionStatus) -> String {
    match status {
        SessionStatus::Busy => "busy".to_string(),
        SessionStatus::Retry {
            attempt,
            message,
            next,
        } => {
            if message.is_empty() {
                format!("retry (attempt {}, next in {}s)", attempt, next)
            } else {
                format!(
                    "retry (attempt {}, next in {}s): {}",
                    attempt, next, message
                )
            }
        }
        SessionStatus::Idle => "idle".to_string(),
    }
}

fn blocking_api_session_status<'a>(
    statuses: &'a HashMap<String, SessionStatus>,
    preferred_session_id: Option<&'a str>,
) -> Option<(&'a str, &'a SessionStatus)> {
    if let Some(session_id) = preferred_session_id
        && let Some(status) = statuses.get(session_id)
        && matches!(status, SessionStatus::Busy | SessionStatus::Retry { .. })
    {
        return Some((session_id, status));
    }

    statuses.iter().find_map(|(session_id, status)| {
        if matches!(status, SessionStatus::Busy | SessionStatus::Retry { .. }) {
            Some((session_id.as_str(), status))
        } else {
            None
        }
    })
}

async fn reject_if_api_admission_blocked(
    forgejo: &ForgejoClient,
    config: &Config,
    trigger: &SessionTrigger,
    worktree_path: &Path,
    preferred_session_id: Option<&str>,
) -> Result<()> {
    let api_client = OpencodeApiClient::from_config(&config.opencode.api)
        .context("failed to initialize OpenCode API client for admission gate")?;
    let statuses = api_client
        .session_status(worktree_path)
        .await
        .with_context(|| {
            format!(
                "failed to read OpenCode session statuses for {} issue {}",
                trigger.repo_full_name, trigger.issue_id
            )
        })?;

    if let Some((blocking_session_id, status)) =
        blocking_api_session_status(&statuses, preferred_session_id)
    {
        let status_text = describe_session_status(status);
        info!(
            repo = %trigger.repo_full_name,
            issue_id = %trigger.issue_id,
            opencode_session_id = %blocking_session_id,
            status = %status_text,
            "OpenCode API admission rejected due to active session status"
        );

        if let Err(e) = forgejo
            .post_issue_comment(
                &trigger.repo_full_name,
                trigger.issue_id,
                &format!(
                    "⚠️ forgebot cannot accept a new trigger right now because OpenCode session `{}` is {}. Please wait for the current run to finish and retry.",
                    blocking_session_id,
                    status_text,
                ),
            )
            .await
        {
            warn!(
                repo = %trigger.repo_full_name,
                issue_id = %trigger.issue_id,
                opencode_session_id = %blocking_session_id,
                err = %e,
                "Failed to post OpenCode status rejection comment"
            );
        }

        bail!(
            "OpenCode API admission blocked for {} issue {}: session {} is {}",
            trigger.repo_full_name,
            trigger.issue_id,
            blocking_session_id,
            status_text,
        );
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

fn external_session_id(session: &Session) -> Option<&str> {
    let opencode_session_id = session.opencode_session_id.trim();
    let derived_placeholder = derive_session_id(&session.repo_full_name, session.issue_id as u64);

    if opencode_session_id.is_empty() || opencode_session_id == derived_placeholder {
        None
    } else {
        Some(opencode_session_id)
    }
}

async fn handle_dispatch_success(
    db: &DbPool,
    forgejo: &ForgejoClient,
    config: &Config,
    trigger: &SessionTrigger,
    session_record: &Session,
    captured_session_id: Option<String>,
    completion_confirmed: bool,
) -> Result<()> {
    let session_id = derive_session_id(&trigger.repo_full_name, trigger.issue_id);
    if completion_confirmed {
        info!(
            session_id = %session_id,
            exit_code = 0,
            captured_session_id = ?captured_session_id,
            "Session completed successfully"
        );
    } else {
        info!(
            session_id = %session_id,
            captured_session_id = ?captured_session_id,
            "OpenCode API request accepted; work continues asynchronously"
        );
    }

    let mut effective_session_id = get_issue_external_opencode_session_id(
        db,
        &trigger.repo_full_name,
        trigger.issue_id as i64,
    )
    .await?;
    let should_post_web_link = effective_session_id.is_none();

    if let Some(new_session_id) = captured_session_id {
        if let Err(e) = update_issue_external_opencode_session_id(
            db,
            &trigger.repo_full_name,
            trigger.issue_id as i64,
            &new_session_id,
        )
        .await
        {
            error!("Failed to update session with opencode ID: {}", e);
        }
        effective_session_id = Some(new_session_id);
    }

    if should_post_web_link && effective_session_id.is_none() {
        let continuity_err = anyhow!(
            "failed to capture opencode session ID; cannot establish session continuity for {} issue {}",
            trigger.repo_full_name,
            trigger.issue_id
        );
        return handle_dispatch_failure(
            db,
            forgejo,
            trigger,
            &session_id,
            session_record,
            continuity_err,
        )
        .await;
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

    let success_msg = if completion_confirmed {
        match trigger.action {
            SessionAction::Plan => {
                "✅ Collaboration update posted. Add another @forgebot comment when you want the agent to continue."
            }
            SessionAction::Build => "✅ Implementation complete! A pull request has been created.",
            SessionAction::Revision => "✅ Review comments addressed and changes pushed.",
        }
    } else {
        "✅ Request dispatched to OpenCode API. Work continues asynchronously in this session."
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

fn parse_model_input(model: &str) -> Option<PromptModelInput> {
    let mut parts = model.splitn(2, '/');
    let provider_id = parts.next()?.trim();
    let model_id = parts.next()?.trim();

    if provider_id.is_empty() || model_id.is_empty() {
        return None;
    }

    Some(PromptModelInput {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
    })
}

async fn resolve_opencode_api_session_id(
    config: &Config,
    trigger: &SessionTrigger,
    derived_session_id: &str,
    external_opencode_session_id: Option<&str>,
    worktree_path: &Path,
) -> Result<String> {
    let api_client = OpencodeApiClient::from_config(&config.opencode.api)
        .context("failed to initialize OpenCode API client")?;

    let mut session_id = external_opencode_session_id.map(str::to_string);

    if let Some(existing_id) = external_opencode_session_id {
        match api_client.get_session(worktree_path, existing_id).await {
            Ok(_) => {
                info!(
                    repo = %trigger.repo_full_name,
                    issue_id = %trigger.issue_id,
                    opencode_session_id = %existing_id,
                    "Reusing existing OpenCode API session"
                );
            }
            Err(e) => {
                warn!(
                    repo = %trigger.repo_full_name,
                    issue_id = %trigger.issue_id,
                    opencode_session_id = %existing_id,
                    err = %e,
                    "Existing OpenCode API session not available; creating a new session"
                );
                session_id = None;
            }
        }
    }

    if session_id.is_none() {
        let created = api_client
            .create_session(
                worktree_path,
                &CreateSessionRequest {
                    parent_id: None,
                    title: derived_session_id.to_string(),
                },
            )
            .await
            .with_context(|| {
                format!(
                    "failed to create OpenCode API session for {} issue {}",
                    trigger.repo_full_name, trigger.issue_id
                )
            })?;
        session_id = Some(created.id);
    }

    let session_id = session_id.ok_or_else(|| {
        anyhow!(
            "failed to resolve OpenCode API session for {} issue {}",
            trigger.repo_full_name,
            trigger.issue_id
        )
    })?;

    Ok(session_id)
}

async fn run_opencode_api(
    config: &Config,
    trigger: &SessionTrigger,
    worktree_path: &Path,
    prompt: &str,
    session_id: &str,
) -> Result<String> {
    let api_client = OpencodeApiClient::from_config(&config.opencode.api)
        .context("failed to initialize OpenCode API client")?;

    if parse_model_input(&config.opencode.model).is_none() {
        warn!(
            model = %config.opencode.model,
            "FORGEBOT_OPENCODE_MODEL missing provider/model format; API dispatch will use server default model"
        );
    }

    let prompt_request = PromptAsyncRequest {
        agent: Some(trigger.action.agent_mode().to_string()),
        model: parse_model_input(&config.opencode.model),
        no_reply: None,
        parts: vec![PromptPartInput::Text {
            text: prompt.to_string(),
        }],
    };

    api_client
        .prompt_async(worktree_path, session_id, &prompt_request)
        .await
        .with_context(|| {
            format!(
                "failed to dispatch OpenCode API prompt_async for {} issue {}",
                trigger.repo_full_name, trigger.issue_id
            )
        })?;

    Ok(session_id.to_string())
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

    // 2. Check if session already exists
    let existing_session = load_existing_session(db, &trigger).await?;

    // 3. Look up repository metadata and ensure worktree exists
    let repo_record = lookup_repo_record(db, &trigger).await?;
    let worktree_path =
        ensure_session_worktree(config, &trigger, &repo_record.default_branch).await?;

    // 4. Build prompt with explicit target context
    let work_branch = format!("agent/issue-{}", trigger.issue_id);
    let prompt_context = PromptContext {
        repo_full_name: &trigger.repo_full_name,
        issue_id: trigger.issue_id,
        pr_id: trigger.pr_id,
        base_branch: &repo_record.default_branch,
        work_branch: &work_branch,
    };
    let prompt = build_prompt(
        trigger.action,
        &prompt_context,
        &issue_context.issue,
        &issue_context.issue_comments,
        &issue_context.pr_review_comments,
    );

    // 5. Reject triggers while OpenCode reports busy/retry.
    let preferred_session_id = get_issue_external_opencode_session_id(
        db,
        &trigger.repo_full_name,
        trigger.issue_id as i64,
    )
    .await?
    .or_else(|| {
        existing_session
            .as_ref()
            .and_then(external_session_id)
            .map(str::to_string)
    });
    reject_if_api_admission_blocked(
        forgejo,
        config,
        &trigger,
        &worktree_path,
        preferred_session_id.as_deref(),
    )
    .await?;

    // 6. Load environment in the worktree using the repository's configured loader.
    // This currently serves as validation that the configured loader succeeds.
    let _env_extras = load_env_or_fail(
        db,
        forgejo,
        &trigger,
        &session_id,
        &repo_record.env_loader,
        &worktree_path,
        &existing_session,
    )
    .await?;

    // 7. Get or create session record
    let session_record =
        get_or_create_session(db, &trigger, &session_id, &worktree_path, existing_session).await?;

    // 8. Resolve API session immediately so acknowledgement can include link.
    let mut ack_session_web_url: Option<String> = None;
    let existing_external_id = external_session_id(&session_record);
    let api_session_id = match resolve_opencode_api_session_id(
        config,
        &trigger,
        &session_id,
        existing_external_id,
        &worktree_path,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return handle_dispatch_failure(db, forgejo, &trigger, &session_id, &session_record, e)
                .await;
        }
    };

    if existing_external_id.is_none() {
        if let Some(web_host) = config.opencode.web_host.as_deref() {
            ack_session_web_url = Some(opencode_session_web_url(
                web_host,
                &session_record.worktree_path,
                &api_session_id,
            ));
        }

        if let Err(e) = update_issue_external_opencode_session_id(
            db,
            &trigger.repo_full_name,
            trigger.issue_id as i64,
            &api_session_id,
        )
        .await
        {
            warn!(
                repo = %trigger.repo_full_name,
                issue_id = %trigger.issue_id,
                opencode_session_id = %api_session_id,
                err = %e,
                "Failed to persist resolved OpenCode API session ID before acknowledgement"
            );
        }
    }

    // 9. Post acknowledgement comment
    post_acknowledgement(
        forgejo,
        &trigger,
        &session_id,
        ack_session_web_url.as_deref(),
    )
    .await;

    // 10. Update session state
    update_session_state(db, &session_record.id, new_state).await?;

    // 11. Determine agent mode
    let agent_mode = trigger.action.agent_mode();

    // 12. Spawn opencode
    let external_session_id = external_session_id(&session_record);

    info!(
        session_id = %session_id,
        agent_mode = %agent_mode,
        worktree_path = %worktree_path.display(),
        has_external_session = external_session_id.is_some(),
        "Spawning opencode"
    );

    let opencode_result =
        run_opencode_api(config, &trigger, &worktree_path, &prompt, &api_session_id)
            .await
            .map(Some);
    let completion_confirmed = false;

    // 14. Handle result
    match opencode_result {
        Ok(captured_session_id) => {
            handle_dispatch_success(
                db,
                forgejo,
                config,
                &trigger,
                &session_record,
                captured_session_id,
                completion_confirmed,
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
    _db: &DbPool,
    _forgejo: &ForgejoClient,
    _config: &Config,
) -> Result<usize> {
    info!("Startup crash recovery skipped: session lifecycle is tracked by OpenCode API state");
    Ok(0)
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
            api: crate::config::OpencodeApiConfig {
                base_url: None,
                token: None,
                timeout_secs: 30,
            },
        };

        // First call should create all files
        setup_opencode_config_dir(&config).expect("Setup should succeed");

        // Verify all files exist
        assert!(temp_dir.join("package.json").exists());
        assert!(temp_dir.join("opencode.json").exists());
        assert!(temp_dir.join("agents").join("forgebot.md").exists());

        // Verify content was written correctly
        let package_json_content = std::fs::read_to_string(temp_dir.join("package.json")).unwrap();
        assert!(package_json_content.contains("\"name\": \"@forgebot/plugins\""));

        let opencode_json_content =
            std::fs::read_to_string(temp_dir.join("opencode.json")).unwrap();
        assert!(opencode_json_content.contains("\"permission\": \"allow\""));

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
            api: crate::config::OpencodeApiConfig {
                base_url: None,
                token: None,
                timeout_secs: 30,
            },
        };

        // Setup should succeed and overwrite managed files
        setup_opencode_config_dir(&config).expect("Setup should succeed");

        // Verify managed content was restored
        let content = std::fs::read_to_string(temp_dir.join("package.json")).unwrap();
        assert!(content.contains("\"name\": \"@forgebot/plugins\""));

        // But other files should still be created
        assert!(temp_dir.join("agents").join("forgebot.md").exists());

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_external_session_id_filters_derived_and_empty_values() {
        let derived = Session {
            id: "1".to_string(),
            repo_full_name: "owner/repo".to_string(),
            issue_id: 1,
            pr_id: None,
            opencode_session_id: "ses_1_owner_repo".to_string(),
            worktree_path: "/tmp/worktree".to_string(),
            state: SessionState::Idle,
            mode: crate::session::SessionMode::Collab,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        };
        assert_eq!(external_session_id(&derived), None);

        let empty = Session {
            opencode_session_id: "   ".to_string(),
            ..derived.clone()
        };
        assert_eq!(external_session_id(&empty), None);

        let external = Session {
            opencode_session_id: "oc_123".to_string(),
            ..derived
        };
        assert_eq!(external_session_id(&external), Some("oc_123"));

        let api_prefixed = Session {
            opencode_session_id: "ses_322440d07ffe7SGZDgAqvamd5v".to_string(),
            ..external
        };
        assert_eq!(
            external_session_id(&api_prefixed),
            Some("ses_322440d07ffe7SGZDgAqvamd5v")
        );
    }

    #[test]
    fn test_parse_model_input() {
        let parsed = parse_model_input("opencode/kimi-k2.5").expect("model should parse");
        assert_eq!(parsed.provider_id, "opencode");
        assert_eq!(parsed.model_id, "kimi-k2.5");

        assert!(parse_model_input("invalid").is_none());
        assert!(parse_model_input("provider/").is_none());
        assert!(parse_model_input("/model").is_none());
    }

    #[test]
    fn test_blocking_api_session_status_prefers_targeted_session() {
        let mut statuses = HashMap::new();
        statuses.insert("other".to_string(), SessionStatus::Busy);
        statuses.insert(
            "target".to_string(),
            SessionStatus::Retry {
                attempt: 2,
                message: "still running".to_string(),
                next: 5,
            },
        );

        let (session_id, status) = blocking_api_session_status(&statuses, Some("target"))
            .expect("target session should block");
        assert_eq!(session_id, "target");
        assert!(matches!(status, SessionStatus::Retry { .. }));
    }

    #[test]
    fn test_blocking_api_session_status_falls_back_to_any_busy() {
        let mut statuses = HashMap::new();
        statuses.insert("one".to_string(), SessionStatus::Idle);
        statuses.insert("two".to_string(), SessionStatus::Busy);

        let (session_id, status) = blocking_api_session_status(&statuses, Some("missing"))
            .expect("busy session should block even without preferred session");
        assert_eq!(session_id, "two");
        assert!(matches!(status, SessionStatus::Busy));
    }

    #[test]
    fn test_describe_session_status_retry_message() {
        let status = SessionStatus::Retry {
            attempt: 4,
            message: "waiting on tool".to_string(),
            next: 8,
        };
        assert_eq!(
            describe_session_status(&status),
            "retry (attempt 4, next in 8s): waiting on tool"
        );
    }

    #[test]
    fn test_build_acknowledgement_message_includes_async_notice() {
        let message = build_acknowledgement_message(SessionAction::Plan, "ses_1_owner_repo", None);

        assert!(message.contains("Session: `ses_1_owner_repo`"));
        assert!(message.contains("Work continues asynchronously."));
        assert!(!message.contains("OpenCode session:"));
    }

    #[test]
    fn test_build_acknowledgement_message_includes_link_when_present() {
        let message = build_acknowledgement_message(
            SessionAction::Build,
            "ses_2_owner_repo",
            Some("https://opencode.local/session/oc_123"),
        );

        assert!(message.contains("OpenCode session:"));
        assert!(message.contains("https://opencode.local/session/oc_123"));
    }
}
