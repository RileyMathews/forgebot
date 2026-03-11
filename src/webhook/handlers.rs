use axum::response::Response;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::db::{
    DbPool, NewSession, add_pending_worktree, get_repo_by_full_name, get_session_by_issue,
    get_session_by_pr, insert_session, update_session_mode, update_session_pr_id,
};
use crate::forgejo::ForgejoClient;
use crate::session::opencode::dispatch_session;
use crate::session::worktree::{bare_clone_path, remove_worktree, worktree_path};
use crate::session::{
    SessionAction, SessionMode, SessionState, SessionTrigger, comment_text_busy,
    comment_text_error, comment_text_no_context, comment_text_thinking, comment_text_working,
    derive_session_id,
};

use super::models::*;

fn ok_response(body: &str) -> Response {
    Response::builder()
        .status(200)
        .body(body.to_string().into())
        .unwrap()
}

async fn post_issue_comment_non_blocking(
    forgejo: &ForgejoClient,
    repo_full_name: &str,
    issue_number: u64,
    message: &str,
    context: &str,
) {
    if let Err(e) = forgejo
        .post_issue_comment(repo_full_name, issue_number, message)
        .await
    {
        warn!(
            repo = %repo_full_name,
            issue_id = %issue_number,
            err = %e,
            "Failed to post issue comment: {}",
            context
        );
    }
}

async fn post_pr_comment_non_blocking(
    forgejo: &ForgejoClient,
    repo_full_name: &str,
    pr_number: u64,
    message: &str,
    context: &str,
) {
    if let Err(e) = forgejo
        .post_pr_comment(repo_full_name, pr_number, message)
        .await
    {
        warn!(
            repo = %repo_full_name,
            pr_id = %pr_number,
            err = %e,
            "Failed to post PR comment: {}",
            context
        );
    }
}

