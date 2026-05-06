use std::path::PathBuf;

/// Errors that can occur during config loading, parsing, or saving.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse config: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("no provider configured")]
    MissingProvider,

    #[error("schema error: {0}")]
    Schema(String),
}

impl ConfigError {
    /// Convenience for wrapping an `io::Error` with the offending path.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(err: std::io::Error) -> Self {
        Self::Io {
            path: PathBuf::from("<unknown>"),
            source: err,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_displays_path() {
        let err = ConfigError::io(
            "/tmp/bad.json",
            std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"),
        );
        let msg = format!("{err}");
        assert!(msg.contains("/tmp/bad.json"), "message was: {msg}");
        assert!(msg.contains("file not found"), "message was: {msg}");
    }

    #[test]
    fn parse_error_from_serde() {
        let bad_json = "{ not json }";
        let serde_err = serde_json::from_str::<serde_json::Value>(bad_json).unwrap_err();
        let cfg_err: ConfigError = serde_err.into();
        let msg = format!("{cfg_err}");
        assert!(msg.contains("parse"), "message was: {msg}");
    }

    #[test]
    fn missing_provider_display() {
        let err = ConfigError::MissingProvider;
        assert_eq!(format!("{err}"), "no provider configured");
    }

    #[test]
    fn schema_error_display() {
        let err = ConfigError::Schema("bad field".into());
        assert!(format!("{err}").contains("bad field"));
    }
}
