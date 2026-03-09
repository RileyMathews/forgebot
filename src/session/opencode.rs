//! opencode configuration directory setup
//!
//! This module handles writing the global opencode config directory on startup.
//! Files are embedded in the binary and written only if they don't already exist,
//! allowing operators to customize them without having changes overwritten.

use crate::config::OpencodeConfig;
use anyhow::{Context, Result};
use tracing::info;

// Template files embedded at compile time
const PACKAGE_JSON: &str = include_str!("../../opencode-config/package.json");
const AGENT_DEF: &str = include_str!("../../opencode-config/agents/forgebot.md");
const TOOL_COMMENT_ISSUE: &str = include_str!("../../opencode-config/tools/comment-issue.ts");
const TOOL_COMMENT_PR: &str = include_str!("../../opencode-config/tools/comment-pr.ts");
const TOOL_CREATE_PR: &str = include_str!("../../opencode-config/tools/create-pr.ts");

/// Sets up the opencode config directory with embedded template files.
///
/// This function is called once on startup. It creates the directory structure
/// and writes template files if they don't already exist. Existing files are
/// never overwritten, allowing operators to customize them.
///
/// # Arguments
/// * `config` - The opencode configuration containing the config_dir path
///
/// # Returns
/// * `Ok(())` on success
/// * `Err` on permission or I/O errors
pub fn setup_opencode_config_dir(config: &OpencodeConfig) -> Result<()> {
    let config_dir = &config.config_dir;

    info!(
        "Setting up opencode config directory at: {}",
        config_dir.display()
    );

    // Create the main config directory
    std::fs::create_dir_all(config_dir).with_context(|| {
        format!(
            "Failed to create config directory: {}",
            config_dir.display()
        )
    })?;

    // Create subdirectories
    let agents_dir = config_dir.join("agents");
    let tools_dir = config_dir.join("tools");

    std::fs::create_dir_all(&agents_dir).with_context(|| {
        format!(
            "Failed to create agents directory: {}",
            agents_dir.display()
        )
    })?;

    std::fs::create_dir_all(&tools_dir)
        .with_context(|| format!("Failed to create tools directory: {}", tools_dir.display()))?;

    // Define template files to write
    let files_to_write = [
        (
            config_dir.join("package.json"),
            PACKAGE_JSON,
            "package.json",
        ),
        (
            agents_dir.join("forgebot.md"),
            AGENT_DEF,
            "agents/forgebot.md",
        ),
        (
            tools_dir.join("comment-issue.ts"),
            TOOL_COMMENT_ISSUE,
            "tools/comment-issue.ts",
        ),
        (
            tools_dir.join("comment-pr.ts"),
            TOOL_COMMENT_PR,
            "tools/comment-pr.ts",
        ),
        (
            tools_dir.join("create-pr.ts"),
            TOOL_CREATE_PR,
            "tools/create-pr.ts",
        ),
    ];

    // Write each file if it doesn't exist
    for (path, content, name) in &files_to_write {
        if path.exists() {
            info!("opencode config file already exists, skipping: {}", name);
        } else {
            std::fs::write(path, content).with_context(|| {
                format!(
                    "Failed to write opencode config file: {} at {}",
                    name,
                    path.display()
                )
            })?;
            info!("Created opencode config file: {}", name);
        }
    }

    info!("opencode config directory setup complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_setup_opencode_config_dir_creates_files() {
        // Create a temporary directory for testing
        let temp_dir =
            std::env::temp_dir().join(format!("test-opencode-config-{}", std::process::id()));

        // Clean up any existing test directory
        let _ = std::fs::remove_dir_all(&temp_dir);

        let config = OpencodeConfig {
            binary: "opencode".to_string(),
            worktree_base: temp_dir.join("worktrees"),
            config_dir: temp_dir.clone(),
        };

        // First call should create all files
        setup_opencode_config_dir(&config).expect("Setup should succeed");

        // Verify all files exist
        assert!(temp_dir.join("package.json").exists());
        assert!(temp_dir.join("agents").join("forgebot.md").exists());
        assert!(temp_dir.join("tools").join("comment-issue.ts").exists());
        assert!(temp_dir.join("tools").join("comment-pr.ts").exists());
        assert!(temp_dir.join("tools").join("create-pr.ts").exists());

        // Verify content was written correctly
        let package_json_content = std::fs::read_to_string(temp_dir.join("package.json")).unwrap();
        assert!(package_json_content.contains("@opencode-ai/plugin"));

        let agent_content =
            std::fs::read_to_string(temp_dir.join("agents").join("forgebot.md")).unwrap();
        assert!(agent_content.contains("forgebot"));

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_setup_opencode_config_dir_preserves_existing_files() {
        let temp_dir = std::env::temp_dir().join(format!(
            "test-opencode-config-existing-{}",
            std::process::id()
        ));

        // Clean up any existing test directory
        let _ = std::fs::remove_dir_all(&temp_dir);

        // Create the directory and a custom file first
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("package.json"), "custom content").unwrap();

        let config = OpencodeConfig {
            binary: "opencode".to_string(),
            worktree_base: temp_dir.join("worktrees"),
            config_dir: temp_dir.clone(),
        };

        // Setup should succeed and not overwrite existing files
        setup_opencode_config_dir(&config).expect("Setup should succeed");

        // Verify custom content was preserved
        let content = std::fs::read_to_string(temp_dir.join("package.json")).unwrap();
        assert_eq!(content, "custom content");

        // But other files should still be created
        assert!(temp_dir.join("agents").join("forgebot.md").exists());

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
