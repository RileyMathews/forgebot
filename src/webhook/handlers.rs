use axum::response::Response;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::db::{
    DbPool, NewSession, Session, add_pending_worktree, get_repo_by_full_name, get_session_by_issue,
    get_session_by_pr, insert_session, update_session_pr_id,
};
use crate::forgejo::ForgejoClient;
use crate::session::opencode::dispatch_session;
use crate::session::worktree::{bare_clone_path, remove_worktree, worktree_path};
use crate::session::{
    SessionAction, SessionMode, SessionState, SessionTrigger, comment_text_error,
    comment_text_no_context, derive_session_id,
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

enum IssueCommentSessionResult {
    Ready(Session),
    ErrorLogged,
    CreationError,
}

fn pr_number_from_issue_comment(payload: &IssueCommentPayload) -> Option<u64> {
    payload
        .issue
        .pull_request
        .as_ref()
        .map(|_| payload.issue.number)
}

async fn validate_watched_repo(db: &DbPool, repo_full_name: &str) -> Option<Response> {
    match get_repo_by_full_name(db, repo_full_name).await {
        Ok(Some(_repo)) => None,
        Ok(None) => {
            info!(
                "Repository '{}' not watched, ignoring comment",
                repo_full_name
            );
            Some(ok_response("OK - repo not watched"))
        }
        Err(e) => {
            error!(
                repo = %repo_full_name,
                err = %e,
                "Failed to check repo watch state"
            );
            None
        }
    }
}

async fn load_or_create_issue_session(
    payload: &IssueCommentPayload,
    db: &DbPool,
    forgejo: &ForgejoClient,
    config: &Config,
) -> IssueCommentSessionResult {
    let issue_id = payload.issue.number as i64;
    let existing_session =
        match get_session_by_issue(db, &payload.repository.full_name, issue_id).await {
            Ok(session) => session,
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
                return IssueCommentSessionResult::ErrorLogged;
            }
        };

    if let Some(session) = existing_session {
        return IssueCommentSessionResult::Ready(session);
    }

    let session_id = derive_session_id(&payload.repository.full_name, payload.issue.number);
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
        opencode_session_id: session_id,
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
        return IssueCommentSessionResult::ErrorLogged;
    }

    match get_session_by_issue(db, &payload.repository.full_name, issue_id).await {
        Ok(Some(session)) => IssueCommentSessionResult::Ready(session),
        Ok(None) => {
            error!(
                repo = %payload.repository.full_name,
                issue_id = %payload.issue.number,
                "Session missing immediately after create"
            );
            IssueCommentSessionResult::CreationError
        }
        Err(e) => {
            error!(
                repo = %payload.repository.full_name,
                issue_id = %payload.issue.number,
                err = %e,
                "Failed to retrieve newly created session"
            );
            IssueCommentSessionResult::CreationError
        }
    }
}

fn spawn_issue_dispatch(
    payload: IssueCommentPayload,
    db: DbPool,
    forgejo: ForgejoClient,
    config: Config,
    action: SessionAction,
    in_flight_issue_triggers: Arc<Mutex<HashSet<String>>>,
) {
    let trigger = SessionTrigger {
        repo_full_name: payload.repository.full_name.clone(),
        issue_id: payload.issue.number,
        pr_id: None,
        action,
    };

    tokio::spawn(async move {
        if let Err(e) = dispatch_session(&db, &forgejo, &config, trigger).await {
            error!(
                "dispatch_session failed for {} issue {}: {}",
                payload.repository.full_name, payload.issue.number, e
            );
        }

        let lock_key = format!("{}#{}", payload.repository.full_name, payload.issue.number);
        let mut in_flight = in_flight_issue_triggers.lock().await;
        in_flight.remove(&lock_key);
    });
}

async fn mark_issue_comment_event_seen(
    processed_issue_comment_events: &Arc<Mutex<HashSet<String>>>,
    payload: &IssueCommentPayload,
) -> bool {
    let event_key = format!(
        "issue_comment:{}:{}:{}",
        payload.repository.full_name, payload.comment.id, payload.action
    );

    let mut processed = processed_issue_comment_events.lock().await;
    if processed.contains(&event_key) {
        return false;
    }

    processed.insert(event_key);
    true
}

async fn acquire_issue_dispatch_lock(
    in_flight_issue_triggers: &Arc<Mutex<HashSet<String>>>,
    repo_full_name: &str,
    issue_id: u64,
) -> bool {
    let lock_key = format!("{}#{}", repo_full_name, issue_id);
    let mut in_flight = in_flight_issue_triggers.lock().await;

    if in_flight.contains(&lock_key) {
        return false;
    }

    in_flight.insert(lock_key);
    true
}