/// Handle issue_comment webhook events
pub async fn handle_issue_comment(
    payload: IssueCommentPayload,
    db: &DbPool,
    forgejo: &ForgejoClient,
    config: &Config,
) -> Result<Response, axum::response::Response> {
    info!(
        repo = %payload.repository.full_name,
        issue_id = %payload.issue.number,
        action = %payload.action,
        sender = %payload.sender.login,
        "Received issue_comment webhook"
    );

    // 1. Ignore if comment author == config.forgejo.bot_username (loop prevention)
    if payload.sender.login == config.forgejo.bot_username {
        info!(
            "Ignoring comment from bot user '{}' (loop prevention)",
            config.forgejo.bot_username
        );
        return Ok(ok_response("OK - ignored bot comment"));
    }

    // 2. Ignore if repository repo_full_name not in repos table (not watched)
    match get_repo_by_full_name(db, &payload.repository.full_name).await {
        Ok(Some(_repo)) => {
            // Repo exists and is watched, continue processing
        }
        Ok(None) => {
            info!(
                "Repository '{}' not watched, ignoring comment",
                payload.repository.full_name
            );
            return Ok(ok_response("OK - repo not watched"));
        }
        Err(e) => {
            error!(
                repo = %payload.repository.full_name,
                err = %e,
                "Failed to check repo watch state"
            );
            // Continue processing - don't make Forgejo retry
        }
    }

    // 3. Ignore if comment body does not contain "@forgebot"
    if !payload.comment.body.contains("@forgebot") {
        info!("Comment does not contain @forgebot trigger, ignoring");
        return Ok(ok_response("OK - no @forgebot trigger"));
    }

    // 4. Parse trigger flags from comment
    let build_requested = parse_build_requested_from_comment(&payload.comment.body);
    info!(build_requested, "Parsed trigger flags from comment");

    // 5. Look up or create session row
    let issue_id = payload.issue.number as i64;
    let session_result = get_session_by_issue(db, &payload.repository.full_name, issue_id).await;

    match session_result {
        Ok(Some(session)) => {
            // Check if session is busy
            if session.state.is_busy() {
                info!(
                    "Session {} is busy (state: {}), posting busy comment",
                    session.id, session.state
                );
                let busy_msg = comment_text_busy();
                if let Err(e) = forgejo
                    .post_issue_comment(
                        &payload.repository.full_name,
                        payload.issue.number,
                        &busy_msg,
                    )
                    .await
                {
                    error!("Failed to post busy comment: {}", e);
                }
                return Ok(ok_response("OK - session busy"));
            }
        }
        Ok(None) => {
            // No existing session, will create one below
        }
        Err(e) => {
            error!("Failed to get session: {}", e);
            let err_msg = comment_text_error(&format!("Failed to load session: {}", e));
            post_issue_comment_non_blocking(
                forgejo,
                &payload.repository.full_name,
                payload.issue.number,
                &err_msg,
                "session load error",
            )
            .await;
            return Ok(ok_response("OK - error logged"));
        }
    }

    // 6. Create new session if needed and update state
    let session_id = derive_session_id(&payload.repository.full_name, payload.issue.number);
    // Check if session exists and create if not
    let existing_session =
        match get_session_by_issue(db, &payload.repository.full_name, issue_id).await {
            Ok(session) => session,
            Err(e) => {
                error!(
                    repo = %payload.repository.full_name,
                    issue_id = %payload.issue.number,
                    err = %e,
                    "Failed to load session before create"
                );
                let err_msg = comment_text_error(&format!("Failed to load session: {}", e));
                post_issue_comment_non_blocking(
                    forgejo,
                    &payload.repository.full_name,
                    payload.issue.number,
                    &err_msg,
                    "session load before create",
                )
                .await;
                return Ok(ok_response("OK - error logged"));
            }
        };

    let mut session_record = if let Some(session) = existing_session {
        session
    } else {
        // Create new session
        let worktree_path = worktree_path(
            &config.opencode,
            &payload.repository.full_name,
            payload.issue.number,
        );
        let new_session = NewSession {
            id: uuid::Uuid::new_v4().to_string(),
            repo_full_name: payload.repository.full_name.clone(),
            issue_id,
            pr_id: None,
            opencode_session_id: session_id.clone(),
            worktree_path: worktree_path.display().to_string(),
            state: SessionState::Idle.as_str().to_string(),
            mode: SessionMode::Collab.as_str().to_string(),
        };

        if let Err(e) = insert_session(db, &new_session).await {
            error!(
                repo = %payload.repository.full_name,
                issue_id = %payload.issue.number,
                err = %e,
                "Failed to create session"
            );
            let err_msg = comment_text_error(&format!("Failed to create session: {}", e));
            post_issue_comment_non_blocking(
                forgejo,
                &payload.repository.full_name,
                payload.issue.number,
                &err_msg,
                "session create error",
            )
            .await;
            return Ok(ok_response("OK - error logged"));
        }

        // Retrieve the newly created session
        match get_session_by_issue(db, &payload.repository.full_name, issue_id).await {
            Ok(Some(session)) => session,
            Ok(None) => {
                error!(
                    repo = %payload.repository.full_name,
                    issue_id = %payload.issue.number,
                    "Session missing immediately after create"
                );
                return Ok(ok_response("OK - session creation error"));
            }
            Err(e) => {
                error!(
                    repo = %payload.repository.full_name,
                    issue_id = %payload.issue.number,
                    err = %e,
                    "Failed to retrieve newly created session"
                );
                return Ok(ok_response("OK - session creation error"));
            }
        }
    };

    // 7. Persist mode switch to build when requested.
    if build_requested && session_record.mode != SessionMode::Build {
        if let Err(e) =
            update_session_mode(db, &session_record.id, SessionMode::Build.as_str()).await
        {
            error!(
                repo = %payload.repository.full_name,
                issue_id = %payload.issue.number,
                session_id = %session_record.id,
                err = %e,
                "Failed to update session mode"
            );
            let err_msg =
                comment_text_error(&format!("Failed to switch session to build mode: {}", e));
            post_issue_comment_non_blocking(
                forgejo,
                &payload.repository.full_name,
                payload.issue.number,
                &err_msg,
                "session mode update error",
            )
            .await;
            return Ok(ok_response("OK - error logged"));
        }
        session_record.mode = SessionMode::Build;
    }

    let action = session_record.mode.action();
    info!(mode = %session_record.mode.as_str(), action = %action.as_str(), "Selected session mode and action");

    // 8. Post acknowledgement comment
    let ack_msg = match action {
        SessionAction::Plan => comment_text_thinking(),
        SessionAction::Build | SessionAction::Revision => comment_text_working(),
    };

    post_issue_comment_non_blocking(
        forgejo,
        &payload.repository.full_name,
        payload.issue.number,
        &ack_msg,
        "acknowledgement",
    )
    .await;

    // 9. Create SessionTrigger and dispatch in background task
    let trigger = SessionTrigger {
        repo_full_name: payload.repository.full_name.clone(),
        issue_id: payload.issue.number,
        pr_id: None,
        action,
    };

    // Clone values for the spawned task
    let db_clone = db.clone();
    let forgejo_clone = forgejo.clone();
    let config_clone = config.clone();

    tokio::spawn(async move {
        if let Err(e) = dispatch_session(&db_clone, &forgejo_clone, &config_clone, trigger).await {
            error!(
                "dispatch_session failed for {} issue {}: {}",
                payload.repository.full_name, payload.issue.number, e
            );
        }
    });

    // 10. Return 200 immediately (non-blocking)
    Ok(ok_response("OK - dispatching session"))
}

