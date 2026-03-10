use std::error::Error;
use std::fmt;

use crate::db::errors::DbError;
use crate::forgejo::errors::ForgejoError;
use crate::session::worktree_errors::WorktreeError;

#[derive(Debug)]
pub enum RepoCleanupError {
    Database(DbError),
    Forgejo(ForgejoError),
    Worktree(WorktreeError),
    ActiveSessions(String),
}

impl fmt::Display for RepoCleanupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(source) => write!(f, "database cleanup error: {}", source),
            Self::Forgejo(source) => write!(f, "forgejo cleanup error: {}", source),
            Self::Worktree(source) => write!(f, "worktree cleanup error: {}", source),
            Self::ActiveSessions(repo) => write!(
                f,
                "cannot delete repository {}: has active sessions in planning/building/revising state",
                repo
            ),
        }
    }
}

impl Error for RepoCleanupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(source) => Some(source),
            Self::Forgejo(source) => Some(source),
            Self::Worktree(source) => Some(source),
            Self::ActiveSessions(_) => None,
        }
    }
}

impl From<DbError> for RepoCleanupError {
    fn from(value: DbError) -> Self {
        Self::Database(value)
    }
}

impl From<ForgejoError> for RepoCleanupError {
    fn from(value: ForgejoError) -> Self {
        Self::Forgejo(value)
    }
}

impl From<WorktreeError> for RepoCleanupError {
    fn from(value: WorktreeError) -> Self {
        Self::Worktree(value)
    }
}

pub type Result<T> = std::result::Result<T, RepoCleanupError>;
