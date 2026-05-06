use std::path::PathBuf;

/// Errors produced by the store layer.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("I/O error: {source} (path: {path:?})")]
    Io {
        source: std::io::Error,
        path: Option<PathBuf>,
    },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("store file is locked: {path}")]
    Locked { path: PathBuf },
}

impl StoreError {
    /// Convenience: wrap an `io::Error` with an associated path.
    pub fn io(source: std::io::Error, path: impl Into<PathBuf>) -> Self {
        Self::Io {
            source,
            path: Some(path.into()),
        }
    }

    /// Convenience: wrap an `io::Error` without a path.
    pub fn io_no_path(source: std::io::Error) -> Self {
        Self::Io { source, path: None }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            source: e,
            path: None,
        }
    }
}

pub type StoreResult<T> = Result<T, StoreError>;