async fn release_issue_dispatch_lock(
    in_flight_issue_triggers: &Arc<Mutex<HashSet<String>>>,
    repo_full_name: &str,
    issue_id: u64,
) {
    let lock_key = format!("{}#{}", repo_full_name, issue_id);
    let mut in_flight = in_flight_issue_triggers.lock().await;
    in_flight.remove(&lock_key);
}

/// Handle issue_comment webhook events
pub async fn handle_issue_comment(
    payload: IssueCommentPayload,
    db: &DbPool,
    forgejo: &ForgejoClient,
    config: &Config,
    forgejo_user_id: u64,
    in_flight_issue_triggers: &Arc<Mutex<HashSet<String>>>,
    processed_issue_comment_events: &Arc<Mutex<HashSet<String>>>,
) -> Result<Response, axum::response::Response> {
    info!(
        repo = %payload.repository.full_name,
        issue_id = %payload.issue.number,
        action = %payload.action,
        sender = %payload.sender.login,
        "Received issue_comment webhook"
    );

    // 1. Ignore comments from the authenticated Forgejo account (loop prevention)
    if payload.sender.id == forgejo_user_id {
        info!(
            sender_id = %payload.sender.id,
            sender_login = %payload.sender.login,
            bot_user_id = %forgejo_user_id,
            "Ignoring comment from authenticated Forgejo account (loop prevention)"
        );
        return Ok(ok_response("OK - ignored bot comment"));
    }

    // 2. Ignore if repository repo_full_name not in repos table (not watched)
    if let Some(response) = validate_watched_repo(db, &payload.repository.full_name).await {
        return Ok(response);
    }

    // 3. Ignore if comment body does not contain "@forgebot"
    if !payload.comment.body.contains("@forgebot") {
        info!("Comment does not contain @forgebot trigger, ignoring");
        return Ok(ok_response("OK - no @forgebot trigger"));
    }

    // 4. Ignore duplicate deliveries for the same comment event
    if !mark_issue_comment_event_seen(processed_issue_comment_events, &payload).await {
        info!(
            repo = %payload.repository.full_name,
            issue_id = %payload.issue.number,
            comment_id = %payload.comment.id,
            action = %payload.action,
            "Ignoring duplicate issue_comment delivery"
        );
        return Ok(ok_response("OK - duplicate issue_comment ignored"));
    }

    // 5. Prevent concurrent dispatches for the same issue (fail closed)
    if !acquire_issue_dispatch_lock(
        in_flight_issue_triggers,
        &payload.repository.full_name,
        payload.issue.number,
    )
    .await
    {
        info!(
            repo = %payload.repository.full_name,
            issue_id = %payload.issue.number,
            "Ignoring trigger while another dispatch is already in-flight"
        );
        return Ok(ok_response("OK - dispatch already in-flight"));
    }

    // 6. Route PR timeline comments through PR session resume, issue comments through issue flow.
    if let Some(pr_number) = pr_number_from_issue_comment(&payload) {
        let session = match get_session_by_pr(db, pr_number as i64).await {
            Ok(Some(session)) => session,
            Ok(None) => {
                warn!(
                    repo = %payload.repository.full_name,
                    pr_id = %pr_number,
                    "No session found for PR timeline comment"
                );
                let fail_msg = comment_text_no_context();
                post_pr_comment_non_blocking(
                    forgejo,
                    &payload.repository.full_name,
                    pr_number,
                    &fail_msg,
                    "missing session context",
                )
                .await;
                release_issue_dispatch_lock(
                    in_flight_issue_triggers,
                    &payload.repository.full_name,
                    payload.issue.number,
                )
                .await;
                return Ok(ok_response("OK - no session context"));
            }
            Err(e) => {
                error!(
                    repo = %payload.repository.full_name,
                    pr_id = %pr_number,
                    err = %e,
                    "Failed to load session for PR timeline comment"
                );
                let err_msg = comment_text_error(&format!("Failed to load session: {}", e));
                post_pr_comment_non_blocking(
                    forgejo,
                    &payload.repository.full_name,
                    pr_number,
                    &err_msg,
                    "session load by PR error",
                )
                .await;
                release_issue_dispatch_lock(
                    in_flight_issue_triggers,
                    &payload.repository.full_name,
                    payload.issue.number,
                )
                .await;
                return Ok(ok_response("OK - error logged"));
            }
        };

        let action = SessionAction::Revision;
        info!(
            mode = %session.mode.as_str(),
            action = %action.as_str(),
            issue_id = %session.issue_id,
            pr_id = %pr_number,
            "Selected PR timeline issue-comment action"
        );

        spawn_pr_timeline_dispatch(
            payload,
            db.clone(),
            forgejo.clone(),
            config.clone(),
            session.issue_id as u64,
            Arc::clone(in_flight_issue_triggers),
        );
    } else {
        // 6. Look up or create session row
        let session_record = match load_or_create_issue_session(&payload, db, forgejo, config).await
        {
            IssueCommentSessionResult::Ready(session) => session,
            IssueCommentSessionResult::ErrorLogged => {
                release_issue_dispatch_lock(
                    in_flight_issue_triggers,
                    &payload.repository.full_name,
                    payload.issue.number,
                )
                .await;
                return Ok(ok_response("OK - error logged"));
            }
            IssueCommentSessionResult::CreationError => {
                release_issue_dispatch_lock(
                    in_flight_issue_triggers,
                    &payload.repository.full_name,
                    payload.issue.number,
                )
                .await;
                return Ok(ok_response("OK - session creation error"));
            }
        };

        let action = SessionAction::Plan;
        info!(
            mode = %session_record.mode.as_str(),
            action = %action.as_str(),
            "Selected issue-comment action"
        );

        // 7. Create SessionTrigger and dispatch in background task.
        // Acknowledgement comments are posted from dispatch after admission gate passes.
        spawn_issue_dispatch(
            payload,
            db.clone(),
            forgejo.clone(),
            config.clone(),
            action,
            Arc::clone(in_flight_issue_triggers),
        );
    }

    // 8. Return 200 immediately (non-blocking)
    Ok(ok_response("OK - dispatching session"))
}

