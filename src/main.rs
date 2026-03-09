use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod config;

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

    // For Phase 1, we just exit cleanly
    info!("forgebot scaffold initialized successfully. Exiting.");

    Ok(())
}
