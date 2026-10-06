use std::path::PathBuf;

/// Errors the CLI and the app turn into translated messages.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("HOME is not set")]
    NoHome,
    #[error("unknown account: {0}")]
    UnknownAccount(String),
    #[error("an account named {0} already exists")]
    DuplicateAlias(String),
    #[error("{0} is already registered")]
    DuplicateEmail(String),
    #[error("no account is registered yet")]
    NoAccounts,
    #[error("the claude command was not found in PATH")]
    ClaudeNotFound,
    #[error("command failed: {0}")]
    Command(String),
    #[error("something that is not a link already exists at {0}")]
    Conflict(PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}
