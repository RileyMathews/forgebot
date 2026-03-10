use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum ForgejoError {
    BuildClient(reqwest::Error),
    Transport {
        operation: &'static str,
        source: reqwest::Error,
    },
    Parse {
        operation: &'static str,
        source: reqwest::Error,
    },
    ApiStatus {
        operation: &'static str,
        status: u16,
        reason: String,
        body: String,
    },
    NotFound {
        operation: &'static str,
        resource: String,
        body: String,
    },
}

impl ForgejoError {
    pub fn from_status(
        operation: &'static str,
        resource: impl Into<String>,
        status: reqwest::StatusCode,
        body: String,
    ) -> Self {
        if status == reqwest::StatusCode::NOT_FOUND {
            return Self::NotFound {
                operation,
                resource: resource.into(),
                body,
            };
        }

        Self::ApiStatus {
            operation,
            status: status.as_u16(),
            reason: status.canonical_reason().unwrap_or("Unknown").to_string(),
            body,
        }
    }
}

impl fmt::Display for ForgejoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuildClient(err) => write!(f, "failed to build forgejo http client: {}", err),
            Self::Transport { operation, source } => {
                write!(
                    f,
                    "forgejo transport error during {}: {}",
                    operation, source
                )
            }
            Self::Parse { operation, source } => {
                write!(f, "forgejo parse error during {}: {}", operation, source)
            }
            Self::ApiStatus {
                operation,
                status,
                reason,
                body,
            } => {
                write!(
                    f,
                    "forgejo api error during {}: {} {} - {}",
                    operation, status, reason, body
                )
            }
            Self::NotFound {
                operation,
                resource,
                body,
            } => write!(
                f,
                "forgejo resource not found during {}: {} - {}",
                operation, resource, body
            ),
        }
    }
}

impl Error for ForgejoError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BuildClient(err) => Some(err),
            Self::Transport { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::ApiStatus { .. } | Self::NotFound { .. } => None,
        }
    }
}

impl From<reqwest::Error> for ForgejoError {
    fn from(value: reqwest::Error) -> Self {
        Self::Transport {
            operation: "request",
            source: value,
        }
    }
}

pub type Result<T> = std::result::Result<T, ForgejoError>;
