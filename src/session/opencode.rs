//! opencode configuration directory setup and subprocess invocation
//!
//! This module handles:
//! 1. Writing the global opencode config directory on startup
//! 2. Spawning opencode subprocesses with proper environment
//! 3. Session orchestration and lifecycle management
//! 4. Crash recovery on startup

use crate::config::{Config, OpencodeConfig};
use crate::db::{
    DbPool, NewSession, Session, get_sessions_in_state, insert_session, update_session_state,
};
use crate::forgejo::ForgejoClient;
use crate::session::env_loader;
use crate::session::worktree;
use crate::session::{SessionTrigger, build_prompt, derive_session_id};
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

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

/// Run opencode subprocess with the given parameters.
///
/// # Arguments
/// * `config` - The forgebot configuration
/// * `session_id` - The session ID for this invocation
/// * `agent_mode` - The agent mode: "plan" or "build"
/// * `worktree_path` - Path to the worktree directory
/// * `prompt` - The prompt string to pass to opencode
/// * `env_extras` - Additional environment variables from env loader
///
/// # Returns
/// * `Ok(())` if opencode exits with code 0
/// * `Err` if opencode fails or exits non-zero
pub async fn run_opencode(
    config: &Config,
    session_id: &str,
    agent_mode: &str,
    worktree_path: &Path,
    prompt: &str,
    env_extras: HashMap<String, String>,
) -> Result<()> {
    let binary = &config.opencode.binary;
    let opencode_config_home = config.opencode.config_dir.clone();

    debug!(
        "Spawning opencode: binary={}, session_id={}, agent_mode={}",
        binary, session_id, agent_mode
    );

    // Build environment
    let mut env_vars: HashMap<String, String> = HashMap::new();

    // 1. Start with process environment
    for (key, value) in std::env::vars() {
        env_vars.insert(key, value);
    }

    // Log the PATH for debugging
    let path = env_vars.get("PATH").cloned().unwrap_or_else(|| "NOT_SET".to_string());
    info!("Environment PATH: {}", path);
    info!("Binary name: {}", binary);

    // 2. Add env loader output (direnv/nix results)
    for (key, value) in env_extras {
        env_vars.insert(key, value);
    }

    // 3. Set FORGEBOT_* vars (always win)
    env_vars.insert(
        "FORGEBOT_FORGEJO_URL".to_string(),
        config.forgejo.url.clone(),
    );
    env_vars.insert(
        "FORGEBOT_FORGEJO_TOKEN".to_string(),
        config.forgejo.token.clone(),
    );
    env_vars.insert("FORGEBOT_REPO".to_string(), config.forgejo.url.clone());
    env_vars.insert(
        "OPENCODE_CONFIG_HOME".to_string(),
        opencode_config_home.display().to_string(),
    );

    // Resolve binary path from PATH if not an absolute path
    let binary_path = if binary.contains('/') {
        binary.to_string()
    } else {
        let path_var = env_vars.get("PATH").cloned().unwrap_or_default();
        let mut found = None;
        for dir in path_var.split(':') {
            let candidate = std::path::Path::new(dir).join(binary);
            if candidate.exists() {
                found = Some(candidate.to_string_lossy().to_string());
                break;
            }
        }
        found.unwrap_or_else(|| binary.to_string())
    };

    // Ensure worktree directory exists
    if !worktree_path.exists() {
        info!("Creating worktree directory: {}", worktree_path.display());
        std::fs::create_dir_all(worktree_path)
            .with_context(|| format!("Failed to create worktree directory: {}", worktree_path.display()))?;
    }

    // Build the command
    let mut cmd = Command::new(&binary_path);
    cmd.arg("run")
        .arg("--session")
        .arg(session_id)
        .arg("--agent")
        .arg(agent_mode)
        .arg(prompt)
        .current_dir(worktree_path)
        .envs(&env_vars)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    info!(
        "Running opencode command: binary={}, resolved_path={}, worktree={}",
        binary, binary_path, worktree_path.display()
    );

    let output = match cmd.output().await {
        Ok(output) => output,
        Err(e) => {
            error!(
                "Failed to spawn opencode process: {} (resolved to {}): kind={:?}, os_error={:?}",
                binary, binary_path, e.kind(), e.raw_os_error()
            );
            return Err(anyhow!(
                "Failed to spawn opencode process: {} (resolved to {}): {}",
                binary, binary_path, e
            ));
        }
    };

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        debug!("opencode exited successfully with code 0");
        Ok(())
    } else {
        error!(
            "opencode failed with exit code {}: stdout={}, stderr={}",
            exit_code, stdout, stderr
        );
        Err(anyhow!(
            "opencode process failed with exit code {}: stdout={}, stderr={}",
            exit_code,
            stdout,
            stderr
        ))
    }
}

