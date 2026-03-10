use askama::Template;
use axum::{
    extract::{Form, Path as AxumPath, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::db::{
    get_repo_by_full_name, list_repos, reset_clone_status_if_failed, update_repo_env_loader,
    validate_repo_full_name,
};
use crate::forgejo::ForgejoClient;
use crate::session::repo_cleanup;
use crate::webhook::AppState;

// ============================================================================
// Templates
// ============================================================================

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    repos: Vec<RepoWithStatus>,
    webhook_url: String,
}

#[derive(Template)]
#[template(path = "repo_setup.html")]
struct RepoSetupTemplate {
    full_name: String,
    owner: String,
    name: String,
    default_branch: String,
    env_loader: String,
    clone_status: String,
    webhook_registered: bool,
    webhook_url: String,
    webhook_secret: String,
    token_valid: bool,
    opencode_exists: bool,
    opencode_path: String,
    config_files_exist: bool,
    message: Option<String>,
    success: bool,
}

// ============================================================================
// Helper Structs
// ============================================================================

/// Repository with computed status information
struct RepoWithStatus {
    full_name: String,
    owner: String,
    name: String,
    default_branch: String,
    clone_status: String,
    webhook_registered: bool,
}

/// Form data for adding a new repository
#[derive(Deserialize)]
pub struct AddRepoForm {
    full_name: String,
    default_branch: String,
    env_loader: String,
}

/// Form data for updating environment loader
#[derive(Deserialize)]
pub struct EnvLoaderForm {
    env_loader: String,
}

// ============================================================================
// Route Handlers
// ============================================================================

/// GET / - Dashboard showing all repos
pub async fn dashboard(State(state): State<AppState>) -> impl IntoResponse {
    // Get all repos from database
    let repos = match list_repos(&state.db).await {
        Ok(repos) => repos,
        Err(e) => {
            error!("Failed to list repos: {}", e);
            return internal_error_response(format!("Failed to list repos: {}", e));
        }
    };

    // Enrich with status information
    let mut repos_with_status = Vec::new();
    for repo in repos {
        let owner_name: Vec<&str> = repo.full_name.split('/').collect();
        let owner = owner_name.first().unwrap_or(&"").to_string();
        let name = owner_name.get(1).unwrap_or(&"").to_string();

        // Check webhook status
        let webhook_registered =
            check_webhook_status(&state.forgejo, &repo.full_name, &state.config).await;

        repos_with_status.push(RepoWithStatus {
            full_name: repo.full_name.clone(),
            owner,
            name,
            default_branch: repo.default_branch,
            clone_status: repo.clone_status,
            webhook_registered,
        });
    }

    // Build webhook URL
    let webhook_url = format_webhook_url(&state.config);

    let template = DashboardTemplate {
        repos: repos_with_status,
        webhook_url,
    };

    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => internal_error_response(format!("Template error: {}", e)),
    }
}

/// POST /repos - Add a new repository
pub async fn add_repo(
    State(state): State<AppState>,
    Form(form): Form<AddRepoForm>,
) -> impl IntoResponse {
    // Validate the full_name format
    if let Err(e) = validate_repo_full_name(&form.full_name) {
        warn!(full_name = %form.full_name, error = %e, "Invalid repo full name format");
        return Redirect::to("/").into_response();
    }

    // Validate env_loader value
    let env_loader = match form.env_loader.as_str() {
        "nix" | "direnv" | "none" => form.env_loader.clone(),
        _ => "none".to_string(),
    };

    // Generate a UUID for the repo
    let repo_id = uuid::Uuid::new_v4().to_string();

    // Insert into database
    if let Err(e) = crate::db::insert_repo(
        &state.db,
        &repo_id,
        &form.full_name,
        &form.default_branch,
        &env_loader,
    )
    .await
    {
        error!("Failed to insert repo: {}", e);
        return Redirect::to("/").into_response();
    }

    // Spawn background clone task
    let db_clone = state.db.clone();
    let config_clone = state.config.clone();
    let full_name_clone = form.full_name.clone();

    tokio::spawn(async move {
        if let Err(e) =
            crate::session::clone::perform_clone(&db_clone, &config_clone, &full_name_clone).await
        {
            error!(err = %e, full_name = %full_name_clone, "Clone task failed");
        }
    });

    // Parse owner and name for redirect
    let parts: Vec<&str> = form.full_name.split('/').collect();
    if parts.len() == 2 {
        let owner = parts[0];
        let name = parts[1];
        Redirect::to(&format!("/repo/{}/{}", owner, name)).into_response()
    } else {
        Redirect::to("/").into_response()
    }
}

