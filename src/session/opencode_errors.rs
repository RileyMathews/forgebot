use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use crate::db::errors::DbError;
use crate::forgejo::errors::ForgejoError;
use crate::session::env_loader_errors::EnvLoaderError;
use crate::session::worktree_errors::WorktreeError;

#[derive(Debug)]
pub enum OpencodeError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    Spawn {
        binary: String,
        resolved_path: String,
        source: std::io::Error,
    },
    ProcessFailed {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    SessionListFailed(String),
    SessionListParse {
        source: serde_json::Error,
        output: String,
    },
    SessionBusy {
        session_id: String,
        state: String,
    },
    MissingRepository(String),
    MissingCreatedSession(String),
    Database(DbError),
    Forgejo(ForgejoError),
    EnvLoader(EnvLoaderError),
    Worktree(WorktreeError),
}

impl fmt::Display for OpencodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(f, "failed to {} {}: {}", operation, path.display(), source),
            Self::Spawn {
                binary,
                resolved_path,
                source,
            } => write!(
                f,
                "Failed to spawn opencode process: {} (resolved to {}): {}",
                binary, resolved_path, source
            ),
            Self::ProcessFailed {
                exit_code,
                stdout,
                stderr,
            } => write!(
                f,
                "opencode process failed with exit code {}: stdout={}, stderr={}",
                exit_code, stdout, stderr
            ),
            Self::SessionListFailed(stderr) => {
                write!(f, "opencode session list failed: {}", stderr)
            }
            Self::SessionListParse { source, output } => write!(
                f,
                "Failed to parse opencode session list JSON from '{}': {}",
                output, source
            ),
            Self::SessionBusy { session_id, state } => {
                write!(f, "session {} is busy in state {}", session_id, state)
            }
            Self::MissingRepository(repo) => write!(f, "Repository {} not found in database", repo),
            Self::MissingCreatedSession(repo) => {
                write!(f, "Failed to retrieve newly created session for {}", repo)
            }
            Self::Database(source) => write!(f, "database error: {}", source),
            Self::Forgejo(source) => write!(f, "forgejo error: {}", source),
            Self::EnvLoader(source) => write!(f, "env loader error: {}", source),
            Self::Worktree(source) => write!(f, "worktree error: {}", source),
        }
    }
}

impl Error for OpencodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Spawn { source, .. } => Some(source),
            Self::SessionListParse { source, .. } => Some(source),
            Self::Database(source) => Some(source),
            Self::Forgejo(source) => Some(source),
            Self::EnvLoader(source) => Some(source),
            Self::Worktree(source) => Some(source),
            Self::ProcessFailed { .. }
            | Self::SessionListFailed(_)
            | Self::SessionBusy { .. }
            | Self::MissingRepository(_)
            | Self::MissingCreatedSession(_) => None,
        }
    }
}

impl From<DbError> for OpencodeError {
    fn from(value: DbError) -> Self {
        Self::Database(value)
    }
}

impl From<ForgejoError> for OpencodeError {
    fn from(value: ForgejoError) -> Self {
        Self::Forgejo(value)
    }
}

impl From<EnvLoaderError> for OpencodeError {
    fn from(value: EnvLoaderError) -> Self {
        Self::EnvLoader(value)
    }
}

impl From<WorktreeError> for OpencodeError {
    fn from(value: WorktreeError) -> Self {
        Self::Worktree(value)
    }
}

pub type Result<T> = std::result::Result<T, OpencodeError>;