/// Parse build trigger from comment body.
/// - "--build" anywhere in the comment switches to build mode
/// - "@forgebot build" is accepted for backwards compatibility
fn parse_build_requested_from_comment(body: &str) -> bool {
    let body_lower = body.to_lowercase();

    if body_lower.contains("--build") {
        return true;
    }

    // Look for @forgebot followed by action keyword
    if let Some(idx) = body_lower.find("@forgebot") {
        let after_trigger = &body_lower[idx..];
        let words: Vec<&str> = after_trigger.split_whitespace().collect();

        // Check second word (first word is @forgebot)
        if words.len() > 1 && words[1] == "build" {
            return true;
        }
    }

    false
}

/// Handle pull_request webhook events (opened, closed, merged)
pub async fn handle_pull_request(
    payload: PullRequestPayload,
    db: &DbPool,
    forgejo: &ForgejoClient,
    config: &Config,
) -> Result<Response, axum::response::Response> {
    info!(
        repo = %payload.repository.full_name,
        pr_id = %payload.pull_request.number,
        action = %payload.action,
        sender = %payload.sender.login,
        "Received pull_request webhook"
    );

    match payload.action.as_str() {
        "opened" => handle_pr_opened(&payload, db, forgejo, config).await,
        "closed" | "merged" => handle_pr_closed(&payload, db, forgejo, config).await,
        _ => {
            info!("Ignoring pull_request action: {}", payload.action);
            Ok(ok_response("OK - unhandled action"))
        }
    }
}

/// Handle PR opened action - link PR to session
async fn handle_pr_opened(
    payload: &PullRequestPayload,
    db: &DbPool,
    _forgejo: &ForgejoClient,
    _config: &Config,
) -> Result<Response, axum::response::Response> {
    // Parse head branch; extract issue ID from `agent/issue-<id>` pattern
    let head_ref = &payload.pull_request.head.ref_field;
    let issue_id = match extract_issue_id_from_branch(head_ref) {
        Some(id) => id,
        None => {
            warn!(
                "PR head branch '{}' does not match agent/issue-<id> pattern, ignoring",
                head_ref
            );
            return Ok(ok_response("OK - not a forgebot PR"));
        }
    };

    info!("PR opened for issue {} on branch {}", issue_id, head_ref);

    // Look up session by (repo_full_name, issue_id)
    let session =
        match get_session_by_issue(db, &payload.repository.full_name, issue_id as i64).await {
            Ok(Some(session)) => session,
            Ok(None) => {
                warn!(
                    "No session found for {} issue {}, cannot link PR",
                    payload.repository.full_name, issue_id
                );
                return Ok(ok_response("OK - no session found"));
            }
            Err(e) => {
                error!("Failed to get session: {}", e);
                return Ok(ok_response("OK - database error"));
            }
        };

    // Update session row with PR ID
    let pr_id = payload.pull_request.number as i64;
    if let Err(e) = update_session_pr_id(db, &session.id, pr_id).await {
        error!("Failed to update session PR ID: {}", e);
        return Ok(ok_response("OK - update error"));
    }

    info!("Linked PR {} to session {}", pr_id, session.id);

    Ok(ok_response("OK - PR linked to session"))
}

