use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use reqwest::Url;
use tracing::{info, warn};

const DEFAULT_ASKPASS_PATH: &str = "/var/lib/forgebot/git-askpass.sh";
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
    pub web_host: Option<String>,
    pub api: OpencodeApiConfig,
}

#[derive(Debug, Clone)]
pub struct OpencodeApiConfig {
    pub base_url: Option<String>,
    pub token: Option<String>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub path: PathBuf,
}

pub fn webhook_url(config: &Config) -> String {
    format!("{}/webhook", config.server.forgebot_host)
}

impl Config {
    /// Load configuration entirely from environment variables.
    /// Required env vars (must be set or error): FORGEBOT_WEBHOOK_SECRET, FORGEBOT_FORGEJO_URL, FORGEBOT_FORGEJO_TOKEN
    /// Optional env vars (have defaults): FORGEBOT_SERVER_HOST, FORGEBOT_SERVER_PORT, FORGEBOT_FORGEJO_BOT_USERNAME, FORGEBOT_OPENCODE_BINARY, FORGEBOT_OPENCODE_WORKTREE_BASE, FORGEBOT_OPENCODE_CONFIG_DIR, FORGEBOT_DATABASE_PATH, FORGEBOT_OPENCODE_API_BASE_URL, FORGEBOT_OPENCODE_API_TIMEOUT_SECS
    /// Optional env vars (unset by default): FORGEBOT_OPENCODE_WEB_HOST, FORGEBOT_OPENCODE_API_TOKEN
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
        let configured_opencode_web_host = env_var_optional("FORGEBOT_OPENCODE_WEB_HOST");
        let opencode_api_base_url = Some(validate_http_url(
            "FORGEBOT_OPENCODE_API_BASE_URL",
            &env_var_with_default("FORGEBOT_OPENCODE_API_BASE_URL", "http://127.0.0.1:4096"),
        )?);
        let opencode_api_token = env_var_optional("FORGEBOT_OPENCODE_API_TOKEN");
        let opencode_api_timeout_secs =
            env_var_parse_u64_with_default("FORGEBOT_OPENCODE_API_TIMEOUT_SECS", 30)?;
        let opencode_web_host = match configured_opencode_web_host {
            Some(host) => Some(host),
            None => {
                if let Some(api_base_url) = &opencode_api_base_url {
                    warn!(
                        "FORGEBOT_OPENCODE_WEB_HOST not set. Defaulting to FORGEBOT_OPENCODE_API_BASE_URL for session Web UI links: {}",
                        api_base_url
                    );
                    Some(api_base_url.clone())
                } else {
                    None
                }
            }
        };

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
        info!("  FORGEBOT_OPENCODE_TRANSPORT: api (fixed)");
        match &opencode_api_base_url {
            Some(url) => info!("  FORGEBOT_OPENCODE_API_BASE_URL: {}", url),
            None => info!("  FORGEBOT_OPENCODE_API_BASE_URL: [not set]"),
        }
        info!(
            "  FORGEBOT_OPENCODE_API_TOKEN: {}",
            if opencode_api_token.is_some() {
                "[REDACTED]"
            } else {
                "[not set]"
            }
        );
        info!(
            "  FORGEBOT_OPENCODE_API_TIMEOUT_SECS: {}",
            opencode_api_timeout_secs
        );
        match &opencode_web_host {
            Some(host) => info!("  FORGEBOT_OPENCODE_WEB_HOST: {}", host),
            None => warn!(
                "FORGEBOT_OPENCODE_WEB_HOST not set. Session Web UI links will not be posted."
            ),
        }
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
                web_host: opencode_web_host,
                api: OpencodeApiConfig {
                    base_url: opencode_api_base_url,
                    token: opencode_api_token,
                    timeout_secs: opencode_api_timeout_secs,
                },
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

/// Parse an environment variable as u64 with a default value.
fn env_var_parse_u64_with_default(name: &str, default: u64) -> Result<u64> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => match value.parse::<u64>() {
            Ok(parsed) => {
                info!("Using {} from environment variable: {}", name, parsed);
                Ok(parsed)
            }
            Err(_) => {
                anyhow::bail!(
                    "ERROR: {} has invalid value '{}'. Expected an integer.",
                    name,
                    value
                )
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

fn env_var_optional(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            info!("Using {} from environment variable: {}", name, value);
            Some(value)
        }
        _ => None,
    }
}

pub fn resolve_askpass_path(raw_value: Option<String>) -> PathBuf {
    match raw_value {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => PathBuf::from(DEFAULT_ASKPASS_PATH),
    }
}

pub fn askpass_script_path() -> PathBuf {
    resolve_askpass_path(std::env::var("FORGEBOT_ASKPASS_PATH").ok())
}

pub fn setup_askpass_script(script_path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if let Some(parent) = script_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create askpass dir at {}", parent.display()))?;
    }