/// GET /repo/:owner/:name - Per-repo setup page
pub async fn repo_setup(
    State(state): State<AppState>,
    AxumPath((owner, name)): AxumPath<(String, String)>,
) -> impl IntoResponse {
    let full_name = format!("{}/{}", owner, name);

    // Get repo from database
    let repo = match get_repo_by_full_name(&state.db, &full_name).await {
        Ok(Some(repo)) => repo,
        Ok(None) => {
            return Redirect::to("/").into_response();
        }
        Err(e) => {
            error!("Failed to get repo: {}", e);
            return internal_error_response(format!("Failed to get repo: {}", e));
        }
    };

    // Build webhook URL and secret
    let webhook_url = format_webhook_url(&state.config);
    let webhook_secret = state.config.server.webhook_secret.clone();

    // Check webhook registration status
    let webhook_registered = check_webhook_status(&state.forgejo, &full_name, &state.config).await;

    // Verify token permissions
    let token_valid = state
        .forgejo
        .check_token_permissions(&full_name)
        .await
        .unwrap_or(false);

    // Check opencode binary
    let opencode_path = &state.config.opencode.binary;
    let opencode_exists = which::which(opencode_path).is_ok() || Path::new(opencode_path).exists();

    // Check config files (basic check for config dir existence)
    let config_files_exist = state.config.opencode.config_dir.exists();

    let template = RepoSetupTemplate {
        full_name: full_name.clone(),
        owner: owner.clone(),
        name: name.clone(),
        default_branch: repo.default_branch,
        env_loader: repo.env_loader,
        clone_status: repo.clone_status,
        webhook_registered,
        webhook_url,
        webhook_secret,
        token_valid,
        opencode_exists,
        opencode_path: opencode_path.clone(),
        config_files_exist,
        message: None,
        success: true,
    };

    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => internal_error_response(format!("Template error: {}", e)),
    }
}

/// POST /repo/:owner/:name/webhook - Register webhook
pub async fn register_webhook(
    State(state): State<AppState>,
    AxumPath((owner, name)): AxumPath<(String, String)>,
) -> impl IntoResponse {
    let full_name = format!("{}/{}", owner, name);

    // Fetch current repo state
    let repo = match get_repo_by_full_name(&state.db, &full_name).await {
        Ok(Some(repo)) => repo,
        _ => return Redirect::to("/").into_response(),
    };

    // Validate clone is ready before allowing webhook registration
    if repo.clone_status != "ready" {
        info!(repo = %full_name, clone_status = %repo.clone_status, "Webhook registration attempted before clone ready");
        return Redirect::to(&format!("/repo/{}/{}", owner, name)).into_response();
    }

    // Build webhook URL
    let webhook_url = format_webhook_url(&state.config);

    // Create webhook
    let result = state
        .forgejo
        .create_repo_webhook(
            &full_name,
            &webhook_url,
            &state.config.server.webhook_secret,
        )
        .await;

    let (message, success) = match result {
        Ok(_) => ("Webhook registered successfully".to_string(), true),
        Err(e) => {
            error!("Failed to create webhook: {}", e);
            (format!("Failed to create webhook: {}", e), false)
        }
    };

    // Re-render the setup page with the message
    render_repo_setup_with_message(state, owner, name, message, success).await
}

/// POST /repo/:owner/:name/env-loader - Update environment loader
pub async fn save_env_loader(
    State(state): State<AppState>,
    AxumPath((owner, name)): AxumPath<(String, String)>,
    Form(form): Form<EnvLoaderForm>,
) -> impl IntoResponse {
    let full_name = format!("{}/{}", owner, name);

    // Validate env_loader value
    let env_loader = match form.env_loader.as_str() {
        "nix" | "direnv" | "none" => form.env_loader.clone(),
        _ => "none".to_string(),
    };

    // Update in database
    let (message, success) = match update_repo_env_loader(&state.db, &full_name, &env_loader).await
    {
        Ok(_) => ("Environment loader updated".to_string(), true),
        Err(e) => {
            error!("Failed to update env_loader: {}", e);
            (format!("Failed to update: {}", e), false)
        }
    };

    // Re-render the setup page with the message
    render_repo_setup_with_message(state, owner, name, message, success).await
}