/// Handle PR closed/merged action - queue worktree cleanup
async fn handle_pr_closed(
    payload: &PullRequestPayload,
    db: &DbPool,
    _forgejo: &ForgejoClient,
    config: &Config,
) -> Result<Response, axum::response::Response> {
    // Parse head branch for `agent/issue-<id>` pattern
    let head_ref = &payload.pull_request.head.ref_field;
    let issue_id = match extract_issue_id_from_branch(head_ref) {
        Some(id) => id,
        None => {
            info!(
                "PR head branch '{}' does not match agent/issue-<id> pattern, ignoring",
                head_ref
            );
            return Ok(ok_response("OK - not a forgebot PR"));
        }
    };

    info!(
        "PR closed/merged for issue {} on branch {}",
        issue_id, head_ref
    );

    // Look up session by (repo_full_name, issue_id)
    let session =
        match get_session_by_issue(db, &payload.repository.full_name, issue_id as i64).await {
            Ok(Some(session)) => session,
            Ok(None) => {
                info!(
                    "No session found for {} issue {}, nothing to clean up",
                    payload.repository.full_name, issue_id
                );
                return Ok(ok_response("OK - no session"));
            }
            Err(e) => {
                error!("Failed to get session: {}", e);
                return Ok(ok_response("OK - database error"));
            }
        };

    // Insert into pending_worktrees table
    if let Err(e) = add_pending_worktree(db, &session.id, &session.worktree_path).await {
        error!("Failed to add pending worktree: {}", e);
        // Continue anyway to try removing the worktree
    }

    // Get worktree path and remove it
    let worktree_path = worktree_path(&config.opencode, &payload.repository.full_name, issue_id);

    // Spawn worktree removal in background
    let worktree_path_clone = worktree_path.clone();
    let session_id_clone = session.id.clone();
    let git_binary = config.opencode.git_binary.clone();
    let bare_path = bare_clone_path(&config.opencode, &payload.repository.full_name);
    tokio::spawn(async move {
        if let Err(e) = remove_worktree(&worktree_path_clone, &bare_path, &git_binary).await {
            error!(
                "Failed to remove worktree for session {}: {}",
                session_id_clone, e
            );
        } else {
            info!(
                "Successfully removed worktree for session {}",
                session_id_clone
            );
        }
    });

    Ok(ok_response("OK - worktree cleanup queued"))
}

/// Extract issue ID from branch name like "agent/issue-42"
fn extract_issue_id_from_branch(branch: &str) -> Option<u64> {
    // Handle both "agent/issue-42" and "refs/heads/agent/issue-42"
    let branch_clean = branch.trim_start_matches("refs/heads/");

    if let Some(idx) = branch_clean.find("agent/issue-") {
        let after_prefix = &branch_clean[idx + "agent/issue-".len()..];
        // Parse the number until we hit a non-digit
        let num_str: String = after_prefix
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        num_str.parse::<u64>().ok()
    } else {
        None
    }
}