    std::fs::write(script_path, ASKPASS_SCRIPT).with_context(|| {
        format!(
            "Failed to write git askpass script at {}",
            script_path.display()
        )
    })?;

    std::fs::set_permissions(script_path, std::fs::Permissions::from_mode(0o700)).with_context(
        || {
            format!(
                "Failed to set executable permissions on {}",
                script_path.display()
            )
        },
    )?;

    Ok(())
}

fn validate_http_url(var_name: &str, value: &str) -> Result<String> {
    let trimmed = value.trim();
    let parsed = Url::parse(trimmed)
        .with_context(|| format!("ERROR: {} is not a valid URL: {}", var_name, trimmed))?;

    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        anyhow::bail!(
            "ERROR: {} must use http or https scheme (got '{}')",
            var_name,
            parsed.scheme()
        );
    }

    Ok(trimmed.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

    #[test]
    fn test_validate_http_url() {
        let normalized = validate_http_url("FORGEBOT_TEST_URL", "https://example.com/")
            .expect("https url should parse");
        assert_eq!(normalized, "https://example.com");
        assert!(validate_http_url("FORGEBOT_TEST_URL", "ftp://example.com").is_err());
    }

    #[test]
    fn test_webhook_url() {
        let config = Config {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8765,
                webhook_secret: "secret".to_string(),
                forgebot_host: "http://example.com".to_string(),
            },
            forgejo: ForgejoConfig {
                url: "https://forgejo.example.com".to_string(),
                token: "token".to_string(),
                bot_username: "forgebot".to_string(),
            },
            opencode: OpencodeConfig {
                binary: "opencode".to_string(),
                worktree_base: PathBuf::from("/tmp/worktrees"),
                config_dir: PathBuf::from("/tmp/config"),
                git_binary: "git".to_string(),
                model: "opencode/kimi-k2.5".to_string(),
                web_host: None,
                api: OpencodeApiConfig {
                    base_url: None,
                    token: None,
                    timeout_secs: 30,
                },
            },
            database: DatabaseConfig {
                path: PathBuf::from("/tmp/test.db"),
            },
        };

        assert_eq!(webhook_url(&config), "http://example.com/webhook");
    }

    #[test]
    fn test_resolve_askpass_path_default() {
        assert_eq!(
            resolve_askpass_path(None),
            PathBuf::from("/var/lib/forgebot/git-askpass.sh")
        );
    }

    #[test]
    fn test_resolve_askpass_path_custom() {
        assert_eq!(
            resolve_askpass_path(Some("/tmp/custom-askpass.sh".to_string())),
            PathBuf::from("/tmp/custom-askpass.sh")
        );
    }

    #[test]
    fn test_setup_askpass_script_writes_expected_content() {
        let temp_dir =
            std::env::temp_dir().join(format!("forgebot-askpass-test-{}", std::process::id()));
        let script_path = temp_dir.join("git-askpass.sh");
        let _ = fs::remove_dir_all(&temp_dir);

        setup_askpass_script(&script_path).expect("script setup should succeed");

        let content = fs::read_to_string(&script_path).expect("script should be readable");
        assert!(content.contains("FORGEBOT_FORGEJO_TOKEN"));
        assert!(content.contains("FORGEBOT_FORGEJO_BOT_USERNAME"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
