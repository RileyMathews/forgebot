pub mod errors;
pub mod handlers;

use axum::{
    Router,
    routing::{get, post},
};

use crate::webhook::AppState;

/// Create the UI router with all routes
/// Note: These routes are mounted at root by the parent router
pub fn create_ui_router(state: AppState) -> Router {
    Router::new()
        // Dashboard (GET /)
        .route("/", get(handlers::dashboard))
        // Add repository (POST /repos)
        .route("/repos", post(handlers::add_repo))
        // Per-repo setup page (GET /repo/:owner/:name)
        .route("/repo/:owner/:name", get(handlers::repo_setup))
        // Webhook registration (POST /repo/:owner/:name/webhook)
        .route(
            "/repo/:owner/:name/webhook",
            post(handlers::register_webhook),
        )
        // Environment loader configuration (POST /repo/:owner/:name/env-loader)
        .route(
            "/repo/:owner/:name/env-loader",
            post(handlers::save_env_loader),
        )
        // Retry clone (POST /repo/:owner/:name/retry-clone)
        .route(
            "/repo/:owner/:name/retry-clone",
            post(handlers::retry_clone),
        )
        // Remove repository (POST /repo/:owner/:name/remove)
        .route("/repo/:owner/:name/remove", post(handlers::remove_repo))
        .with_state(state)
}
