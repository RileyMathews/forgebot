use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

mod config;
mod db;
pub mod forgejo;

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

    // Phase 3: Uncomment the following to test the Forgejo API client
    // 
    // // Create Forgejo client using config
    // let client = forgejo::ForgejoClient::new(
    //     &config.forgejo.url,
    //     &config.forgejo.token,
    //     &config.forgejo.bot_username,
    // ).context("Failed to create Forgejo client")?;
    //
    // // Test: List webhooks for a repo (change "owner/repo" to your test repo)
    // match client.list_repo_webhooks("owner/repo").await {
    //     Ok(webhooks) => {
    //         info!("Found {} webhooks", webhooks.len());
    //         for webhook in &webhooks {
    //             info!("  Webhook {}: {} (active: {})", webhook.id, webhook.url, webhook.active);
    //         }
    //     }
    //     Err(e) => {
    //         error!("Failed to list webhooks: {}", e);
    //     }
    // }
    //
    // // Test: Check token permissions
    // match client.check_token_permissions("owner/repo").await {
    //     Ok(has_perms) => {
    //         info!("Token has permissions: {}", has_perms);
    //     }
    //     Err(e) => {
    //         error!("Failed to check permissions: {}", e);
    //     }
    // }

    // For Phase 2, verify database is working and exit cleanly
    info!("forgebot Phase 2 database setup complete. Exiting.");

    // Close the database pool gracefully
    db_pool.close().await;

    Ok(())
}
