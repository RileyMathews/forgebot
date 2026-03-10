use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{Pool, Row, Sqlite};
use std::path::Path;
use tracing::{debug, info};

use crate::config::DatabaseConfig;
use crate::session::{CloneStatus, SessionState};

pub mod errors;

use errors::{DbError, Result};

fn parse_clone_status(value: String) -> Result<CloneStatus> {
    value
        .parse::<CloneStatus>()
        .map_err(|source| DbError::ParseCloneStatus { value, source })
}

fn parse_session_state(value: String) -> Result<SessionState> {
    value
        .parse::<SessionState>()
        .map_err(|source| DbError::ParseSessionState { value, source })
}

fn utf8_db_path(db_path: &Path) -> Result<&str> {
    db_path
        .to_str()
        .ok_or_else(|| DbError::InvalidDatabasePath(db_path.to_path_buf()))
}

/// Type alias for SQLite connection pool
pub type DbPool = Pool<Sqlite>;

/// Repository record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub id: String,
    pub full_name: String,
    pub default_branch: String,
    pub env_loader: String,
    pub clone_status: CloneStatus,
    pub clone_error: Option<String>,
    pub clone_attempts: i64,
    pub last_clone_attempt_at: Option<String>,
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
    pub state: SessionState,
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

fn map_repo_row(row: &SqliteRow) -> Result<Repo> {
    let clone_status = parse_clone_status(row.get::<String, _>("clone_status"))?;

    Ok(Repo {
        id: row.get("id"),
        full_name: row.get("full_name"),
        default_branch: row.get("default_branch"),
        env_loader: row.get("env_loader"),
        clone_status,
        clone_error: row.get("clone_error"),
        clone_attempts: row.get("clone_attempts"),
        last_clone_attempt_at: row.get("last_clone_attempt_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn map_session_row(row: &SqliteRow) -> Result<Session> {
    let state = parse_session_state(row.get::<String, _>("state"))?;

    Ok(Session {
        id: row.get("id"),
        repo_full_name: row.get("repo_full_name"),
        issue_id: row.get("issue_id"),
        pr_id: row.get("pr_id"),
        opencode_session_id: row.get("opencode_session_id"),
        worktree_path: row.get("worktree_path"),
        state,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn map_pending_worktree_row(row: &SqliteRow) -> PendingWorktree {
    PendingWorktree {
        session_id: row.get("session_id"),
        worktree_path: row.get("worktree_path"),
        scheduled_at: row.get("scheduled_at"),
    }
}

/// Initialize the database pool and run migrations
pub async fn init_db(config: &DatabaseConfig) -> Result<DbPool> {
    let db_path = &config.path;

    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| DbError::Io {
                operation: "create directory",
                path: parent.to_path_buf(),
                source,
            })?;
    }

    // Build connection options with create_if_missing
    let db_path_str = utf8_db_path(db_path)?;
    let connect_options = SqliteConnectOptions::new()
        .filename(db_path_str)
        .create_if_missing(true);

    debug!("Connecting to database at: {}", db_path.display());

    // Create connection pool
    let pool = SqlitePoolOptions::new()
        .connect_with(connect_options)
        .await
        .map_err(|source| DbError::Connect {
            path: db_path.to_path_buf(),
            source,
        })?;

    // Run migrations
    info!("Running database migrations...");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(DbError::from)?;

    info!(
        "Database initialized successfully at: {}",
        db_path.display()
    );
    Ok(pool)
}

/// Initialize database from a path directly (for testing)
pub async fn init_db_at_path(db_path: &Path) -> Result<DbPool> {
    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| DbError::Io {
                operation: "create directory",
                path: parent.to_path_buf(),
                source,
            })?;
    }

    let db_path_str = utf8_db_path(db_path)?;
    let connect_options = SqliteConnectOptions::new()
        .filename(db_path_str)
        .create_if_missing(true);

    debug!("Connecting to database at: {}", db_path.display());

    let pool = SqlitePoolOptions::new()
        .connect_with(connect_options)
        .await
        .map_err(|source| DbError::Connect {
            path: db_path.to_path_buf(),
            source,
        })?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(DbError::from)?;

    info!(
        "Database initialized successfully at: {}",
        db_path.display()
    );
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
    .map_err(|source| DbError::from_query("insert repo", source))?;

    debug!("Inserted repo: {}", full_name);
    Ok(())
}

/// Get a repository by its full name
pub async fn get_repo_by_full_name(pool: &DbPool, full_name: &str) -> Result<Option<Repo>> {
    let row = sqlx::query(
        r#"
        SELECT id, full_name, default_branch, env_loader, clone_status, clone_error,
               clone_attempts, last_clone_attempt_at, created_at, updated_at
        FROM repos
        WHERE full_name = ?1
        "#,
    )
    .bind(full_name)
    .fetch_optional(pool)
    .await
    .map_err(|source| DbError::from_query("get repo by full name", source))?;

    row.map(|row| map_repo_row(&row)).transpose()
}

/// List all repositories
pub async fn list_repos(pool: &DbPool) -> Result<Vec<Repo>> {
    let rows = sqlx::query(
        r#"
        SELECT id, full_name, default_branch, env_loader, clone_status, clone_error,
               clone_attempts, last_clone_attempt_at, created_at, updated_at
        FROM repos
        ORDER BY full_name
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|source| DbError::from_query("list repos", source))?;

    let repos = rows
        .into_iter()
        .map(|row| map_repo_row(&row))
        .collect::<Result<Vec<_>>>()?;

    Ok(repos)
}

/// Validate a repository full name format.
///
/// Valid format: owner/repo where both owner and repo contain only
/// alphanumeric characters, hyphens, and underscores, with exactly one '/'.
///
/// Returns Ok(()) if valid, Err with message if invalid.
pub fn validate_repo_full_name(full_name: &str) -> Result<()> {
    // Check for exactly one slash
    let slash_count = full_name.chars().filter(|&c| c == '/').count();
    if slash_count != 1 {
        return Err(DbError::InvalidRepoFullName(format!(
            "Invalid repository name '{}' - must contain exactly one '/'",
            full_name
        )));
    }

    // Check each part against allowed character set
    for part in full_name.split('/') {
        if part.is_empty() {
            return Err(DbError::InvalidRepoFullName(format!(
                "Invalid repository name '{}' - empty owner or repository name",
                full_name
            )));
        }

        if !part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(DbError::InvalidRepoFullName(format!(
                "Invalid repository name '{}' - parts must contain only alphanumeric, hyphens, and underscores",
                full_name
            )));
        }
    }

    Ok(())
}

/// Atomically reset clone status to 'pending' if currently 'failed'.
///
/// Uses a single UPDATE query with WHERE clause for atomicity.
/// Only the first concurrent UPDATE will match 'clone_status = failed',
/// subsequent ones will see 'pending' and return false.
///
/// Returns `true` if the update succeeded (row was updated), `false` if no
/// rows matched (meaning status changed or another retry is in progress).
pub async fn reset_clone_status_if_failed(pool: &DbPool, full_name: &str) -> Result<bool> {
    // Execute UPDATE with WHERE clause for atomicity.
    // SQLite handles concurrent calls safely - only first UPDATE matches.
    let result = sqlx::query(
        r#"
        UPDATE repos 
        SET clone_status = ?1,
            clone_error = NULL,
            clone_attempts = clone_attempts + 1, 
            last_clone_attempt_at = datetime('now'),
            updated_at = datetime('now')
        WHERE full_name = ?2 AND clone_status = ?3
        "#,
    )
    .bind(CloneStatus::Pending.as_str())
    .bind(full_name)
    .bind(CloneStatus::Failed.as_str())
    .execute(pool)
    .await
    .map_err(|source| DbError::from_query("reset clone status", source))?;

    Ok(result.rows_affected() > 0)
}

/// Update a repository's clone status
pub async fn update_repo_clone_status(
    pool: &DbPool,
    full_name: &str,
    status: impl ToString,
    error: Option<&str>,
) -> Result<()> {
    let status = status.to_string();
    let parsed_status = status
        .parse::<CloneStatus>()
        .map_err(|_| DbError::InvalidCloneStatus(status.clone()))?;

    let result = sqlx::query(
        r#"
        UPDATE repos
        SET clone_status = ?1,
            clone_error = ?2,
            clone_attempts = clone_attempts + 1,
            last_clone_attempt_at = datetime('now'),
            updated_at = datetime('now')
        WHERE full_name = ?3
        "#,
    )
    .bind(parsed_status.as_str())
    .bind(error)
    .bind(full_name)
    .execute(pool)
    .await
    .map_err(|source| DbError::from_query("update repo clone status", source))?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound {
            entity: "Repo",
            key: full_name.to_string(),
        });
    }

    debug!(
        "Updated repo clone status: {} -> {}",
        full_name,
        parsed_status.as_str()
    );
    Ok(())
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
    .map_err(|source| DbError::from_query("update repo env_loader", source))?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound {
            entity: "Repo",
            key: full_name.to_string(),
        });
    }

    debug!("Updated repo env_loader: {} -> {}", full_name, env_loader);
    Ok(())
}

