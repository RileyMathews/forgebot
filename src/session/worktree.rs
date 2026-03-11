use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::config::OpencodeConfig;

/// Compute the path for a worktree directory.
///
/// Returns: `<worktree_base>/_worktrees/<owner>_<repo>/<issue_id>/`
/// Example: `worktree_path(config, "alice/myrepo", 42)` → `/var/lib/forgebot/worktrees/_worktrees/alice_myrepo/42/`
pub fn worktree_path(config: &OpencodeConfig, repo_full_name: &str, issue_id: u64) -> PathBuf {
    let (owner, repo) = parse_repo_full_name(repo_full_name);
    let repo_dir = format!("{}_{}", owner, repo);

    config
        .worktree_base
        .join("_worktrees")
        .join(repo_dir)
        .join(issue_id.to_string())
}

/// Parse a "owner/repo" string into (owner, repo) components.
/// All components are converted to lowercase.
fn parse_repo_full_name(repo_full_name: &str) -> (String, String) {
    let parts: Vec<&str> = repo_full_name.split('/').collect();
    if parts.len() != 2 {
        // Return as-is for the first part, empty for second if malformed
        // This should not happen in practice with valid repo names
        return (
            parts.first().unwrap_or(&"").to_lowercase(),
            parts.get(1).unwrap_or(&"").to_lowercase(),
        );
    }
    (parts[0].to_lowercase(), parts[1].to_lowercase())
}

/// Get the path to the bare clone directory for a repo.
///
/// Returns: `<worktree_base>/<owner>_<repo>/`
pub fn bare_clone_path(config: &OpencodeConfig, repo_full_name: &str) -> PathBuf {
    let (owner, repo) = parse_repo_full_name(repo_full_name);
    let repo_dir = format!("{}_{}", owner, repo);

    config.worktree_base.join(repo_dir)
}

/// Check if a bare clone exists for the repository.
///
/// Checks if the directory `<worktree_base>/<owner>_<repo>/` contains a valid git repo
/// by checking for the presence of a HEAD file (exists in both bare and non-bare repos).
/// Returns true if the bare clone directory exists.
pub fn clone_exists(config: &OpencodeConfig, repo_full_name: &str) -> bool {
    let clone_path = bare_clone_path(config, repo_full_name);
    // Check for HEAD file which exists in all git repositories (bare or not)
    clone_path.join("HEAD").exists()
}

/// Create a new git worktree for an issue.
///
/// # Arguments
/// * `config` - The opencode configuration containing worktree_base path
/// * `repo_full_name` - The repository in "owner/repo" format
/// * `issue_id` - The issue number to create the worktree for
/// * `default_branch` - The default branch to base the worktree on (currently unused but reserved for future)
///
/// # Returns
/// The full path to the created worktree on success.
///
/// # Errors
/// Returns an error if:
/// - The bare clone does not exist
/// - The git worktree add command fails
pub async fn create_worktree(
    config: &OpencodeConfig,
    repo_full_name: &str,
    issue_id: u64,
    _default_branch: &str,
) -> Result<PathBuf> {
    // Check that bare clone exists
    let bare_clone_dir = bare_clone_path(config, repo_full_name);
    if !clone_exists(config, repo_full_name) {
        bail!(
            "Bare clone does not exist at {}. Please clone the repository first.",
            bare_clone_dir.display()
        );
    }

    // Compute the worktree path
    let worktree_dir = worktree_path(config, repo_full_name, issue_id);

    // Check if worktree directory already exists
    if worktree_dir.exists() {
        warn!(
            "Worktree directory already exists at {}. Removing and recreating.",
            worktree_dir.display()
        );
        // Remove the existing directory
        tokio::fs::remove_dir_all(&worktree_dir)
            .await
            .with_context(|| {
                format!(
                    "Failed to remove existing worktree directory: {}",
                    worktree_dir.display()
                )
            })?;
    }

    // Create parent directories if needed
    if let Some(parent) = worktree_dir.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            format!("Failed to create parent directories: {}", parent.display())
        })?;
    }

    // Run git worktree add command
    let branch_name = format!("agent/issue-{}", issue_id);
    info!(
        "Creating worktree for {} issue {} at {} on branch {}",
        repo_full_name,
        issue_id,
        worktree_dir.display(),
        branch_name
    );

    let output = tokio::process::Command::new(&config.git_binary)
        .arg("worktree")
        .arg("add")
        .arg(&worktree_dir)
        .arg("-b")
        .arg(&branch_name)
        .current_dir(&bare_clone_dir)
        .output()
        .await
        .with_context(|| "Failed to execute git worktree add command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git worktree add failed for {} issue {}: {}",
            repo_full_name,
            issue_id,
            stderr
        );
    }

    info!(
        "Successfully created worktree at {} for {} issue {}",
        worktree_dir.display(),
        repo_full_name,
        issue_id
    );

    Ok(worktree_dir)
}

