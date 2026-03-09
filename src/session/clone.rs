use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::{error, info};

use crate::config::Config;
use crate::db::{DbPool, update_repo_clone_status, validate_repo_full_name};
use crate::session::worktree::bare_clone_path;

/// Timeout for git clone operations (10 minutes)
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);

/// Perform a bare clone of a repository.
///
/// This function handles the full clone lifecycle:
/// 1. Updates status to "cloning" before starting
/// 2. Ensures parent directory exists
/// 3. Runs git clone --bare
/// 4. On success: updates status to "ready"
/// 5. On failure: captures stderr, updates status to "failed" with error
///
/// # Arguments
/// * `db` - The database pool for status updates
/// * `config` - The application configuration
/// * `repo_full_name` - The repository in "owner/repo" format
///
/// # Returns
/// Result<()> - Ok on successful clone, Err otherwise
pub async fn perform_clone(db: &DbPool, config: &Arc<Config>, repo_full_name: &str) -> Result<()> {
    // Validate repo_full_name format as defense-in-depth
    validate_repo_full_name(repo_full_name).context("repository name validation failed")?;

    info!(repo = %repo_full_name, "Starting repository clone");

    // Update status to "cloning" before starting
    update_repo_clone_status(db, repo_full_name, "cloning", None)
        .await
        .with_context(|| {
            format!(
                "Failed to set clone status to 'cloning' for {}",
                repo_full_name
            )
        })?;

    // Construct the bare clone path
    let bare_path = bare_clone_path(&config.opencode, repo_full_name);

    // Check if clone directory already exists (another clone may be in progress)
    // If it exists and looks complete, mark as ready and skip
    // If it exists but incomplete, this is likely a collision - report error
    if bare_path.exists() {
        let head_file = bare_path.join("HEAD");
        if head_file.exists() {
            info!(
                repo = %repo_full_name,
                path = %bare_path.display(),
                "Clone directory exists and appears complete, marking as ready"
            );
            update_repo_clone_status(db, repo_full_name, "ready", None)
                .await
                .with_context(|| {
                    format!(
                        "Failed to set clone status to 'ready' for {}",
                        repo_full_name
                    )
                })?;
            return Ok(());
        } else {
            // Directory exists but doesn't look like a valid bare clone
            update_repo_clone_status(
                db,
                repo_full_name,
                "failed",
                Some("Clone directory already exists but appears incomplete (another clone may be in progress)"),
            )
            .await
            .with_context(|| {
                format!(
                    "Failed to set clone status to 'failed' for {}",
                    repo_full_name
                )
            })?;
            anyhow::bail!(
                "Clone directory already exists for {} but appears incomplete",
                repo_full_name
            );
        }
    }

    // Construct the clone URL
    let clone_url = format!("https://{}/{}.git", config.forgejo.url, repo_full_name);

    // Ensure parent directory exists
    let parent_dir = bare_path
        .parent()
        .with_context(|| format!("Bare clone path has no parent: {}", bare_path.display()))?;

    tokio::fs::create_dir_all(parent_dir)
        .await
        .with_context(|| {
            format!(
                "Failed to create parent directory: {}",
                parent_dir.display()
            )
        })?;

    // Run git clone --bare with timeout
    info!(
        repo = %repo_full_name,
        url = %clone_url,
        path = %bare_path.display(),
        timeout_secs = CLONE_TIMEOUT.as_secs(),
        "Executing git clone --bare"
    );

    let clone_result = tokio::time::timeout(
        CLONE_TIMEOUT,
        Command::new("git")
            .arg("clone")
            .arg("--bare")
            .arg(&clone_url)
            .arg(&bare_path)
            .output(),
    )
    .await;

    match clone_result {
        Ok(Ok(output)) => {
            if output.status.success() {
                // Clone succeeded - update to ready
                update_repo_clone_status(db, repo_full_name, "ready", None)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to set clone status to 'ready' for {}",
                            repo_full_name
                        )
                    })?;

                info!(repo = %repo_full_name, "Repository clone completed successfully");
                Ok(())
            } else {
                // Clone failed - capture stderr and update to failed
                let stderr = String::from_utf8_lossy(&output.stderr);
                let error_msg = format!("git clone failed: {}", stderr);

                update_repo_clone_status(db, repo_full_name, "failed", Some(&error_msg))
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to set clone status to 'failed' for {}",
                            repo_full_name
                        )
                    })?;

                error!(
                    repo = %repo_full_name,
                    error = %stderr,
                    "Repository clone failed"
                );

                Err(anyhow::anyhow!(
                    "Clone failed for {}: {}",
                    repo_full_name,
                    error_msg
                ))
            }
        }
        Ok(Err(e)) => {
            // Command execution error (not timeout)
            let error_msg = format!("Failed to execute git clone: {}", e);

            update_repo_clone_status(db, repo_full_name, "failed", Some(&error_msg))
                .await
                .with_context(|| {
                    format!(
                        "Failed to set clone status to 'failed' for {}",
                        repo_full_name
                    )
                })?;

            error!(
                repo = %repo_full_name,
                error = %e,
                "Failed to execute git clone command"
            );

            Err(e).context("Failed to execute git clone command")
        }
        Err(_) => {
            // Timeout occurred
            let error_msg = "Clone operation timed out after 10 minutes";

            update_repo_clone_status(db, repo_full_name, "failed", Some(error_msg))
                .await
                .with_context(|| {
                    format!(
                        "Failed to set clone status to 'failed' for {}",
                        repo_full_name
                    )
                })?;

            error!(
                repo = %repo_full_name,
                timeout_secs = CLONE_TIMEOUT.as_secs(),
                "Clone operation timed out"
            );

            anyhow::bail!(
                "Clone timeout for {} after {} seconds",
                repo_full_name,
                CLONE_TIMEOUT.as_secs()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_config() -> Config {
        Config {
            server: crate::config::ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8765,
                webhook_secret: "test_secret".to_string(),
                forgebot_host: "http://localhost:8765".to_string(),
            },
            forgejo: crate::config::ForgejoConfig {
                url: "git.example.com".to_string(),
                token: "test_token".to_string(),
                bot_username: "forgebot".to_string(),
            },
            opencode: crate::config::OpencodeConfig {
                binary: "opencode".to_string(),
                worktree_base: PathBuf::from("/tmp/forgebot-test-worktrees"),
                config_dir: PathBuf::from("/tmp/forgebot-test-config"),
            },
            database: crate::config::DatabaseConfig {
                path: PathBuf::from("/tmp/forgebot-test.db"),
            },
        }
    }

    #[test]
    fn test_clone_url_construction() {
        let config = test_config();
        let repo_full_name = "alice/myrepo";

        let expected_url = format!("https://{}/{}.git", config.forgejo.url, repo_full_name);
        assert_eq!(expected_url, "https://git.example.com/alice/myrepo.git");
    }

    #[test]
    fn test_clone_url_with_special_characters() {
        let config = test_config();

        // Test with hyphens and underscores
        let repo_full_name = "my-org/repo_name-v1";
        let expected_url = format!("https://{}/{}.git", config.forgejo.url, repo_full_name);
        assert_eq!(
            expected_url,
            "https://git.example.com/my-org/repo_name-v1.git"
        );
    }

    #[test]
    fn test_bare_clone_path_has_valid_parent() {
        let config = test_config();
        let repo_full_name = "alice/myrepo";

        let bare_path = crate::session::worktree::bare_clone_path(&config.opencode, repo_full_name);

        // Path must have a parent directory (not root)
        assert!(
            bare_path.parent().is_some(),
            "Bare clone path should have a valid parent: {}",
            bare_path.display()
        );

        // Parent should be within worktree_base
        let parent = bare_path.parent().unwrap();
        assert!(
            parent.starts_with(&config.opencode.worktree_base),
            "Bare clone path parent should be within worktree_base"
        );
    }

    #[test]
    fn test_timeout_constant_is_10_minutes() {
        // Verify the timeout is set to 10 minutes as documented
        assert_eq!(CLONE_TIMEOUT.as_secs(), 600);
    }
}
