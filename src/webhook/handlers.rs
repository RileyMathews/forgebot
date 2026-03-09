use axum::response::Response;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::db::{
    DbPool, NewSession, add_pending_worktree, get_repo_by_full_name, get_session_by_issue,
    get_session_by_pr, insert_session, update_session_pr_id,
};
use crate::forgejo::ForgejoClient;
use crate::session::opencode::dispatch_session;
use crate::session::worktree::{remove_worktree, worktree_path};
use crate::session::{
    SessionTrigger, comment_text_busy, comment_text_error, comment_text_no_context,
    comment_text_thinking, comment_text_working, derive_session_id,
};

use super::models::*;

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
        // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
        return Ok(Response::builder()
            .status(200)
            .body("OK - ignored bot comment".into())
            .unwrap());
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
            // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
            return Ok(Response::builder()
                .status(200)
                .body("OK - repo not watched".into())
                .unwrap());
        }
        Err(e) => {
            error!(
                "Failed to check repo {}: {}",
                payload.repository.full_name, e
            );
            // Continue processing - don't make Forgejo retry
        }
    }

    // 3. Ignore if comment body does not contain "@forgebot"
    if !payload.comment.body.contains("@forgebot") {
        info!("Comment does not contain @forgebot trigger, ignoring");
        // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
        return Ok(Response::builder()
            .status(200)
            .body("OK - no @forgebot trigger".into())
            .unwrap());
    }

    // 4. Parse action from trigger
    let action = parse_action_from_comment(&payload.comment.body);
    info!("Parsed action '{}' from comment", action);

    // 5. Look up or create session row
    let issue_id = payload.issue.number as i64;
    let session_result = get_session_by_issue(db, &payload.repository.full_name, issue_id).await;

    match session_result {
        Ok(Some(session)) => {
            // Check if session is busy
            if session.state == "planning" || session.state == "building" {
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
                // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
                return Ok(Response::builder()
                    .status(200)
                    .body("OK - session busy".into())
                    .unwrap());
            }
        }
        Ok(None) => {
            // No existing session, will create one below
        }
        Err(e) => {
            error!("Failed to get session: {}", e);
            let err_msg = comment_text_error(&format!("Failed to load session: {}", e));
            let _ = forgejo
                .post_issue_comment(
                    &payload.repository.full_name,
                    payload.issue.number,
                    &err_msg,
                )
                .await;
            // Return 200 so Forgejo doesn't retry; Response::builder() cannot fail with standard strings
            return Ok(Response::builder()
                .status(200)
                .body("OK - error logged".into())
                .unwrap());
        }
    }

    // 6. Create new session if needed and update state
    let session_id = derive_session_id(&payload.repository.full_name, payload.issue.number);
    let _new_state = match action.as_str() {
        "plan" => "planning",
        "build" => "building",
        _ => "planning", // Default to planning for unknown actions
    };

    // Check if session exists and create if not
    let existing_session = get_session_by_issue(db, &payload.repository.full_name, issue_id)
        .await
        .ok()
        .flatten();

    let _session_record = if let Some(session) = existing_session {
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
            state: "idle".to_string(),
        };

        if let Err(e) = insert_session(db, &new_session).await {
            error!("Failed to create session: {}", e);
            let err_msg = comment_text_error(&format!("Failed to create session: {}", e));
            let _ = forgejo
                .post_issue_comment(
                    &payload.repository.full_name,
                    payload.issue.number,
                    &err_msg,
                )
                .await;
            // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
            return Ok(Response::builder()
                .status(200)
                .body("OK - error logged".into())
                .unwrap());
        }

        // Retrieve the newly created session
        match get_session_by_issue(db, &payload.repository.full_name, issue_id).await {
            Ok(Some(session)) => session,
            _ => {
                error!("Failed to retrieve newly created session");
                // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
                return Ok(Response::builder()
                    .status(200)
                    .body("OK - session creation error".into())
                    .unwrap());
            }
        }
    };

    // 7. Post acknowledgement comment
    let ack_msg = match action.as_str() {
        "plan" => comment_text_thinking(),
        "build" => comment_text_working(),
        _ => comment_text_thinking(),
    };

    if let Err(e) = forgejo
        .post_issue_comment(
            &payload.repository.full_name,
            payload.issue.number,
            &ack_msg,
        )
        .await
    {
        error!("Failed to post acknowledgement comment: {}", e);
    }

    // 8. Create SessionTrigger and dispatch in background task
    let trigger = SessionTrigger {
        repo_full_name: payload.repository.full_name.clone(),
        issue_id: payload.issue.number,
        pr_id: None,
        action: action.clone(),
        comment_body: payload.comment.body.clone(),
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

    // 9. Return 200 immediately (non-blocking)
    // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
    Ok(Response::builder()
        .status(200)
        .body("OK - dispatching session".into())
        .unwrap())
}

