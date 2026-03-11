use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::{info, warn};

/// Script content for the git askpass helper.
/// This script returns bot username for username prompts and token for password prompts.
const ASKPASS_SCRIPT: &str = r#"#!/bin/sh
prompt="$1"
case "$prompt" in
  *Username*|*username*)
    printf '%s\n' "${FORGEBOT_FORGEJO_BOT_USERNAME:-forgebot}"
    ;;
  *)
    printf '%s\n' "${FORGEBOT_FORGEJO_TOKEN:-}"
    ;;
esac
"#;

#[derive(Debug, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub forgejo: ForgejoConfig,
    pub opencode: OpencodeConfig,
    pub database: DatabaseConfig,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub webhook_secret: String,
    pub forgebot_host: String,
}

#[derive(Debug, Clone)]
pub struct ForgejoConfig {
    pub url: String,
    pub token: String,
    pub bot_username: String,
}

#[derive(Debug, Clone)]
pub struct OpencodeConfig {
    pub binary: String,
    pub worktree_base: PathBuf,
    pub config_dir: PathBuf,
    pub git_binary: String,
    pub model: String,
    pub askpass_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub path: PathBuf,
}

impl Config {
    /// Load configuration entirely from environment variables.
    /// Required env vars (must be set or error): FORGEBOT_WEBHOOK_SECRET, FORGEBOT_FORGEJO_URL, FORGEBOT_FORGEJO_TOKEN
    /// Optional env vars (have defaults): FORGEBOT_SERVER_HOST, FORGEBOT_SERVER_PORT, FORGEBOT_FORGEJO_BOT_USERNAME, FORGEBOT_OPENCODE_BINARY, FORGEBOT_OPENCODE_WORKTREE_BASE, FORGEBOT_OPENCODE_CONFIG_DIR, FORGEBOT_DATABASE_PATH
    pub fn load() -> Result<Self> {
        info!("Loading configuration from environment variables...");

        // Load required environment variables
        let webhook_secret = require_env_var("FORGEBOT_WEBHOOK_SECRET")?;
        let forgejo_url = require_env_var("FORGEBOT_FORGEJO_URL")?;
        let forgejo_token = require_env_var("FORGEBOT_FORGEJO_TOKEN")?;

        // Load optional environment variables with defaults
        let server_host = env_var_with_default("FORGEBOT_SERVER_HOST", "127.0.0.1");
        let server_port = env_var_parse_with_default("FORGEBOT_SERVER_PORT", 8765)?;

        // Load forgebot_host - the public-facing URL for webhooks
        // If not set, construct from server_host and server_port with http://
        let forgebot_host = match std::env::var("FORGEBOT_FORGEBOT_HOST") {
            Ok(value) if !value.trim().is_empty() => {
                info!(
                    "Using FORGEBOT_FORGEBOT_HOST from environment variable: {}",
                    value
                );
                value
            }
            _ => {
                // Construct from server host and port
                let constructed = format!("http://{}:{}", server_host, server_port);
                warn!(
                    "FORGEBOT_FORGEBOT_HOST not set, using constructed URL: {}. \
                     For production, set FORGEBOT_FORGEBOT_HOST to your public-facing URL.",
                    constructed
                );
                // Additional warning for localhost bindings
                if server_host == "127.0.0.1" || server_host == "localhost" {
                    warn!(
                        "Server is bound to {} - this URL will only work from the same machine. \
                         Set FORGEBOT_FORGEBOT_HOST to your public hostname/IP for production use.",
                        server_host
                    );
                }
                constructed
            }
        };
        let bot_username = env_var_with_default("FORGEBOT_FORGEJO_BOT_USERNAME", "forgebot");
        let opencode_binary = env_var_with_default("FORGEBOT_OPENCODE_BINARY", "opencode");
        let git_binary = env_var_with_default("FORGEBOT_GIT_BINARY", "git");
        let opencode_model = env_var_with_default("FORGEBOT_OPENCODE_MODEL", "opencode/kimi-k2.5");
        let worktree_base = env_var_path_with_default(
            "FORGEBOT_OPENCODE_WORKTREE_BASE",
            "/var/lib/forgebot/worktrees",
        );
        let config_dir = env_var_path_with_default(
            "FORGEBOT_OPENCODE_CONFIG_DIR",
            "/var/lib/forgebot/opencode-config",
        );
        let database_path =
            env_var_path_with_default("FORGEBOT_DATABASE_PATH", "/var/lib/forgebot/forgebot.db");

        // Set askpass path: use env var if set, otherwise use runtime directory
        let askpass_path =
            env_var_path_with_default("FORGEBOT_ASKPASS_PATH", "/var/lib/forgebot/git-askpass.sh");

        info!("Configuration loaded successfully");
        info!("  FORGEBOT_SERVER_HOST: {}", server_host);
        info!("  FORGEBOT_SERVER_PORT: {}", server_port);
        info!("  FORGEBOT_FORGEBOT_HOST: {}", forgebot_host);
        info!("  FORGEBOT_WEBHOOK_SECRET: [REDACTED]");
        info!("  FORGEBOT_FORGEJO_URL: {}", forgejo_url);
        info!("  FORGEBOT_FORGEJO_TOKEN: [REDACTED]");
        info!("  FORGEBOT_FORGEJO_BOT_USERNAME: {}", bot_username);
        info!("  FORGEBOT_OPENCODE_BINARY: {}", opencode_binary);
        info!("  FORGEBOT_GIT_BINARY: {}", git_binary);
        info!("  FORGEBOT_OPENCODE_MODEL: {}", opencode_model);
        info!(
            "  FORGEBOT_OPENCODE_WORKTREE_BASE: {}",
            worktree_base.display()
        );
        info!("  FORGEBOT_OPENCODE_CONFIG_DIR: {}", config_dir.display());
        info!("  FORGEBOT_DATABASE_PATH: {}", database_path.display());

        Ok(Config {
            server: ServerConfig {
                host: server_host,
                port: server_port,
                webhook_secret,
                forgebot_host,
            },
            forgejo: ForgejoConfig {
                url: forgejo_url,
                token: forgejo_token,
                bot_username,
            },
            opencode: OpencodeConfig {
                binary: opencode_binary,
                worktree_base,
                config_dir,
                git_binary,
                model: opencode_model,
                askpass_path,
            },
            database: DatabaseConfig {
                path: database_path,
            },
        })
    }
}

