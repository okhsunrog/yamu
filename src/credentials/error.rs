use std::{io, path::PathBuf};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("could not determine a platform-specific state directory")]
    StateDirectoryUnavailable,

    #[error("invalid credential profile name: {0:?}")]
    InvalidProfile(String),

    #[error("credential profile {profile:?} does not exist at {path}")]
    ProfileNotFound { profile: String, path: PathBuf },

    #[error("credential profile {profile:?} already exists at {path}")]
    ProfileAlreadyExists { profile: String, path: PathBuf },

    #[error("unsupported credential file version {found}; expected {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },

    #[cfg(unix)]
    #[error("credential file {path} has insecure permissions {mode:o}; expected 600")]
    InsecurePermissions { path: PathBuf, mode: u32 },

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid credential JSON at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("system clock is before the Unix epoch")]
    InvalidSystemClock,

    #[error("credential profile {0:?} has no refresh token")]
    MissingRefreshToken(String),

    #[error(transparent)]
    Api(#[from] crate::Error),

    #[error("profile lock worker failed: {0}")]
    LockWorker(#[from] tokio::task::JoinError),
}

pub type Result<T> = std::result::Result<T, Error>;
