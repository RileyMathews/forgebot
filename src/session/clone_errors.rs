use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use crate::db::errors::DbError;

#[derive(Debug)]
pub enum CloneError {
    InvalidRepoName(DbError),
    StatusUpdate {
        repo: String,
        status: &'static str,
        source: DbError,
    },
    BarePathMissingParent(PathBuf),
    CreateParentDir {
        path: PathBuf,
        source: std::io::Error,
    },
    ExecuteClone(std::io::Error),
    CloneFailed {
        repo: String,
        stderr: String,
    },
    Timeout {
        repo: String,
        seconds: u64,
    },
    IncompleteExistingClone(String),
}

impl fmt::Display for CloneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRepoName(source) => {
                write!(f, "repository name validation failed: {}", source)
            }
            Self::StatusUpdate {
                repo,
                status,
                source,
            } => write!(
                f,
                "Failed to set clone status to '{}' for {}: {}",
                status, repo, source
            ),
            Self::BarePathMissingParent(path) => {
                write!(f, "Bare clone path has no parent: {}", path.display())
            }
            Self::CreateParentDir { path, source } => {
                write!(
                    f,
                    "Failed to create parent directory {}: {}",
                    path.display(),
                    source
                )
            }
            Self::ExecuteClone(source) => {
                write!(f, "Failed to execute git clone command: {}", source)
            }
            Self::CloneFailed { repo, stderr } => {
                write!(f, "Clone failed for {}: {}", repo, stderr)
            }
            Self::Timeout { repo, seconds } => {
                write!(f, "Clone timeout for {} after {} seconds", repo, seconds)
            }
            Self::IncompleteExistingClone(repo) => write!(
                f,
                "Clone directory already exists for {} but appears incomplete",
                repo
            ),
        }
    }
}

impl Error for CloneError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidRepoName(source) => Some(source),
            Self::StatusUpdate { source, .. } => Some(source),
            Self::CreateParentDir { source, .. } => Some(source),
            Self::ExecuteClone(source) => Some(source),
            Self::BarePathMissingParent(_)
            | Self::CloneFailed { .. }
            | Self::Timeout { .. }
            | Self::IncompleteExistingClone(_) => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, CloneError>;
