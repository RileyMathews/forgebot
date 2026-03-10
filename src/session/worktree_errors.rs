use std::error::Error;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum WorktreeError {
    BareCloneMissing(PathBuf),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    CommandExecution {
        operation: &'static str,
        source: std::io::Error,
    },
    CommandFailed {
        operation: &'static str,
        target: String,
        stderr: String,
    },
}

impl fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BareCloneMissing(path) => write!(
                f,
                "Bare clone does not exist at {}. Please clone the repository first.",
                path.display()
            ),
            Self::Io {
                operation,
                path,
                source,
            } => write!(f, "failed to {} {}: {}", operation, path.display(), source),
            Self::CommandExecution { operation, source } => {
                write!(f, "failed to execute {}: {}", operation, source)
            }
            Self::CommandFailed {
                operation,
                target,
                stderr,
            } => write!(f, "{} failed for {}: {}", operation, target, stderr),
        }
    }
}

impl Error for WorktreeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::CommandExecution { source, .. } => Some(source),
            Self::BareCloneMissing(_) | Self::CommandFailed { .. } => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, WorktreeError>;