/// Delete a repository by its full name
pub async fn delete_repo(pool: &DbPool, full_name: &str) -> Result<()> {
    sqlx::query(
        r#"
        DELETE FROM repos WHERE full_name = ?1
        "#,
    )
    .bind(full_name)
    .execute(pool)
    .await
    .map_err(|source| DbError::from_query("delete repo", source))?;

    debug!("Deleted repo: {}", full_name);
    Ok(())
}

/// Get all sessions for a repository
pub async fn get_sessions_for_repo(pool: &DbPool, full_name: &str) -> Result<Vec<Session>> {
    let rows = sqlx::query(
        r#"
        SELECT id, repo_full_name, issue_id, pr_id, opencode_session_id,
               worktree_path, state, created_at, updated_at
        FROM sessions
        WHERE repo_full_name = ?1
        "#,
    )
    .bind(full_name)
    .fetch_all(pool)
    .await
    .map_err(|source| DbError::from_query("get sessions for repo", source))?;

    let sessions = rows
        .into_iter()
        .map(|row| map_session_row(&row))
        .collect::<Result<Vec<_>>>()?;

    Ok(sessions)
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
    .map_err(|source| DbError::from_query("insert session", source))?;

    debug!(
        "Inserted session: {} for repo {} issue {}",
        session.id, session.repo_full_name, session.issue_id
    );
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
    .map_err(|source| DbError::from_query("get session by issue", source))?;

    row.map(|row| map_session_row(&row)).transpose()
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
    .map_err(|source| DbError::from_query("get session by pr", source))?;

    row.map(|row| map_session_row(&row)).transpose()
}

