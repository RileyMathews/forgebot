use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Row, Sqlite};
use std::path::Path;
use tracing::{debug, info};

use crate::config::DatabaseConfig;

/// Type alias for SQLite connection pool
pub type DbPool = Pool<Sqlite>;

/// Repository record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub id: String,
    pub full_name: String,
    pub default_branch: String,
    pub env_loader: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Session record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub repo_full_name: String,
    pub issue_id: i64,
    pub pr_id: Option<i64>,
    pub opencode_session_id: String,
    pub worktree_path: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

/// New session data for insertion (without generated fields)
#[derive(Debug, Clone)]
pub struct NewSession {
    pub id: String,
    pub repo_full_name: String,
    pub issue_id: i64,
    pub pr_id: Option<i64>,
    pub opencode_session_id: String,
    pub worktree_path: String,
    pub state: String,
}

/// Pending worktree record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingWorktree {
    pub session_id: String,
    pub worktree_path: String,
    pub scheduled_at: String,
}

/// Initialize the database pool and run migrations
pub async fn init_db(config: &DatabaseConfig) -> Result<DbPool> {
    let db_path = &config.path;

    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create database directory: {}", parent.display()))?;
    }

    // Build connection options with create_if_missing
    let db_path_str = db_path.to_str()
        .context("Invalid database path (not UTF-8)")?;
    let connect_options = SqliteConnectOptions::new()
        .filename(db_path_str)
        .create_if_missing(true);

    debug!("Connecting to database at: {}", db_path.display());

    // Create connection pool
    let pool = SqlitePoolOptions::new()
        .connect_with(connect_options)
        .await
        .with_context(|| format!("Failed to connect to database: {}", db_path.display()))?;

    // Run migrations
    info!("Running database migrations...");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("Failed to run database migrations")?;

    info!("Database initialized successfully at: {}", db_path.display());
    Ok(pool)
}

/// Initialize database from a path directly (for testing)
pub async fn init_db_at_path(db_path: &Path) -> Result<DbPool> {
    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create database directory: {}", parent.display()))?;
    }

    let db_path_str = db_path.to_str()
        .context("Invalid database path (not UTF-8)")?;
    let connect_options = SqliteConnectOptions::new()
        .filename(db_path_str)
        .create_if_missing(true);

    debug!("Connecting to database at: {}", db_path.display());

    let pool = SqlitePoolOptions::new()
        .connect_with(connect_options)
        .await
        .with_context(|| format!("Failed to connect to database: {}", db_path.display()))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("Failed to run database migrations")?;

    info!("Database initialized successfully at: {}", db_path.display());
    Ok(pool)
}

// ============================================================================
// Repo CRUD Operations
// ============================================================================

/// Insert a new repository
pub async fn insert_repo(
    pool: &DbPool,
    id: &str,
    full_name: &str,
    default_branch: &str,
    env_loader: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO repos (id, full_name, default_branch, env_loader)
        VALUES (?1, ?2, ?3, ?4)
        "#,
    )
    .bind(id)
    .bind(full_name)
    .bind(default_branch)
    .bind(env_loader)
    .execute(pool)
    .await
    .with_context(|| format!("Failed to insert repo: {}", full_name))?;

    debug!("Inserted repo: {}", full_name);
    Ok(())
}

/// Get a repository by its full name
pub async fn get_repo_by_full_name(pool: &DbPool, full_name: &str) -> Result<Option<Repo>> {
    let row = sqlx::query(
        r#"
        SELECT id, full_name, default_branch, env_loader, created_at, updated_at
        FROM repos
        WHERE full_name = ?1
        "#,
    )
    .bind(full_name)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("Failed to get repo by full name: {}", full_name))?;

    match row {
        Some(row) => Ok(Some(Repo {
            id: row.get("id"),
            full_name: row.get("full_name"),
            default_branch: row.get("default_branch"),
            env_loader: row.get("env_loader"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })),
        None => Ok(None),
    }
}

