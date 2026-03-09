use axum::response::Response;
use tracing::{info, warn};

use super::models::*;

/// Handle issue_comment webhook events
pub async fn handle_issue_comment(payload: IssueCommentPayload) -> Result<Response, axum::response::Response> {
    info!(
        "Webhook received: issue_comment - repo={}, issue={}, action={}, sender={}",
        payload.repository.full_name,
        payload.issue.number,
        payload.action,
        payload.sender.login
    );

    // Phase 4: Stub - just return 200 OK immediately
    // Phase 5+: Implement actual processing logic here
    Ok(Response::builder()
        .status(200)
        .body("OK".into())
        .unwrap())
}

/// Handle pull_request webhook events
pub async fn handle_pull_request(payload: PullRequestPayload) -> Result<Response, axum::response::Response> {
    info!(
        "Webhook received: pull_request - repo={}, pr={}, action={}, sender={}",
        payload.repository.full_name,
        payload.pull_request.number,
        payload.action,
        payload.sender.login
    );

    // Phase 4: Stub - just return 200 OK immediately
    // Phase 5+: Implement actual processing logic here
    Ok(Response::builder()
        .status(200)
        .body("OK".into())
        .unwrap())
}

/// Handle pull_request_review_comment webhook events
pub async fn handle_pull_request_review_comment(
    payload: PullRequestReviewCommentPayload,
) -> Result<Response, axum::response::Response> {
    info!(
        "Webhook received: pull_request_review_comment - repo={}, pr={}, comment_path={}, sender={}",
        payload.repository.full_name,
        payload.pull_request.number,
        payload.review_comment.path,
        payload.sender.login
    );

    // Phase 4: Stub - just return 200 OK immediately
    // Phase 5+: Implement actual processing logic here
    Ok(Response::builder()
        .status(200)
        .body("OK".into())
        .unwrap())
}

/// Handle unknown webhook events
pub async fn handle_unknown_event(event_type: &str) -> Result<Response, axum::response::Response> {
    warn!(
        "Webhook received: unknown event type '{}', ignoring",
        event_type
    );

    // Return 200 OK - we don't want Forgejo to retry unknown events
    Ok(Response::builder()
        .status(200)
        .body("OK".into())
        .unwrap())
}
