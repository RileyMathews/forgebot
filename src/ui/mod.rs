pub mod handlers;

use axum::{
    routing::{get, post},
    Router,
};

use crate::webhook::AppState;

/// Create the UI router with all routes
/// Note: These routes are nested under /ui by the parent router
pub fn create_ui_router(state: AppState) -> Router {
    Router::new()
        // Dashboard (GET /ui)
        .route("/", get(handlers::dashboard))
        // Add repository (POST /ui/repos)
        .route("/repos", post(handlers::add_repo))
        // Per-repo setup page (GET /ui/repo/:owner/:name)
        .route("/repo/:owner/:name", get(handlers::repo_setup))
        // Webhook registration (POST /ui/repo/:owner/:name/webhook)
        .route(
            "/repo/:owner/:name/webhook",
            post(handlers::register_webhook),
        )
        // Environment loader configuration (POST /ui/repo/:owner/:name/env-loader)
        .route(
            "/repo/:owner/:name/env-loader",
            post(handlers::save_env_loader),
        )
        // Test environment (POST /ui/repo/:owner/:name/test-env)
        .route(
            "/repo/:owner/:name/test-env",
            post(handlers::test_env),
        )
        // Sessions list (GET /ui/sessions)
        .route("/sessions", get(handlers::sessions))
        .with_state(state)
}