/// Handle pull_request_review_comment webhook events
pub async fn handle_pull_request_review_comment(
    payload: PullRequestReviewCommentPayload,
    db: &DbPool,
    forgejo: &ForgejoClient,
    config: &Config,
) -> Result<Response, axum::response::Response> {
    info!(
        repo = %payload.repository.full_name,
        pr_id = %payload.pull_request.number,
        comment_path = %payload.review_comment.path,
        sender = %payload.sender.login,
        "Received pull_request_review_comment webhook"
    );

    // 1. Ignore if author == bot username
    if payload.sender.login == config.forgejo.bot_username {
        info!(
            "Ignoring review comment from bot user '{}' (loop prevention)",
            config.forgejo.bot_username
        );
        return Ok(ok_response("OK - ignored bot comment"));
    }

    // 2. Ignore if comment body does not contain "@forgebot"
    if !payload.review_comment.body.contains("@forgebot") {
        info!("Review comment does not contain @forgebot trigger, ignoring");
        return Ok(ok_response("OK - no @forgebot trigger"));
    }

    // 3. Look up session by PR ID
    let pr_id = payload.pull_request.number as i64;
    let session = match get_session_by_pr(db, pr_id).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            warn!(
                "No session found for PR {}, posting hard-fail comment",
                pr_id
            );
            let fail_msg = comment_text_no_context();
            post_pr_comment_non_blocking(
                forgejo,
                &payload.repository.full_name,
                payload.pull_request.number,
                &fail_msg,
                "missing session context",
            )
            .await;
            return Ok(ok_response("OK - no session context"));
        }
        Err(e) => {
            error!("Failed to get session by PR: {}", e);
            let err_msg = comment_text_error(&format!("Failed to load session: {}", e));
            post_pr_comment_non_blocking(
                forgejo,
                &payload.repository.full_name,
                payload.pull_request.number,
                &err_msg,
                "session load by PR error",
            )
            .await;
            return Ok(ok_response("OK - error logged"));
        }
    };

    // 4. Check if session is busy
    if session.state.is_busy() {
        info!(
            "Session {} is busy (state: {}), posting busy comment",
            session.id, session.state
        );
        let busy_msg = comment_text_busy();
        post_pr_comment_non_blocking(
            forgejo,
            &payload.repository.full_name,
            payload.pull_request.number,
            &busy_msg,
            "session busy",
        )
        .await;
        return Ok(ok_response("OK - session busy"));
    }

    // 5. Post acknowledgement comment on PR
    let ack_msg = "🤖 forgebot is addressing review comments...".to_string();
    post_pr_comment_non_blocking(
        forgejo,
        &payload.repository.full_name,
        payload.pull_request.number,
        &ack_msg,
        "revision acknowledgement",
    )
    .await;

    // 6. Create SessionTrigger with action "revision" and dispatch
    let trigger = SessionTrigger {
        repo_full_name: payload.repository.full_name.clone(),
        issue_id: session.issue_id as u64,
        pr_id: Some(payload.pull_request.number),
        action: SessionAction::Revision,
    };

    // Clone values for the spawned task
    let db_clone = db.clone();
    let forgejo_clone = forgejo.clone();
    let config_clone = config.clone();

    tokio::spawn(async move {
        if let Err(e) = dispatch_session(&db_clone, &forgejo_clone, &config_clone, trigger).await {
            error!(
                "dispatch_session failed for revision on PR {}: {}",
                pr_id, e
            );
        }
    });

    // 7. Return 200 immediately
    Ok(ok_response("OK - dispatching revision"))
}

/// Handle unknown webhook events
pub async fn handle_unknown_event(event_type: &str) -> Result<Response, axum::response::Response> {
    warn!(
        "Webhook received: unknown event type '{}', ignoring",
        event_type
    );

    // Return 200 OK - we don't want Forgejo to retry unknown events
    Ok(ok_response("OK"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_build_requested_from_comment() {
        assert!(parse_build_requested_from_comment("@forgebot --build"));
        assert!(parse_build_requested_from_comment(
            "Hey @forgebot can you do this --build please"
        ));
        assert!(parse_build_requested_from_comment("@forgebot build"));
        assert!(parse_build_requested_from_comment("@FORGEBOT BUILD"));

        assert!(!parse_build_requested_from_comment("@forgebot"));
        assert!(!parse_build_requested_from_comment("@forgebot plan"));
        assert!(!parse_build_requested_from_comment("just a comment"));
    }

    #[test]
    fn test_extract_issue_id_from_branch() {
        // Standard format
        assert_eq!(extract_issue_id_from_branch("agent/issue-42"), Some(42));
        assert_eq!(extract_issue_id_from_branch("agent/issue-123"), Some(123));
        assert_eq!(extract_issue_id_from_branch("agent/issue-1"), Some(1));

        // With refs/heads/ prefix
        assert_eq!(
            extract_issue_id_from_branch("refs/heads/agent/issue-42"),
            Some(42)
        );

        // No match
        assert_eq!(extract_issue_id_from_branch("feature/something"), None);
        assert_eq!(extract_issue_id_from_branch("main"), None);
        assert_eq!(extract_issue_id_from_branch("agent/other-42"), None);

        // Edge cases
        assert_eq!(extract_issue_id_from_branch("agent/issue-"), None);
        assert_eq!(
            extract_issue_id_from_branch("agent/issue-42-extra"),
            Some(42)
        );
    }
}