/// Parse action keyword from comment body
/// - "plan" = plan mode
/// - "build" = build mode
/// - anything else = use current session state (default to plan)
fn parse_action_from_comment(body: &str) -> String {
    let body_lower = body.to_lowercase();

    // Look for @forgebot followed by action keyword
    if let Some(idx) = body_lower.find("@forgebot") {
        let after_trigger = &body_lower[idx..];
        let words: Vec<&str> = after_trigger.split_whitespace().collect();

        // Check second word (first word is @forgebot)
        if words.len() > 1 {
            match words[1] {
                "plan" => return "plan".to_string(),
                "build" => return "build".to_string(),
                _ => {}
            }
        }
    }

    // Default to plan if no explicit action found
    "plan".to_string()
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
            // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
            Ok(Response::builder()
                .status(200)
                .body("OK - unhandled action".into())
                .unwrap())
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
            // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
            return Ok(Response::builder()
                .status(200)
                .body("OK - not a forgebot PR".into())
                .unwrap());
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
                // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
                return Ok(Response::builder()
                    .status(200)
                    .body("OK - no session found".into())
                    .unwrap());
            }
            Err(e) => {
                error!("Failed to get session: {}", e);
                // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
                return Ok(Response::builder()
                    .status(200)
                    .body("OK - database error".into())
                    .unwrap());
            }
        };

    // Update session row with PR ID
    let pr_id = payload.pull_request.number as i64;
    if let Err(e) = update_session_pr_id(db, &session.id, pr_id).await {
        error!("Failed to update session PR ID: {}", e);
        // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
        return Ok(Response::builder()
            .status(200)
            .body("OK - update error".into())
            .unwrap());
    }

    info!("Linked PR {} to session {}", pr_id, session.id);

    // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
    Ok(Response::builder()
        .status(200)
        .body("OK - PR linked to session".into())
        .unwrap())
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
            // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
            return Ok(Response::builder()
                .status(200)
                .body("OK - not a forgebot PR".into())
                .unwrap());
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
                // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
                return Ok(Response::builder()
                    .status(200)
                    .body("OK - no session".into())
                    .unwrap());
            }
            Err(e) => {
                error!("Failed to get session: {}", e);
                // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
                return Ok(Response::builder()
                    .status(200)
                    .body("OK - database error".into())
                    .unwrap());
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
    tokio::spawn(async move {
        if let Err(e) = remove_worktree(&worktree_path_clone).await {
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

    // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
    Ok(Response::builder()
        .status(200)
        .body("OK - worktree cleanup queued".into())
        .unwrap())
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
        // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
        return Ok(Response::builder()
            .status(200)
            .body("OK - ignored bot comment".into())
            .unwrap());
    }

    // 2. Ignore if comment body does not contain "@forgebot"
    if !payload.review_comment.body.contains("@forgebot") {
        info!("Review comment does not contain @forgebot trigger, ignoring");
        // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
        return Ok(Response::builder()
            .status(200)
            .body("OK - no @forgebot trigger".into())
            .unwrap());
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
            let _ = forgejo
                .post_pr_comment(
                    &payload.repository.full_name,
                    payload.pull_request.number,
                    &fail_msg,
                )
                .await;
            // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
            return Ok(Response::builder()
                .status(200)
                .body("OK - no session context".into())
                .unwrap());
        }
        Err(e) => {
            error!("Failed to get session by PR: {}", e);
            let err_msg = comment_text_error(&format!("Failed to load session: {}", e));
            let _ = forgejo
                .post_pr_comment(
                    &payload.repository.full_name,
                    payload.pull_request.number,
                    &err_msg,
                )
                .await;
            // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
            return Ok(Response::builder()
                .status(200)
                .body("OK - error logged".into())
                .unwrap());
        }
    };

    // 4. Check if session is busy
    if session.state == "planning" || session.state == "building" || session.state == "revising" {
        info!(
            "Session {} is busy (state: {}), posting busy comment",
            session.id, session.state
        );
        let busy_msg = comment_text_busy();
        let _ = forgejo
            .post_pr_comment(
                &payload.repository.full_name,
                payload.pull_request.number,
                &busy_msg,
            )
            .await;
        // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
        return Ok(Response::builder()
            .status(200)
            .body("OK - session busy".into())
            .unwrap());
    }

    // 5. Post acknowledgement comment on PR
    let ack_msg = "🤖 forgebot is addressing review comments...".to_string();
    if let Err(e) = forgejo
        .post_pr_comment(
            &payload.repository.full_name,
            payload.pull_request.number,
            &ack_msg,
        )
        .await
    {
        error!("Failed to post acknowledgement comment: {}", e);
    }

    // 6. Create SessionTrigger with action "revision" and dispatch
    let trigger = SessionTrigger {
        repo_full_name: payload.repository.full_name.clone(),
        issue_id: session.issue_id as u64,
        pr_id: Some(payload.pull_request.number),
        action: "revision".to_string(),
        comment_body: payload.review_comment.body.clone(),
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
    // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
    Ok(Response::builder()
        .status(200)
        .body("OK - dispatching revision".into())
        .unwrap())
}

/// Handle unknown webhook events
pub async fn handle_unknown_event(event_type: &str) -> Result<Response, axum::response::Response> {
    warn!(
        "Webhook received: unknown event type '{}', ignoring",
        event_type
    );

    // Return 200 OK - we don't want Forgejo to retry unknown events
    // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
    Ok(Response::builder().status(200).body("OK".into()).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_action_from_comment() {
        // Plan action
        assert_eq!(parse_action_from_comment("@forgebot plan"), "plan");
        assert_eq!(
            parse_action_from_comment("Hey @forgebot plan this issue"),
            "plan"
        );
        assert_eq!(parse_action_from_comment("@FORGEBOT PLAN"), "plan"); // case insensitive

        // Build action
        assert_eq!(parse_action_from_comment("@forgebot build"), "build");
        assert_eq!(
            parse_action_from_comment("Please @forgebot build this"),
            "build"
        );

        // Unknown action defaults to plan
        assert_eq!(parse_action_from_comment("@forgebot something"), "plan");
        assert_eq!(parse_action_from_comment("@forgebot"), "plan");
        assert_eq!(parse_action_from_comment("just a comment"), "plan");
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