fn spawn_pr_timeline_dispatch(
    payload: IssueCommentPayload,
    db: DbPool,
    forgejo: ForgejoClient,
    config: Config,
    issue_id: u64,
    in_flight_issue_triggers: Arc<Mutex<HashSet<String>>>,
) {
    let pr_number = payload.issue.number;
    let trigger = SessionTrigger {
        repo_full_name: payload.repository.full_name.clone(),
        issue_id,
        pr_id: Some(pr_number),
        action: SessionAction::Revision,
    };

    tokio::spawn(async move {
        if let Err(e) = dispatch_session(&db, &forgejo, &config, trigger).await {
            error!(
                "dispatch_session failed for {} PR {}: {}",
                payload.repository.full_name, pr_number, e
            );
        }

        let lock_key = format!("{}#{}", payload.repository.full_name, payload.issue.number);
        let mut in_flight = in_flight_issue_triggers.lock().await;
        in_flight.remove(&lock_key);
    });
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
    forgejo_user_id: u64,
) -> Result<Response, axum::response::Response> {
    info!(
        repo = %payload.repository.full_name,
        pr_id = %payload.pull_request.number,
        comment_path = %payload.review_comment.path,
        sender = %payload.sender.login,
        "Received pull_request_review_comment webhook"
    );

    // 1. Ignore if author matches the authenticated Forgejo account
    if payload.sender.id == forgejo_user_id {
        info!(
            sender_id = %payload.sender.id,
            sender_login = %payload.sender.login,
            bot_user_id = %forgejo_user_id,
            "Ignoring review comment from authenticated Forgejo account (loop prevention)"
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

    // 4. Post acknowledgement comment on PR
    let ack_msg = "🤖 forgebot is addressing review comments...".to_string();
    post_pr_comment_non_blocking(
        forgejo,
        &payload.repository.full_name,
        payload.pull_request.number,
        &ack_msg,
        "revision acknowledgement",
    )
    .await;

    // 5. Create SessionTrigger with action "revision" and dispatch
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

    // 6. Return 200 immediately
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

    #[test]
    fn test_pr_number_from_issue_comment_when_comment_is_on_pr() {
        let payload: IssueCommentPayload = serde_json::from_str(
            r#"{
                "action": "created",
                "issue": {
                    "number": 77,
                    "title": "Some PR",
                    "body": null,
                    "state": "open",
                    "pull_request": {
                        "url": "https://forgejo.local/api/v1/repos/acme/demo/pulls/77"
                    }
                },
                "comment": {
                    "id": 500,
                    "body": "@forgebot please update this",
                    "user": { "id": 1, "login": "alice" }
                },
                "repository": {
                    "id": 9,
                    "full_name": "acme/demo"
                },
                "sender": { "id": 1, "login": "alice" }
            }"#,
        )
        .expect("payload should deserialize");

        assert_eq!(pr_number_from_issue_comment(&payload), Some(77));
    }

    #[test]
    fn test_pr_number_from_issue_comment_when_comment_is_on_issue() {
        let payload: IssueCommentPayload = serde_json::from_str(
            r#"{
                "action": "created",
                "issue": {
                    "number": 42,
                    "title": "Some issue",
                    "body": null,
                    "state": "open"
                },
                "comment": {
                    "id": 501,
                    "body": "@forgebot plan this",
                    "user": { "id": 2, "login": "bob" }
                },
                "repository": {
                    "id": 10,
                    "full_name": "acme/demo"
                },
                "sender": { "id": 2, "login": "bob" }
            }"#,
        )
        .expect("payload should deserialize");

        assert_eq!(pr_number_from_issue_comment(&payload), None);
    }
}
