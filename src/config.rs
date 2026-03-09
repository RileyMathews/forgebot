use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::path::PathBuf;
use tracing::debug;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub forgejo: ForgejoConfig,
    pub opencode: OpencodeConfig,
    pub database: DatabaseConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    #[serde(skip)]
    pub webhook_secret: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ForgejoConfig {
    pub url: String,
    #[serde(skip)]
    pub token: String,
    pub bot_username: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OpencodeConfig {
    pub binary: String,
    pub worktree_base: PathBuf,
    pub config_dir: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub path: PathBuf,
}

/// Intermediate struct for deserialization that handles the optional fields
#[derive(Debug, Deserialize)]
struct ConfigFile {
    server: ServerConfigFile,
    forgejo: ForgejoConfigFile,
    opencode: OpencodeConfigFile,
    database: DatabaseConfigFile,
}

#[derive(Debug, Deserialize)]
struct ServerConfigFile {
    host: Option<String>,
    port: Option<u16>,
    webhook_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ForgejoConfigFile {
    url: Option<String>,
    token: Option<String>,
    bot_username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpencodeConfigFile {
    binary: Option<String>,
    worktree_base: Option<PathBuf>,
    config_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct DatabaseConfigFile {
    path: Option<PathBuf>,
}

impl Config {
    /// Load configuration from file, with optional explicit path.
    /// Searches in order: explicit path, ./forgebot.toml, ~/.config/forgebot/forgebot.toml, /etc/forgebot/forgebot.toml
    /// Environment variables FORGEBOT_WEBHOOK_SECRET and FORGEBOT_FORGEJO_TOKEN override file values.
    pub fn load(explicit_path: Option<&Path>) -> Result<Self> {
        let config_path = find_config_file(explicit_path)
            .context("Could not find forgebot.toml configuration file")?;

        debug!("Loading configuration from: {}", config_path.display());

        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

        let file_config: ConfigFile = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;

        // Build the final config, applying env var overrides
        let config = Self::from_file_config(file_config, config_path)?;

        Ok(config)
    }

    fn from_file_config(file: ConfigFile, config_path: PathBuf) -> Result<Self> {
        // Load environment variable overrides
        let webhook_secret_env = std::env::var("FORGEBOT_WEBHOOK_SECRET").ok();
        let token_env = std::env::var("FORGEBOT_FORGEJO_TOKEN").ok();

        // Validate and construct server config
        let host = file.server.host.unwrap_or_else(|| "127.0.0.1".to_string());
        let port = file.server.port.unwrap_or(8080);
        let webhook_secret = webhook_secret_env
            .or(file.server.webhook_secret)
            .context("Missing required field: server.webhook_secret (set in config or FORGEBOT_WEBHOOK_SECRET env var)")?;

        // Validate and construct forgejo config
        let url = file
            .forgejo
            .url
            .context("Missing required field: forgejo.url")?;
        let token = token_env
            .or(file.forgejo.token)
            .context("Missing required field: forgejo.token (set in config or FORGEBOT_FORGEJO_TOKEN env var)")?;
        let bot_username = file
            .forgejo
            .bot_username
            .context("Missing required field: forgejo.bot_username")?;

        // Validate and construct opencode config
        let binary = file
            .opencode
            .binary
            .unwrap_or_else(|| "opencode".to_string());
        let worktree_base = file.opencode.worktree_base.unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("forgebot")
                .join("worktrees")
        });
        let config_dir = file.opencode.config_dir.unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("/etc"))
                .join("forgebot")
        });

        // Validate and construct database config
        let database_path = file.database.path.unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("/var/lib/forgebot"))
                .join("forgebot.db")
        });

        Ok(Config {
            server: ServerConfig {
                host,
                port,
                webhook_secret,
            },
            forgejo: ForgejoConfig {
                url,
                token,
                bot_username,
            },
            opencode: OpencodeConfig {
                binary,
                worktree_base,
                config_dir,
            },
            database: DatabaseConfig {
                path: database_path,
            },
        })
    }
}

/// Find the configuration file, searching in priority order.
fn find_config_file(explicit_path: Option<&Path>) -> Option<PathBuf> {
    // Priority 1: Explicit path from CLI
    if let Some(path) = explicit_path {
        if path.exists() {
            return Some(path.to_path_buf());
        }
        // If explicit path was given but doesn't exist, we should error
        // But we let the caller handle that
    }

    // Priority 2: ./forgebot.toml in current directory
    let local_path = Path::new("forgebot.toml");
    if local_path.exists() {
        return Some(local_path.to_path_buf());
    }

    // Priority 3: ~/.config/forgebot/forgebot.toml
    if let Some(home_dir) = dirs::home_dir() {
        let user_config = home_dir
            .join(".config")
            .join("forgebot")
            .join("forgebot.toml");
        if user_config.exists() {
            return Some(user_config);
        }
    }

    // Priority 4: /etc/forgebot/forgebot.toml
    let system_config = Path::new("/etc").join("forgebot").join("forgebot.toml");
    if system_config.exists() {
        return Some(system_config);
    }

    // If explicit path was provided and none found, return it anyway so the error message is clearer
    explicit_path.map(|p| p.to_path_buf())
}