/// List all repositories
pub async fn list_repos(pool: &DbPool) -> Result<Vec<Repo>> {
    let rows = sqlx::query(
        r#"
        SELECT id, full_name, default_branch, env_loader, created_at, updated_at
        FROM repos
        ORDER BY full_name
        "#,
    )
    .fetch_all(pool)
    .await
    .context("Failed to list repos")?;

    let repos = rows
        .into_iter()
        .map(|row| Repo {
            id: row.get("id"),
            full_name: row.get("full_name"),
            default_branch: row.get("default_branch"),
            env_loader: row.get("env_loader"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
        .collect();

    Ok(repos)
}

/// Update a repository's env_loader setting
pub async fn update_repo_env_loader(
    pool: &DbPool,
    full_name: &str,
    env_loader: &str,
) -> Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE repos
        SET env_loader = ?1, updated_at = datetime('now')
        WHERE full_name = ?2
        "#,
    )
    .bind(env_loader)
    .bind(full_name)
    .execute(pool)
    .await
    .with_context(|| format!("Failed to update repo env_loader: {}", full_name))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("Repo not found: {}", full_name);
    }

    debug!("Updated repo env_loader: {} -> {}", full_name, env_loader);
    Ok(())
}

// ============================================================================
// Session CRUD Operations
// ============================================================================

/// Insert a new session
pub async fn insert_session(pool: &DbPool, session: &NewSession) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO sessions (id, repo_full_name, issue_id, pr_id, opencode_session_id, worktree_path, state)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
    )
    .bind(&session.id)
    .bind(&session.repo_full_name)
    .bind(session.issue_id)
    .bind(session.pr_id)
    .bind(&session.opencode_session_id)
    .bind(&session.worktree_path)
    .bind(&session.state)
    .execute(pool)
    .await
    .with_context(|| format!("Failed to insert session: {}", session.id))?;

    debug!("Inserted session: {} for repo {} issue {}",
        session.id, session.repo_full_name, session.issue_id);
    Ok(())
}

/// Get a session by repository and issue ID
pub async fn get_session_by_issue(
    pool: &DbPool,
    repo_full_name: &str,
    issue_id: i64,
) -> Result<Option<Session>> {
    let row = sqlx::query(
        r#"
        SELECT id, repo_full_name, issue_id, pr_id, opencode_session_id,
               worktree_path, state, created_at, updated_at
        FROM sessions
        WHERE repo_full_name = ?1 AND issue_id = ?2
        "#,
    )
    .bind(repo_full_name)
    .bind(issue_id)
    .fetch_optional(pool)
    .await
    .with_context(|| {
        format!(
            "Failed to get session by issue: {}#{}",
            repo_full_name, issue_id
        )
    })?;

    match row {
        Some(row) => Ok(Some(Session {
            id: row.get("id"),
            repo_full_name: row.get("repo_full_name"),
            issue_id: row.get("issue_id"),
            pr_id: row.get("pr_id"),
            opencode_session_id: row.get("opencode_session_id"),
            worktree_path: row.get("worktree_path"),
            state: row.get("state"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })),
        None => Ok(None),
    }
}

/// Get a session by PR ID
pub async fn get_session_by_pr(pool: &DbPool, pr_id: i64) -> Result<Option<Session>> {
    let row = sqlx::query(
        r#"
        SELECT id, repo_full_name, issue_id, pr_id, opencode_session_id,
               worktree_path, state, created_at, updated_at
        FROM sessions
        WHERE pr_id = ?1
        "#,
    )
    .bind(pr_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("Failed to get session by PR: {}", pr_id))?;

    match row {
        Some(row) => Ok(Some(Session {
            id: row.get("id"),
            repo_full_name: row.get("repo_full_name"),
            issue_id: row.get("issue_id"),
            pr_id: row.get("pr_id"),
            opencode_session_id: row.get("opencode_session_id"),
            worktree_path: row.get("worktree_path"),
            state: row.get("state"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })),
        None => Ok(None),
    }
}

/// Update a session's state
pub async fn update_session_state(pool: &DbPool, session_id: &str, state: &str) -> Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE sessions
        SET state = ?1, updated_at = datetime('now')
        WHERE id = ?2
        "#,
    )
    .bind(state)
    .bind(session_id)
    .execute(pool)
    .await
    .with_context(|| format!("Failed to update session state: {}", session_id))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("Session not found: {}", session_id);
    }

    debug!("Updated session state: {} -> {}", session_id, state);
    Ok(())
}