/// POST /repo/:owner/:name/retry-clone - Retry a failed or pending clone
pub async fn retry_clone(
    State(state): State<AppState>,
    AxumPath((owner, name)): AxumPath<(String, String)>,
) -> impl IntoResponse {
    let full_name = format!("{}/{}", owner, name);

    // Validate the full_name format as a safety check
    if let Err(e) = validate_repo_full_name(&full_name) {
        warn!(full_name = %full_name, error = %e, "Invalid repo full name in retry");
        return Redirect::to("/").into_response();
    }

    // Fetch the repo
    let repo = match get_repo_by_full_name(&state.db, &full_name).await {
        Ok(Some(repo)) => repo,
        Ok(None) => {
            return Redirect::to("/").into_response();
        }
        Err(e) => {
            error!("Failed to get repo: {}", e);
            return internal_error_response(format!("Failed to get repo: {}", e));
        }
    };

    // If clone_status is "cloning" or "ready", can't retry
    if repo.clone_status == "cloning" || repo.clone_status == "ready" {
        return Redirect::to(&format!("/repo/{}/{}", owner, name)).into_response();
    }

    // Atomically reset to "pending" state - only succeeds if still failed/pending
    match reset_clone_status_if_failed(&state.db, &full_name).await {
        Ok(true) => {
            // Successfully transitioned to pending - spawn clone task
            let db_clone = state.db.clone();
            let config_clone = state.config.clone();
            let full_name_clone = full_name.clone();

            tokio::spawn(async move {
                if let Err(e) =
                    crate::session::clone::perform_clone(&db_clone, &config_clone, &full_name_clone)
                        .await
                {
                    error!(err = %e, full_name = %full_name_clone, "Retry clone task failed");
                }
            });
        }
        Ok(false) => {
            // No rows updated - status changed or another retry is in progress
            // Just redirect without spawning a new clone task
            info!(
                full_name = %full_name,
                "Retry clone skipped - status changed or concurrent retry in progress"
            );
        }
        Err(e) => {
            error!("Failed to reset clone status: {}", e);
            return internal_error_response(format!("Failed to reset clone status: {}", e));
        }
    }

    // Redirect back to repo setup page
    Redirect::to(&format!("/repo/{}/{}", owner, name)).into_response()
}

/// POST /repo/:owner/:name/remove - Remove a repository
pub async fn remove_repo(
    State(state): State<AppState>,
    AxumPath((owner, name)): AxumPath<(String, String)>,
) -> impl IntoResponse {
    let full_name = format!("{}/{}", owner, name);

    // Spawn cleanup task (active session check moved inside remove_repository)
    let db = state.db.clone();
    let forgejo = state.forgejo.clone();
    let config = state.config.clone();
    let full_name_clone = full_name.clone();

    tokio::spawn(async move {
        let result =
            repo_cleanup::remove_repository(&db, &forgejo, &config, &full_name_clone).await;
        match result {
            Ok(()) => {
                info!(repo = %full_name_clone, "Successfully removed repository");
            }
            Err(e) => {
                error!(repo = %full_name_clone, err = %e, "Failed to remove repository");
            }
        }
    });

    // Return immediately with a 303 See Other redirect to /
    Redirect::to("/").into_response()
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Format the webhook URL from config
fn format_webhook_url(config: &Arc<crate::config::Config>) -> String {
    format!("{}/webhook", config.server.forgebot_host)
}

/// Check if webhook is registered for a repo
async fn check_webhook_status(
    forgejo: &ForgejoClient,
    full_name: &str,
    config: &Arc<crate::config::Config>,
) -> bool {
    let expected_url = format_webhook_url(config);

    match forgejo.list_repo_webhooks(full_name).await {
        Ok(webhooks) => webhooks.iter().any(|w| w.url == expected_url && w.active),
        Err(e) => {
            warn!("Failed to list webhooks for {}: {}", full_name, e);
            false
        }
    }
}

/// Render the repo setup page with a status message
async fn render_repo_setup_with_message(
    state: AppState,
    owner: String,
    name: String,
    message: String,
    success: bool,
) -> Response {
    let full_name = format!("{}/{}", owner, name);

    // Get repo from database
    let repo = match get_repo_by_full_name(&state.db, &full_name).await {
        Ok(Some(repo)) => repo,
        Ok(None) => {
            return Redirect::to("/").into_response();
        }
        Err(e) => {
            return internal_error_response(format!("Failed to get repo: {}", e));
        }
    };

    // Build webhook URL and secret
    let webhook_url = format_webhook_url(&state.config);
    let webhook_secret = state.config.server.webhook_secret.clone();

    // Check webhook registration status
    let webhook_registered = check_webhook_status(&state.forgejo, &full_name, &state.config).await;

    // Verify token permissions
    let token_valid = state
        .forgejo
        .check_token_permissions(&full_name)
        .await
        .unwrap_or(false);

    // Check opencode binary
    let opencode_path = &state.config.opencode.binary;
    let opencode_exists = which::which(opencode_path).is_ok() || Path::new(opencode_path).exists();

    // Check config files
    let config_files_exist = state.config.opencode.config_dir.exists();

    let template = RepoSetupTemplate {
        full_name,
        owner,
        name,
        default_branch: repo.default_branch,
        env_loader: repo.env_loader,
        clone_status: repo.clone_status,
        webhook_registered,
        webhook_url,
        webhook_secret,
        token_valid,
        opencode_exists,
        opencode_path: opencode_path.clone(),
        config_files_exist,
        message: Some(message),
        success,
    };

    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => internal_error_response(format!("Template error: {}", e)),
    }
}

/// Create an internal error response
fn internal_error_response(message: String) -> Response {
    // Response::builder() with standard strings cannot fail; unwrap is safe (last-resort error response)
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(
            format!(
                r#"<!DOCTYPE html>
<html><body>
<h1>Internal Server Error</h1>
<p>{}</p>
<p><a href="/">Return to Dashboard</a></p>
</body></html>"#,
                message
            )
            .into(),
        )
        .unwrap()
}
