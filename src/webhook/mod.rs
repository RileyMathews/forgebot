pub mod errors;
pub mod handlers;
pub mod models;

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    response::Response,
    routing::post,
};
use bytes::Bytes;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::db::DbPool;
use crate::forgejo::ForgejoClient;
use models::*;

/// Application state shared across all handlers
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: DbPool,
    pub forgejo: ForgejoClient,
}

impl AppState {
    pub fn new(config: Arc<Config>, db: DbPool, forgejo: ForgejoClient) -> Self {
        Self {
            config,
            db,
            forgejo,
        }
    }
}

/// HMAC-SHA256 verification middleware/extractor
pub struct WebhookVerifier {
    pub secret: String,
}

impl WebhookVerifier {
    pub fn new(secret: String) -> Self {
        Self { secret }
    }

    /// Compute HMAC-SHA256 signature for request body
    pub fn compute_signature(&self, body: &[u8]) -> String {
        type HmacSha256 = Hmac<Sha256>;
        // HMAC-SHA256 can accept keys of any size, so this expect is safe
        let mut mac = HmacSha256::new_from_slice(self.secret.as_bytes())
            .expect("HMAC-SHA256 accepts keys of any size; this cannot fail");
        mac.update(body);
        let result = mac.finalize();
        let bytes = result.into_bytes();
        format!("sha256={}", hex::encode(bytes))
    }

    /// Verify signature from header
    pub fn verify_signature(&self, body: &[u8], signature_header: &str) -> bool {
        let expected = self.compute_signature(body);
        // Handle both "sha256=..." and raw hex formats (Forgejo sends raw)
        let signature = if signature_header.starts_with("sha256=") {
            signature_header.to_string()
        } else {
            format!("sha256={}", signature_header)
        };
        // Constant-time comparison to prevent timing attacks
        if expected.len() != signature.len() {
            return false;
        }
        let mut result = 0u8;
        for (a, b) in expected.bytes().zip(signature.bytes()) {
            result |= a ^ b;
        }
        result == 0
    }
}

/// Extract raw body and verify signature
pub async fn extract_and_verify_body(
    request: Request,
    verifier: &WebhookVerifier,
) -> Result<(Bytes, String), Response> {
    // Extract headers first before consuming the request
    let signature_header = request.headers().get("X-Gitea-Signature").cloned();

    let event_type_header = request
        .headers()
        .get("X-Gitea-Event")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    // Now consume the request to get the body
    let (_parts, body) = request.into_parts();

    // Get the signature value
    let signature = match signature_header {
        Some(h) => match h.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => {
                warn!("Invalid X-Gitea-Signature header encoding");
                // Response::builder() with standard strings cannot fail; unwrap is safe
                return Err(Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body("Invalid signature header encoding".into())
                    .unwrap());
            }
        },
        None => {
            warn!("Missing X-Gitea-Signature header");
            // Response::builder() with standard strings cannot fail; unwrap is safe
            return Err(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body("Missing signature header".into())
                .unwrap());
        }
    };

    // Get the event type
    let event_type = event_type_header.unwrap_or_else(|| "unknown".to_string());

    // Extract raw body bytes
    let bytes = axum::body::to_bytes(body, usize::MAX).await.map_err(|e| {
        error!("Failed to read request body: {}", e);
        // Response::builder() with standard strings cannot fail; unwrap is safe
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body("Failed to read body".into())
            .unwrap()
    })?;

    // Verify signature
    if !verifier.verify_signature(&bytes, &signature) {
        warn!(
            signature_valid = false,
            "Webhook signature verification failed"
        );
        // Response::builder() with standard strings cannot fail; unwrap is safe
        return Err(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body("Invalid signature".into())
            .unwrap());
    }

    debug!(
        event_type = %event_type,
        signature_valid = true,
        "Webhook signature verified successfully"
    );
    Ok((bytes, event_type))
}

/// Handler for POST /webhook
async fn webhook_handler(State(state): State<AppState>, request: Request) -> Response {
    // Create verifier
    let verifier = WebhookVerifier::new(state.config.server.webhook_secret.clone());

    // Verify signature and get body
    let (body, event_type) = match extract_and_verify_body(request, &verifier).await {
        Ok(result) => result,
        Err(response) => return response,
    };

    info!(event_type = %event_type, "Received webhook event");

    // Dispatch based on event type
    match event_type.as_str() {
        "issue_comment" => {
            let payload: IssueCommentPayload = match serde_json::from_slice(&body) {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to parse issue_comment payload: {}", e);
                    // Response::builder() with standard strings cannot fail; unwrap is safe
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(format!("Invalid JSON: {}", e).into())
                        .unwrap();
                }
            };
            match handlers::handle_issue_comment(payload, &state.db, &state.forgejo, &state.config)
                .await
            {
                Ok(response) => response,
                Err(response) => response,
            }
        }
        "pull_request" => {
            let payload: PullRequestPayload = match serde_json::from_slice(&body) {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to parse pull_request payload: {}", e);
                    // Response::builder() with standard strings cannot fail; unwrap is safe
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(format!("Invalid JSON: {}", e).into())
                        .unwrap();
                }
            };
            match handlers::handle_pull_request(payload, &state.db, &state.forgejo, &state.config)
                .await
            {
                Ok(response) => response,
                Err(response) => response,
            }
        }
        "pull_request_review_comment" => {
            let payload: PullRequestReviewCommentPayload = match serde_json::from_slice(&body) {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to parse pull_request_review_comment payload: {}", e);
                    // Response::builder() with standard strings cannot fail; unwrap is safe
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(format!("Invalid JSON: {}", e).into())
                        .unwrap();
                }
            };
            match handlers::handle_pull_request_review_comment(
                payload,
                &state.db,
                &state.forgejo,
                &state.config,
            )
            .await
            {
                Ok(response) => response,
                Err(response) => response,
            }
        }
        _ => {
            // Unknown event type, return 200 to avoid retries
            warn!("Unknown webhook event type: {}", event_type);
            // Response::builder() with standard strings cannot fail; unwrap is safe
            handlers::handle_unknown_event(&event_type)
                .await
                .unwrap_or_else(|e| e)
        }
    }
}

/// Create the webhook router
pub fn create_webhook_router(state: AppState) -> Router {
    Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(state)
}

/// Create the combined app router (webhook + UI)
pub fn create_app_router(state: AppState) -> Router {
    // Create the UI router and merge it at root level
    let ui_router = crate::ui::create_ui_router(state.clone());

    // Combine webhook and UI routers at root level
    create_webhook_router(state).merge(ui_router)
}

/// Start the webhook server with UI routes
pub async fn start_server(state: AppState) -> Result<()> {
    let host = state.config.server.host.clone();
    let port = state.config.server.port;

    let app = create_app_router(state);

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", host, port))
        .await
        .with_context(|| format!("Failed to bind to {}:{}", host, port))?;

    info!("Webhook server listening on {}:{}", host, port);
    info!("UI available at http://{}:{}/", host, port);

    axum::serve(listener, app).await.context("Server error")?;

    Ok(())
}