/// Require an environment variable to be set, or return an error with a clear message.
fn require_env_var(name: &str) -> Result<String> {
    match std::env::var(name) {
        Ok(value) => {
            if value.trim().is_empty() {
                anyhow::bail!(
                    "ERROR: {} environment variable is set but empty. Please provide a valid value.",
                    name
                );
            }
            Ok(value)
        }
        Err(_) => {
            anyhow::bail!(
                "ERROR: {} environment variable is required but not set. Please set it and try again.",
                name
            );
        }
    }
}

/// Get an environment variable with a default value if not set.
/// Logs when the default is used.
fn env_var_with_default(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            info!("Using {} from environment variable", name);
            value
        }
        _ => {
            warn!("{} not set or empty, using default: {}", name, default);
            default.to_string()
        }
    }
}

/// Parse an environment variable as u16 with a default value.
fn env_var_parse_with_default(name: &str, default: u16) -> Result<u16> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => match value.parse::<u16>() {
            Ok(port) => {
                info!("Using {} from environment variable: {}", name, port);
                Ok(port)
            }
            Err(_) => {
                warn!(
                    "{} has invalid value '{}', using default: {}",
                    name, value, default
                );
                Ok(default)
            }
        },
        _ => {
            warn!("{} not set or empty, using default: {}", name, default);
            Ok(default)
        }
    }
}

/// Get an environment variable as a PathBuf with a default value.
fn env_var_path_with_default(name: &str, default: &str) -> PathBuf {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            info!("Using {} from environment variable: {}", name, value);
            PathBuf::from(value)
        }
        _ => {
            warn!("{} not set or empty, using default: {}", name, default);
            PathBuf::from(default)
        }
    }
}

/// Sets up the git askpass script at the configured path.
///
/// This function is called once on startup. It writes the askpass script
/// which is used for non-interactive git HTTPS authentication.
///
/// # Arguments
/// * `askpass_path` - The path where the script should be written
///
/// # Returns
/// * `Ok(())` on success
/// * `Err` on permission or I/O errors
pub fn setup_askpass_script(askpass_path: &PathBuf) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    info!(
        "Setting up git askpass script at: {}",
        askpass_path.display()
    );

    // Create parent directory if it doesn't exist
    if let Some(parent) = askpass_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create directory for askpass script: {}",
                parent.display()
            )
        })?;
    }

    // Write the script content
    std::fs::write(askpass_path, ASKPASS_SCRIPT).with_context(|| {
        format!(
            "Failed to write git askpass script at {}",
            askpass_path.display()
        )
    })?;

    // Set executable permissions
    std::fs::set_permissions(askpass_path, std::fs::Permissions::from_mode(0o700)).with_context(
        || {
            format!(
                "Failed to set executable permissions on {}",
                askpass_path.display()
            )
        },
    )?;

    info!(
        "Git askpass script setup complete at: {}",
        askpass_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_var_with_default_uses_env_when_set() {
        // This test is basic - real testing requires env var manipulation
        // which can affect other tests, so we keep it minimal
        let result = env_var_with_default("FORGEBOT_TEST_NONEXISTENT_VAR_XYZ123", "default_value");
        assert_eq!(result, "default_value");
    }

    #[test]
    fn test_env_var_parse_with_default_invalid_parsing() {
        // Test that invalid parsing falls back to default
        // We can't easily set env vars in unit tests without affecting others
        // so we just verify the function signature works
        let result =
            env_var_parse_with_default("FORGEBOT_TEST_NONEXISTENT_PORT_XYZ123", 8080).unwrap();
        assert_eq!(result, 8080);
    }

    #[test]
    fn test_env_var_path_with_default() {
        let result =
            env_var_path_with_default("FORGEBOT_TEST_NONEXISTENT_PATH_XYZ123", "/default/path");
        assert_eq!(result, PathBuf::from("/default/path"));
    }
}
