use std::error::Error;
use std::fmt;

/// Shared fallback error type for top-level boundary mapping.
///
/// Internal modules should return typed module-local errors.
/// HTTP and process boundaries can map those typed errors into this enum,
/// then into user-facing responses or process exit behavior.
#[derive(Debug)]
pub enum AppError {
    Message(String),
    Source(Box<dyn Error + Send + Sync + 'static>),
}

impl AppError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    pub fn source(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Source(Box::new(source))
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => write!(f, "{}", message),
            Self::Source(source) => write!(f, "{}", source),
        }
    }
}

impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::Source(source) => Some(source.as_ref()),
        }
    }
}
