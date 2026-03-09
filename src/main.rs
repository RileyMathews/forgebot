use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

mod config;
mod db;
pub mod forgejo;
mod session;
mod webhook;
mod ui;

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
    let config = config::Config::load()
        .context("Failed to load configuration")?;

    // Set up opencode config directory
    session::opencode::setup_opencode_config_dir(&config.opencode)
        .context("Failed to set up opencode config directory")?;

    info!(
        config_dir = %config.opencode.config_dir.display(),
        "Opencode config directory initialized"
    );

    info!(
        server_host = %config.server.host,
        server_port = %config.server.port,
        forgejo_url = %config.forgejo.url,
        bot_username = %config.forgejo.bot_username,
        database_path = %config.database.path.to_string_lossy(),
        worktree_base = %config.opencode.worktree_base.to_string_lossy(),
        opencode_binary = %config.opencode.binary,
        "Configuration loaded successfully"
    );

    // Initialize database
    let db_pool = db::init_db(&config.database)
        .await
        .context("Failed to initialize database")?;

    info!("Database initialized successfully");

    // Initialize Forgejo client
    let forgejo_client = forgejo::ForgejoClient::new(
        &config.forgejo.url,
        &config.forgejo.token,
        &config.forgejo.bot_username,
    ).context("Failed to create Forgejo client")?;

    info!(
        base_url = %config.forgejo.url,
        "Forgejo client initialized successfully"
    );

    // Run startup crash recovery before starting the server
    let recovery_result = session::opencode::startup_crash_recovery(&db_pool, &forgejo_client, &config)
        .await;
    
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
    let app_state = webhook::AppState::new(config.clone(), db_pool.clone(), forgejo_client.clone());

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