/// Update a session's PR ID
pub async fn update_session_pr_id(pool: &DbPool, session_id: &str, pr_id: i64) -> Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE sessions
        SET pr_id = ?1, updated_at = datetime('now')
        WHERE id = ?2
        "#,
    )
    .bind(pr_id)
    .bind(session_id)
    .execute(pool)
    .await
    .with_context(|| format!("Failed to update session PR ID: {}", session_id))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("Session not found: {}", session_id);
    }

    debug!("Updated session PR ID: {} -> {}", session_id, pr_id);
    Ok(())
}

/// Get all sessions in the specified states
pub async fn get_sessions_in_state(pool: &DbPool, states: &[&str]) -> Result<Vec<Session>> {
    if states.is_empty() {
        return Ok(Vec::new());
    }

    // Build the IN clause with the correct number of placeholders
    let placeholders = states.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let query_str = format!(
        r#"
        SELECT id, repo_full_name, issue_id, pr_id, opencode_session_id,
               worktree_path, state, created_at, updated_at
        FROM sessions
        WHERE state IN ({})
        ORDER BY created_at DESC
        "#,
        placeholders
    );

    let mut query = sqlx::query(&query_str);
    for state in states {
        query = query.bind(state);
    }

    let rows = query.fetch_all(pool).await.context("Failed to get sessions by state")?;

    let sessions = rows
        .into_iter()
        .map(|row| Session {
            id: row.get("id"),
            repo_full_name: row.get("repo_full_name"),
            issue_id: row.get("issue_id"),
            pr_id: row.get("pr_id"),
            opencode_session_id: row.get("opencode_session_id"),
            worktree_path: row.get("worktree_path"),
            state: row.get("state"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
        .collect();

    Ok(sessions)
}

// ============================================================================
// Pending Worktree Operations
// ============================================================================

/// Add a worktree to the pending cleanup queue
pub async fn add_pending_worktree(pool: &DbPool, session_id: &str, worktree_path: &str) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO pending_worktrees (session_id, worktree_path)
        VALUES (?1, ?2)
        "#,
    )
    .bind(session_id)
    .bind(worktree_path)
    .execute(pool)
    .await
    .with_context(|| format!("Failed to add pending worktree: {}", worktree_path))?;

    debug!("Added pending worktree: {} for session {}", worktree_path, session_id);
    Ok(())
}

/// List all pending worktrees
pub async fn list_pending_worktrees(pool: &DbPool) -> Result<Vec<PendingWorktree>> {
    let rows = sqlx::query(
        r#"
        SELECT session_id, worktree_path, scheduled_at
        FROM pending_worktrees
        ORDER BY scheduled_at
        "#,
    )
    .fetch_all(pool)
    .await
    .context("Failed to list pending worktrees")?;

    let worktrees = rows
        .into_iter()
        .map(|row| PendingWorktree {
            session_id: row.get("session_id"),
            worktree_path: row.get("worktree_path"),
            scheduled_at: row.get("scheduled_at"),
        })
        .collect();

    Ok(worktrees)
}

/// Remove a pending worktree (after cleanup)
pub async fn remove_pending_worktree(pool: &DbPool, session_id: &str) -> Result<()> {
    let result = sqlx::query(
        r#"
        DELETE FROM pending_worktrees
        WHERE session_id = ?1
        "#,
    )
    .bind(session_id)
    .execute(pool)
    .await
    .with_context(|| format!("Failed to remove pending worktree for session: {}", session_id))?;

    if result.rows_affected() > 0 {
        debug!("Removed pending worktree for session: {}", session_id);
    }

    Ok(())
}
