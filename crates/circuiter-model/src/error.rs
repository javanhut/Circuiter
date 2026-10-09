use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{}: {message}", path.display())]
    Parse { path: PathBuf, message: String },
    #[error("{0}")]
    Invalid(String),
}

impl ModelError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        ModelError::Io { path: path.into(), source }
    }

    pub(crate) fn parse(path: impl Into<PathBuf>, message: impl ToString) -> Self {
        ModelError::Parse { path: path.into(), message: message.to_string() }
    }
}

pub type Result<T, E = ModelError> = std::result::Result<T, E>;
