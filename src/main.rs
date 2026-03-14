use forgebot::{config, db, forgejo, session, webhook};
use std::sync::Arc;
use tracing::{Level, error, info, warn};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> () {
    // Initialize tracing subscriber
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    info!("forgebot starting");

    // Load configuration from environment variables
    let config = config::Config::load().expect("Failed to load configuration");

    let askpass_path = config::askpass_script_path();
    config::setup_askpass_script(&askpass_path).expect("Failed to set up git askpass script");

    // Ensure worktree base directory exists
    tokio::fs::create_dir_all(&config.opencode.worktree_base)
        .await
        .expect("could not create opencode worktree dir");

    info!(
        worktree_base = %config.opencode.worktree_base.display(),
        askpass_path = %askpass_path.display(),
        "Worktree directory initialized"
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

    // Initialize database
    let db_pool = db::init_db(&config.database)
        .await
        .expect("Database should be able to initialize");

    info!("Database initialized successfully");

    // Crash recovery: reset any repos stuck in 'cloning' state
    let stuck_clone_recovery = db::recover_stuck_clones_after_restart(&db_pool)
        .await
        .expect("failed to recover stuck clones");

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
    );

    info!(
        base_url = %config.forgejo.url,
        "Forgejo client initialized successfully"
    );

    let authenticated_user = forgejo_client
        .get_authenticated_user()
        .await
        .expect("Failed to resolve authenticated Forgejo user");

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
        .expect("Webhook server failed");

    // Server has shut down (normally this only happens on error)
    info!("Webhook server stopped gracefully");

    // Close the database pool gracefully
    db_pool.close().await;
}