/// Main session orchestration function.
///
/// This is called from webhook handlers to dispatch a new session.
/// It handles the full lifecycle: loading env, building prompt,
/// spawning opencode, and updating state.
///
/// This function runs in a spawned task, so it can block without
/// holding up the webhook response.
///
/// # Arguments
/// * `db` - Database connection pool
/// * `forgejo` - Forgejo API client
/// * `config` - Forgebot configuration
/// * `trigger` - The session trigger event
pub async fn dispatch_session(
    db: &DbPool,
    forgejo: &ForgejoClient,
    config: &Config,
    trigger: SessionTrigger,
) -> Result<()> {
    let session_id = derive_session_id(&trigger.repo_full_name, trigger.issue_id);

    info!(
        session_id = %session_id,
        agent_mode = %trigger.action,
        repo = %trigger.repo_full_name,
        issue_id = %trigger.issue_id,
        "Dispatching session"
    );

    // 1. Fetch issue details from Forgejo
    let issue = match forgejo
        .get_issue(&trigger.repo_full_name, trigger.issue_id)
        .await
    {
        Ok(issue) => issue,
        Err(e) => {
            error!("Failed to fetch issue {}: {}", trigger.issue_id, e);
            return Err(anyhow!("Failed to fetch issue: {}", e));
        }
    };

    // 2. Fetch issue comments
    let issue_comments = match forgejo
        .list_issue_comments(&trigger.repo_full_name, trigger.issue_id)
        .await
    {
        Ok(comments) => comments,
        Err(e) => {
            warn!(
                "Failed to fetch issue comments for {}: {}",
                trigger.issue_id, e
            );
            Vec::new()
        }
    };

    // 3. Fetch PR review comments if in revision phase
    let pr_review_comments = if trigger.action == "revision" && trigger.pr_id.is_some() {
        // Safe to unwrap: guarded by is_some() check above
        let pr_id = trigger.pr_id.unwrap();
        match forgejo
            .list_pr_review_comments(&trigger.repo_full_name, pr_id)
            .await
        {
            Ok(comments) => comments,
            Err(e) => {
                warn!("Failed to fetch PR review comments for PR {}: {}", pr_id, e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // 4. Check if session already exists
    let existing_session =
        crate::db::get_session_by_issue(db, &trigger.repo_full_name, trigger.issue_id as i64)
            .await?;

    // 5. Determine state and check if busy
    let new_state = match trigger.action.as_str() {
        "plan" => "planning",
        "build" => "building",
        "revision" => "revising",
        _ => {
            error!("Unknown action: {}", trigger.action);
            return Err(anyhow!("Unknown action: {}", trigger.action));
        }
    };

    // If session exists and is busy, reject
    if let Some(ref session) = existing_session
        && (session.state == "planning"
            || session.state == "building"
            || session.state == "revising")
    {
        info!(
            "Session {} is busy (state: {}), posting rejection comment",
            session_id, session.state
        );
        let _ = forgejo.post_issue_comment(
                &trigger.repo_full_name,
                trigger.issue_id,
                &format!(
                    "⚠️ forgebot is currently busy (state: {}). Please wait for the current operation to complete before triggering a new one.",
                    session.state
                ),
            ).await;
        return Err(anyhow!("Session is busy"));
    }

    // 6. Load environment
    let env_extras = match env_loader::load_env("none", &config.opencode.worktree_base).await {
        Ok(env) => env,
        Err(e) => {
            let error_str = e.to_string();
            error!(
                "Environment loading failed for session {}: {}",
                session_id, error_str
            );
            let _ = forgejo
                .post_issue_comment(
                    &trigger.repo_full_name,
                    trigger.issue_id,
                    &format!(
                        "❌ forgebot: env loader failed and the session cannot continue. \
Fix the loader configuration and re-trigger when ready. \
Error output: {}",
                        error_str
                    ),
                )
                .await;

            // Set state to error if session exists
            if let Some(ref session) = existing_session {
                let _ = update_session_state(db, &session.id, "error").await;
            }

            return Err(anyhow!("Environment loading failed: {}", error_str));
        }
    };

    // 7. Build prompt
    let prompt = build_prompt(
        &trigger.action,
        &issue,
        &issue_comments,
        &pr_review_comments,
        trigger.pr_id,
    );

    // 8. Get or create worktree
    let worktree_path =
        worktree::worktree_path(&config.opencode, &trigger.repo_full_name, trigger.issue_id);

    // If worktree doesn't exist, we need to create it
    if !worktree_path.exists() {
        warn!(
            "Worktree does not exist at {}. It will be created when needed.",
            worktree_path.display()
        );
    }

    // 9. Get or create session record
    let session_record: Session;
    if let Some(session) = existing_session {
        session_record = session;
    } else {
        // Create new session
        let new_session = NewSession {
            id: uuid::Uuid::new_v4().to_string(),
            repo_full_name: trigger.repo_full_name.clone(),
            issue_id: trigger.issue_id as i64,
            pr_id: trigger.pr_id.map(|id| id as i64),
            opencode_session_id: session_id.clone(),
            worktree_path: worktree_path.display().to_string(),
            state: "idle".to_string(),
        };
        insert_session(db, &new_session).await?;

        session_record =
            crate::db::get_session_by_issue(db, &trigger.repo_full_name, trigger.issue_id as i64)
                .await?
                .ok_or_else(|| anyhow!("Failed to retrieve newly created session"))?;
    }

    // 10. Post acknowledgement comment
    let ack_msg = match trigger.action.as_str() {
        "plan" => format!(
            "🤖 forgebot is starting to work on this issue. Creating plan...\n\nSession: `{}`",
            session_id
        ),
        "build" => format!(
            "🤖 forgebot is implementing the plan. Building...\n\nSession: `{}`",
            session_id
        ),
        "revision" => format!(
            "🤖 forgebot is addressing review comments. Revising...\n\nSession: `{}`",
            session_id
        ),
        _ => format!("🤖 forgebot is starting work.\n\nSession: `{}`", session_id),
    };

    let _ = forgejo
        .post_issue_comment(&trigger.repo_full_name, trigger.issue_id, &ack_msg)
        .await;

    // 11. Update session state
    update_session_state(db, &session_record.id, new_state).await?;

    // 12. Determine agent mode
    let agent_mode = match trigger.action.as_str() {
        "plan" => "plan",
        "build" => "build",
        "revision" => "build", // revision uses build agent mode
        _ => "build",
    };

    // 13. Set FORGEBOT_* env vars for this session
    let mut session_env = env_extras.clone();
    session_env.insert(
        "FORGEBOT_ISSUE_ID".to_string(),
        trigger.issue_id.to_string(),
    );
    if let Some(pr_id) = trigger.pr_id {
        session_env.insert("FORGEBOT_PR_ID".to_string(), pr_id.to_string());
    }

    // 14. Spawn opencode
    info!(
        session_id = %session_id,
        agent_mode = %agent_mode,
        worktree_path = %worktree_path.display(),
        "Spawning opencode"
    );

    let opencode_result = run_opencode(
        config,
        &session_id,
        agent_mode,
        &worktree_path,
        &prompt,
        session_env,
    )
    .await;

    // 15. Handle result
    match opencode_result {
        Ok(()) => {
            info!(
                session_id = %session_id,
                exit_code = 0,
                "Session completed successfully"
            );
            update_session_state(db, &session_record.id, "idle").await?;

            let success_msg = match trigger.action.as_str() {
                "plan" => {
                    "✅ Plan created successfully! Check the comments above for the plan details."
                }
                "build" => "✅ Implementation complete! A pull request has been created.",
                "revision" => "✅ Review comments addressed and changes pushed.",
                _ => "✅ Task completed successfully.",
            };
            let _ = forgejo
                .post_issue_comment(&trigger.repo_full_name, trigger.issue_id, success_msg)
                .await;

            Ok(())
        }
        Err(e) => {
            let error_str = e.to_string();
            error!(
                session_id = %session_id,
                error = %error_str,
                "Session failed"
            );
            update_session_state(db, &session_record.id, "error").await?;

            let error_msg = format!(
                "❌ Task failed. Error: {}\n\nSession set to error state. Please re-trigger when ready.",
                error_str
            );
            let _ = forgejo
                .post_issue_comment(&trigger.repo_full_name, trigger.issue_id, &error_msg)
                .await;

            Err(e)
        }
    }
}

/// Crash recovery: handle sessions that were in progress when forgebot restarted.
///
/// This function is called on startup before the server starts.
/// It finds all sessions in "planning", "building", or "revising" state and:
/// 1. Sets them to "error" state
/// 2. Posts a comment on the issue explaining what happened
///
/// # Arguments
/// * `db` - Database connection pool
/// * `forgejo` - Forgejo API client
/// * `config` - Forgebot configuration
///
/// # Returns
/// Always returns Ok(()) - failures are logged but don't block startup
pub async fn startup_crash_recovery(
    db: &DbPool,
    forgejo: &ForgejoClient,
    _config: &Config,
) -> Result<usize> {
    info!("Running startup crash recovery...");

    let stuck_states = ["planning", "building", "revising"];
    let stuck_sessions = match get_sessions_in_state(db, &stuck_states).await {
        Ok(sessions) => sessions,
        Err(e) => {
            error!("Failed to query stuck sessions: {}", e);
            return Ok(0); // Non-blocking
        }
    };

    if stuck_sessions.is_empty() {
        info!("No stuck sessions found, crash recovery complete");
        return Ok(0);
    }

    let session_count = stuck_sessions.len();
    info!(
        session_count = %session_count,
        "Found sessions stuck in progress, recovering"
    );

    for session in stuck_sessions {
        warn!(
            session_id = %session.id,
            state = %session.state,
            repo = %session.repo_full_name,
            issue_id = %session.issue_id,
            "Recovering stuck session"
        );

        // Set state to error
        if let Err(e) = update_session_state(db, &session.id, "error").await {
            error!(
                session_id = %session.id,
                error = %e,
                "Failed to set session to error state"
            );
            continue;
        }

        // Post recovery comment
        let recovery_msg =
            "⚠️ forgebot restarted mid-run. The session has been reset. Please retry your command.";

        if let Err(e) = forgejo
            .post_issue_comment(
                &session.repo_full_name,
                session.issue_id as u64,
                recovery_msg,
            )
            .await
        {
            error!(
                "Failed to post recovery comment for session {}: {}",
                session.id, e
            );
        } else {
            info!(
                session_id = %session.id,
                "Posted recovery comment"
            );
        }
    }

    info!(
        recovered_count = %session_count,
        "Crash recovery complete"
    );
    Ok(session_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_session_id() {
        // Basic case
        let id = derive_session_id("Alice/My-Repo", 42);
        assert_eq!(id, "ses_42_alice_my_repo");

        // Already lowercase
        let id = derive_session_id("alice/myrepo", 123);
        assert_eq!(id, "ses_123_alice_myrepo");

        // With dots (should become underscores)
        let id = derive_session_id("user/repo.name", 1);
        assert_eq!(id, "ses_1_user_repo_name");

        // With multiple special chars
        let id = derive_session_id("My-Org/Some_Repo", 99);
        assert_eq!(id, "ses_99_my_org_some_repo");

        // With numbers
        let id = derive_session_id("org2/repo-v1.0", 7);
        assert_eq!(id, "ses_7_org2_repo_v1_0");

        // Edge case: missing slash
        let id = derive_session_id("just-owner", 5);
        assert_eq!(id, "ses_5_just_owner_");
    }

    #[test]
    fn test_sanitize_for_session_id() {
        // Test via derive_session_id since sanitize is private
        assert_eq!(derive_session_id("My-Repo/Test", 1), "ses_1_my_repo_test");
        assert_eq!(derive_session_id("UPPER/LOWER", 1), "ses_1_upper_lower");
        assert_eq!(derive_session_id("123/456", 1), "ses_1_123_456");
    }

    // Note: run_opencode tests would require mocking or actual opencode binary
    // Note: dispatch_session tests would require complex mocking of db and forgejo
    // Note: startup_crash_recovery tests would require a test database

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
            git_binary: "git".to_string(),
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
            git_binary: "git".to_string(),
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
