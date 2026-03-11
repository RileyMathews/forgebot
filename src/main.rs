use anyhow::{Context, Result};
use forgebot::{config, db, forgejo, session, webhook};
use std::sync::Arc;
use tracing::{Level, error, info, warn};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing subscriber
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set tracing subscriber")?;

    info!("forgebot starting");

    // Load configuration from environment variables
    let config = config::Config::load().context("Failed to load configuration")?;

    // Set up opencode config directory
    session::opencode::setup_opencode_config_dir(&config.opencode)
        .context("Failed to set up opencode config directory")?;

    // Ensure worktree base directory exists
    tokio::fs::create_dir_all(&config.opencode.worktree_base)
        .await
        .with_context(|| {
            format!(
                "Failed to create worktree base directory: {}",
                config.opencode.worktree_base.display()
            )
        })?;

    info!(
        config_dir = %config.opencode.config_dir.display(),
        worktree_base = %config.opencode.worktree_base.display(),
        "Opencode config and worktree directories initialized"
    );

    info!(
        server_host = %config.server.host,
        server_port = %config.server.port,
        forgejo_url = %config.forgejo.url,
        bot_username = %config.forgejo.bot_username,
        database_path = %config.database.path.to_string_lossy(),
        worktree_base = %config.opencode.worktree_base.to_string_lossy(),
        opencode_binary = %config.opencode.binary,
        opencode_api_base_url = %config
            .opencode
            .api
            .base_url
            .as_deref()
            .unwrap_or("[not set]"),
        "Configuration loaded successfully"
    );

    let api_client = session::opencode_api::OpencodeApiClient::from_config(&config.opencode.api)
        .context("Failed to initialize OpenCode API client")?;
    let health = api_client
        .health()
        .await
        .context("Failed OpenCode API startup health check")?;
    if !health.healthy {
        anyhow::bail!(
            "OpenCode API health check returned unhealthy status (version={})",
            health.version
        );
    }

    info!(
        opencode_api_version = %health.version,
        "OpenCode API startup health check passed"
    );

    // Initialize database
    let db_pool = db::init_db(&config.database)
        .await
        .context("Failed to initialize database")?;

    info!("Database initialized successfully");

    // Crash recovery: reset any repos stuck in 'cloning' state
    let stuck_clone_recovery = db::recover_stuck_clones_after_restart(&db_pool)
        .await
        .context("failed to recover stuck clones")?;

    for full_name in stuck_clone_recovery.recovered_repos {
        info!(full_name = %full_name, "Reset stuck clone to failed state");
    }

    for (full_name, err_message) in stuck_clone_recovery.failed_repos {
        error!(
            full_name = %full_name,
            err = %err_message,
            "Failed to reset stuck clone (continuing startup)"
        );
    }

    // Initialize Forgejo client
    let forgejo_client = forgejo::ForgejoClient::new(
        &config.forgejo.url,
        &config.forgejo.token,
        &config.forgejo.bot_username,
    )
    .context("Failed to create Forgejo client")?;

    info!(
        base_url = %config.forgejo.url,
        "Forgejo client initialized successfully"
    );

    let authenticated_user = forgejo_client
        .get_authenticated_user()
        .await
        .context("Failed to resolve authenticated Forgejo user")?;

    if authenticated_user.login != config.forgejo.bot_username {
        warn!(
            configured_bot_username = %config.forgejo.bot_username,
            authenticated_login = %authenticated_user.login,
            authenticated_user_id = %authenticated_user.id,
            "Configured FORGEBOT_FORGEJO_BOT_USERNAME does not match token identity"
        );
    }

    info!(
        authenticated_login = %authenticated_user.login,
        authenticated_user_id = %authenticated_user.id,
        "Resolved authenticated Forgejo user for webhook loop prevention"
    );

    // Run startup crash recovery before starting the server
    let recovery_result =
        session::opencode::startup_crash_recovery(&db_pool, &forgejo_client, &config).await;

    match recovery_result {
        Ok(count) => {
            info!(
                recovered_sessions = %count,
                "Startup crash recovery complete"
            );
        }
        Err(e) => {
            warn!(
                error = %e,
                "Crash recovery encountered an error (non-fatal, continuing startup)"
            );
        }
    }

    // Create shared application state
    let config = Arc::new(config);
    let app_state = webhook::AppState::new(
        config.clone(),
        db_pool.clone(),
        forgejo_client.clone(),
        authenticated_user.id,
        authenticated_user.login,
    );

    // Start webhook server - this will block until the server shuts down
    info!(
        host = %config.server.host,
        port = %config.server.port,
        "Starting webhook server"
    );

    webhook::start_server(app_state)
        .await
        .context("Webhook server failed")?;

    // Server has shut down (normally this only happens on error)
    info!("Webhook server stopped gracefully");

    // Close the database pool gracefully
    db_pool.close().await;

    Ok(())
}
