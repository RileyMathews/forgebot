use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum EnvLoaderError {
    InvalidLoaderType(String),
    CommandExecution {
        command: &'static str,
        source: std::io::Error,
    },
    CommandFailed {
        command: &'static str,
        exit_code: Option<i32>,
        stderr: String,
    },
    ParseJson {
        loader: &'static str,
        output: String,
        source: serde_json::Error,
    },
    Timeout(u64),
}

impl fmt::Display for EnvLoaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLoaderType(value) => write!(
                f,
                "Invalid env_loader type: {}. Must be 'none', 'direnv', or 'nix'",
                value
            ),
            Self::CommandExecution { command, source } => {
                write!(f, "failed to execute '{}': {}", command, source)
            }
            Self::CommandFailed {
                command,
                exit_code,
                stderr,
            } => write!(
                f,
                "{} failed with exit code {:?}: {}",
                command, exit_code, stderr
            ),
            Self::ParseJson {
                loader,
                output,
                source,
            } => {
                write!(
                    f,
                    "failed to parse {} JSON output '{}': {}",
                    loader, output, source
                )
            }
            Self::Timeout(seconds) => write!(f, "command timed out after {} seconds", seconds),
        }
    }
}

impl Error for EnvLoaderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CommandExecution { source, .. } => Some(source),
            Self::ParseJson { source, .. } => Some(source),
            Self::InvalidLoaderType(_) | Self::CommandFailed { .. } | Self::Timeout(_) => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, EnvLoaderError>;
