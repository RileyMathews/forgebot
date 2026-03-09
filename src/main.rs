use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

mod config;
mod db;
pub mod forgejo;
mod session;
mod webhook;

#[derive(Parser, Debug)]
#[command(name = "forgebot")]
#[command(about = "A daemon that bridges Forgejo webhooks to opencode")]
#[command(version = "0.1.0")]
struct Cli {
    /// Path to the configuration file
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing subscriber
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set tracing subscriber")?;

    // Parse CLI arguments
    let cli = Cli::parse();

    info!("Starting forgebot daemon...");

    // Load configuration
    let config = config::Config::load(cli.config.as_deref())
        .context("Failed to load configuration")?;

    // Set up opencode config directory
    session::opencode::setup_opencode_config_dir(&config.opencode)
        .context("Failed to set up opencode config directory")?;

    info!("Opencode config directory initialized");

    info!(
        "Configuration loaded successfully (from {:?})",
        cli.config.as_deref().unwrap_or_else(|| std::path::Path::new("default location"))
    );
    info!("Server will listen on {}:{}", config.server.host, config.server.port);
    info!("Connected to Forgejo at {}", config.forgejo.url);
    info!("Bot username: {}", config.forgejo.bot_username);
    info!("Database path: {}", config.database.path.display());
    info!("Worktree base: {}", config.opencode.worktree_base.display());
    info!("Opencode binary: {}", config.opencode.binary);

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

    info!("Forgejo client initialized successfully");

    // Run startup crash recovery before starting the server
    session::opencode::startup_crash_recovery(&db_pool, &forgejo_client, &config)
        .await
        .context("Crash recovery failed (this is non-fatal, continuing startup)")
        .ok(); // Don't fail startup if crash recovery fails

    info!("Startup crash recovery complete");

    // Wrap config in Arc for sharing across handlers
    let config = Arc::new(config);

    // Start webhook server - this will block until the server shuts down
    info!("Starting webhook server...");
    
    // Note: In Phase 4, the server just listens forever
    // Phase 5+ will add background workers and graceful shutdown
    webhook::start_server(config)
        .await
        .context("Webhook server failed")?;

    // Server has shut down (normally this only happens on error in Phase 4)
    info!("Webhook server stopped");

    // Close the database pool gracefully
    db_pool.close().await;

    Ok(())
}
