use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use crate::session::errors::SessionError;

#[derive(Debug)]
pub enum DbError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidDatabasePath(PathBuf),
    Connect {
        path: PathBuf,
        source: sqlx::Error,
    },
    Migration(sqlx::migrate::MigrateError),
    Query {
        operation: &'static str,
        source: sqlx::Error,
    },
    ConstraintViolation {
        operation: &'static str,
        source: sqlx::Error,
    },
    ParseCloneStatus {
        value: String,
        source: SessionError,
    },
    ParseSessionState {
        value: String,
        source: SessionError,
    },
    InvalidRepoFullName(String),
    InvalidCloneStatus(String),
    InvalidSessionState(String),
    NotFound {
        entity: &'static str,
        key: String,
    },
}

impl DbError {
    pub fn from_query(operation: &'static str, source: sqlx::Error) -> Self {
        match &source {
            sqlx::Error::Database(db_err)
                if db_err.is_unique_violation() || db_err.is_foreign_key_violation() =>
            {
                Self::ConstraintViolation { operation, source }
            }
            _ => Self::Query { operation, source },
        }
    }
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                f,
                "database {} failed for {}: {}",
                operation,
                path.display(),
                source
            ),
            Self::InvalidDatabasePath(path) => {
                write!(f, "invalid database path (not UTF-8): {}", path.display())
            }
            Self::Connect { path, source } => {
                write!(
                    f,
                    "failed to connect to database {}: {}",
                    path.display(),
                    source
                )
            }
            Self::Migration(source) => write!(f, "failed to run database migrations: {}", source),
            Self::Query { operation, source } => {
                write!(f, "database query failed during {}: {}", operation, source)
            }
            Self::ConstraintViolation { operation, source } => write!(
                f,
                "database constraint violation during {}: {}",
                operation, source
            ),
            Self::ParseCloneStatus { value, source } => {
                write!(f, "failed to parse clone status '{}': {}", value, source)
            }
            Self::ParseSessionState { value, source } => {
                write!(f, "failed to parse session state '{}': {}", value, source)
            }
            Self::InvalidRepoFullName(message) => write!(f, "{}", message),
            Self::InvalidCloneStatus(value) => {
                write!(f, "invalid clone status '{}': expected known state", value)
            }
            Self::InvalidSessionState(value) => {
                write!(f, "invalid session state '{}': expected known state", value)
            }
            Self::NotFound { entity, key } => write!(f, "{} not found: {}", entity, key),
        }
    }
}

impl Error for DbError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Connect { source, .. } => Some(source),
            Self::Migration(source) => Some(source),
            Self::Query { source, .. } => Some(source),
            Self::ConstraintViolation { source, .. } => Some(source),
            Self::ParseCloneStatus { source, .. } => Some(source),
            Self::ParseSessionState { source, .. } => Some(source),
            Self::InvalidDatabasePath(_)
            | Self::InvalidRepoFullName(_)
            | Self::InvalidCloneStatus(_)
            | Self::InvalidSessionState(_)
            | Self::NotFound { .. } => None,
        }
    }
}

impl From<sqlx::migrate::MigrateError> for DbError {
    fn from(value: sqlx::migrate::MigrateError) -> Self {
        Self::Migration(value)
    }
}

pub type Result<T> = std::result::Result<T, DbError>;