/// Remove a git worktree.
///
/// # Arguments
/// * `path` - The path to the worktree to remove
/// * `git_binary` - Path to the git binary
///
/// # Errors
/// Returns an error if the git worktree remove command fails.
pub async fn remove_worktree(path: &Path, bare_repo_path: &Path, git_binary: &str) -> Result<()> {
    // Soft failure if path doesn't exist - just log warning and return Ok
    if !path.exists() {
        warn!(
            "Worktree path does not exist, nothing to remove: {}",
            path.display()
        );
        return Ok(());
    }

    info!("Removing worktree at {}", path.display());

    // Run git worktree remove --force from the bare clone directory.
    let output = tokio::process::Command::new(git_binary)
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(path)
        .current_dir(bare_repo_path)
        .output()
        .await
        .with_context(|| "Failed to execute git worktree remove command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git worktree remove failed for {}: {}",
            path.display(),
            stderr
        );
    }

    info!("Successfully removed worktree at {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_config() -> OpencodeConfig {
        OpencodeConfig {
            binary: "opencode".to_string(),
            worktree_base: PathBuf::from("/var/lib/forgebot/worktrees"),
            config_dir: PathBuf::from("/etc/forgebot"),
            git_binary: "git".to_string(),
            model: "opencode/kimi-k2.5".to_string(),
            web_host: None,
        }
    }

    #[test]
    fn test_worktree_path_computation() {
        let config = test_config();

        // Test basic case
        let path = worktree_path(&config, "alice/myrepo", 42);
        assert_eq!(
            path,
            PathBuf::from("/var/lib/forgebot/worktrees/_worktrees/alice_myrepo/42")
        );

        // Test with different issue ID
        let path = worktree_path(&config, "alice/myrepo", 123);
        assert_eq!(
            path,
            PathBuf::from("/var/lib/forgebot/worktrees/_worktrees/alice_myrepo/123")
        );

        // Test with different owner/repo
        let path = worktree_path(&config, "bob/another-repo", 1);
        assert_eq!(
            path,
            PathBuf::from("/var/lib/forgebot/worktrees/_worktrees/bob_another-repo/1")
        );

        // Test case insensitivity
        let path = worktree_path(&config, "ALICE/MYREPO", 42);
        assert_eq!(
            path,
            PathBuf::from("/var/lib/forgebot/worktrees/_worktrees/alice_myrepo/42")
        );
    }

    #[test]
    fn test_parse_repo_full_name() {
        // Test normal case
        let (owner, repo) = parse_repo_full_name("alice/myrepo");
        assert_eq!(owner, "alice");
        assert_eq!(repo, "myrepo");

        // Test with hyphens
        let (owner, repo) = parse_repo_full_name("some-org/my-cool-repo");
        assert_eq!(owner, "some-org");
        assert_eq!(repo, "my-cool-repo");

        // Test uppercase conversion
        let (owner, repo) = parse_repo_full_name("ALICE/MYREPO");
        assert_eq!(owner, "alice");
        assert_eq!(repo, "myrepo");

        // Test with dots
        let (owner, repo) = parse_repo_full_name("user/repo.name");
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo.name");
    }

    #[test]
    fn test_bare_clone_path_computation() {
        let config = test_config();

        let path = bare_clone_path(&config, "alice/myrepo");
        assert_eq!(
            path,
            PathBuf::from("/var/lib/forgebot/worktrees/alice_myrepo")
        );
    }

    #[test]
    fn test_clone_exists_with_nonexistent_path() {
        let config = test_config();

        // This should return false since the path doesn't actually exist
        // (unless someone has created this specific path)
        let exists = clone_exists(&config, "nonexistent/test-repo-12345");
        assert!(!exists);
    }
}