/// Update a session's state
pub async fn update_session_state(
    pool: &DbPool,
    session_id: &str,
    state: impl ToString,
) -> Result<()> {
    let state = state.to_string();
    let parsed_state = state
        .parse::<SessionState>()
        .map_err(|_| DbError::InvalidSessionState(state.clone()))?;

    let result = sqlx::query(
        r#"
        UPDATE sessions
        SET state = ?1, updated_at = datetime('now')
        WHERE id = ?2
        "#,
    )
    .bind(parsed_state.as_str())
    .bind(session_id)
    .execute(pool)
    .await
    .map_err(|source| DbError::from_query("update session state", source))?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound {
            entity: "Session",
            key: session_id.to_string(),
        });
    }

    debug!(
        "Updated session state: {} -> {}",
        session_id,
        parsed_state.as_str()
    );
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
    .map_err(|source| DbError::from_query("update session pr id", source))?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound {
            entity: "Session",
            key: session_id.to_string(),
        });
    }

    debug!("Updated session PR ID: {} -> {}", session_id, pr_id);
    Ok(())
}

/// Get all sessions in the specified states
pub async fn get_sessions_in_state(pool: &DbPool, states: &[SessionState]) -> Result<Vec<Session>> {
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
        query = query.bind(state.as_str());
    }

    let rows = query
        .fetch_all(pool)
        .await
        .map_err(|source| DbError::from_query("get sessions by state", source))?;

    let sessions = rows
        .into_iter()
        .map(|row| map_session_row(&row))
        .collect::<Result<Vec<_>>>()?;

    Ok(sessions)
}

// ============================================================================
// Pending Worktree Operations
// ============================================================================

/// Add a worktree to the pending cleanup queue
pub async fn add_pending_worktree(
    pool: &DbPool,
    session_id: &str,
    worktree_path: &str,
) -> Result<()> {
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
    .map_err(|source| DbError::from_query("add pending worktree", source))?;

    debug!(
        "Added pending worktree: {} for session {}",
        worktree_path, session_id
    );
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
    .map_err(|source| DbError::from_query("list pending worktrees", source))?;

    let worktrees = rows
        .into_iter()
        .map(|row| map_pending_worktree_row(&row))
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
    .map_err(|source| DbError::from_query("remove pending worktree", source))?;

    if result.rows_affected() > 0 {
        debug!("Removed pending worktree for session: {}", session_id);
    }

    Ok(())
}

/// Update a session's opencode session ID
pub async fn update_session_opencode_id(
    pool: &DbPool,
    session_id: &str,
    opencode_session_id: &str,
) -> Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE sessions
        SET opencode_session_id = ?1, updated_at = datetime('now')
        WHERE id = ?2
        "#,
    )
    .bind(opencode_session_id)
    .bind(session_id)
    .execute(pool)
    .await
    .map_err(|source| DbError::from_query("update session opencode id", source))?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound {
            entity: "Session",
            key: session_id.to_string(),
        });
    }

    debug!(
        "Updated session opencode ID: {} -> {}",
        session_id, opencode_session_id
    );
    Ok(())
}
