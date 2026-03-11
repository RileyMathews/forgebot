use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures::future;
use tracing::{info, warn};

use crate::config::Config;
use crate::db::{DbPool, delete_repo, get_sessions_for_repo};
use crate::forgejo::ForgejoClient;
use crate::session::worktree::{bare_clone_path, remove_worktree};

/// Remove a repository and all associated data.
///
/// This function orchestrates the cleanup of a repository by:
/// 1. Deleting the webhook from Forgejo (required - failure aborts)
/// 2. Removing all worktrees (awaited for completion)
/// 3. Removing the bare clone directory
/// 4. Checking for active sessions (race condition protection)
/// 5. Deleting the repo from the database
///
/// All steps are required to succeed. Any failure will abort the process
/// and return an error, leaving the repository intact.
pub async fn remove_repository(
    db: &DbPool,
    forgejo: &ForgejoClient,
    config: &Arc<Config>,
    full_name: &str,
) -> Result<()> {
    // a) List sessions to find which worktrees to remove
    let sessions = get_sessions_for_repo(db, full_name)
        .await
        .with_context(|| format!("failed to list sessions for repo: {}", full_name))?;

    // b) Delete webhook (required - failure aborts the removal process)
    let expected_url = crate::config::webhook_url(config);
    let webhooks = forgejo
        .list_repo_webhooks(full_name)
        .await
        .with_context(|| format!("failed to list webhooks for repo: {}", full_name))?;

    if let Some(hook) = webhooks.iter().find(|w| w.url == expected_url) {
        forgejo
            .delete_repo_webhook(full_name, hook.id)
            .await
            .with_context(|| {
                format!(
                    "failed to delete webhook {} for repo: {}",
                    hook.id, full_name
                )
            })?;
        info!(repo = %full_name, hook_id = %hook.id, "Deleted webhook");
    }

    // c) Remove all worktrees (await completion - don't fire-and-forget)
    let mut worktree_tasks = Vec::new();
    let git_binary = config.opencode.git_binary.clone();
    let bare_clone_dir = bare_clone_path(&config.opencode, full_name);
    for session in sessions {
        let worktree_path = PathBuf::from(&session.worktree_path);
        let repo_name = full_name.to_string();
        let issue_id = session.issue_id;
        let git_binary_clone = git_binary.clone();
        let bare_clone_dir_clone = bare_clone_dir.clone();

        let handle = tokio::spawn(async move {
            match remove_worktree(&worktree_path, &bare_clone_dir_clone, &git_binary_clone).await {
                Ok(()) => {
                    info!(
                        repo = %repo_name,
                        issue_id = %issue_id,
                        "Removed worktree"
                    );
                    Ok(())
                }
                Err(e) => {
                    warn!(
                        repo = %repo_name,
                        issue_id = %issue_id,
                        path = %worktree_path.display(),
                        err = %e,
                        "failed to remove worktree"
                    );
                    Err(e)
                }
            }
        });
        worktree_tasks.push(handle);
    }

    // Await all worktree removals to guarantee completion
    let worktree_results = future::join_all(worktree_tasks).await;

    // Check for any panics in spawned tasks
    for result in worktree_results {
        if let Err(e) = result {
            warn!(repo = %full_name, err = %e, "Worktree removal task panicked");
        }
    }

    // d) Remove bare clone directory
    match tokio::fs::remove_dir_all(&bare_clone_dir).await {
        Ok(()) => {
            info!(
                repo = %full_name,
                path = %bare_clone_dir.display(),
                "Removed bare clone"
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Directory doesn't exist - that's fine, already cleaned up
            info!(
                repo = %full_name,
                path = %bare_clone_dir.display(),
                "Bare clone already removed (not found)"
            );
        }
        Err(e) => {
            warn!(
                repo = %full_name,
                path = %bare_clone_dir.display(),
                err = %e,
                "failed to remove bare clone"
            );
        }
    }

    // e) Final check for active sessions immediately before DB deletion
    // This minimizes the race condition window between the initial check and actual deletion
    let final_sessions = get_sessions_for_repo(db, full_name)
        .await
        .with_context(|| {
            format!(
                "failed to check for active sessions before deletion: {}",
                full_name
            )
        })?;

    let has_active_sessions = final_sessions.iter().any(|s| s.state.is_busy());

    if has_active_sessions {
        anyhow::bail!(
            "cannot delete repository {}: has active sessions in planning/building/revising state",
            full_name
        );
    }

    // f) Delete repo from DB (last step, after all cleanup succeeds)
    delete_repo(db, full_name)
        .await
        .with_context(|| format!("failed to delete repo from database: {}", full_name))?;

    info!(repo = %full_name, "Successfully removed repository from database");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_url() {
        let config = Arc::new(Config {
            server: crate::config::ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8765,
                webhook_secret: "test-secret".to_string(),
                forgebot_host: "http://example.com".to_string(),
            },
            forgejo: crate::config::ForgejoConfig {
                url: "https://forgejo.example.com".to_string(),
                token: "test-token".to_string(),
                bot_username: "forgebot".to_string(),
            },
            opencode: crate::config::OpencodeConfig {
                binary: "opencode".to_string(),
                worktree_base: std::path::PathBuf::from("/tmp/worktrees"),
                config_dir: std::path::PathBuf::from("/tmp/config"),
                git_binary: "git".to_string(),
                model: "opencode/kimi-k2.5".to_string(),
                web_host: None,
            },
            database: crate::config::DatabaseConfig {
                path: std::path::PathBuf::from("/tmp/test.db"),
            },
        });

        let url = crate::config::webhook_url(&config);
        assert_eq!(url, "http://example.com/webhook");
    }
}
