use std::path::{Path, PathBuf};

#[cfg(test)]
use std::sync::Mutex;

#[cfg(test)]
static HOME_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Override the home directory for tests. Only available in `#[cfg(test)]`.
#[cfg(test)]
pub fn set_home_override(path: impl Into<PathBuf>) {
    *HOME_OVERRIDE.lock().unwrap() = Some(path.into());
}

/// Clear the test home override.
#[cfg(test)]
pub fn clear_home_override() {
    *HOME_OVERRIDE.lock().unwrap() = None;
}

/// Returns the user home directory (`~`).
pub fn user_home() -> PathBuf {
    home_dir()
}

/// Returns the phx home directory: `~/.phx`.
pub fn config_dir() -> PathBuf {
    home_dir().join(".phx")
}

/// Returns the user-level config file: `~/.phx/phx.json`.
pub fn user_config_file() -> PathBuf {
    config_dir().join("phx.json")
}

/// Returns the project-level config file: `.phx/phx.json` relative to
/// the current working directory.
pub fn project_config_file() -> PathBuf {
    PathBuf::from(".phx").join("phx.json")
}

/// Returns the phx data directory: `~/.phx`.
pub fn data_dir() -> PathBuf {
    config_dir()
}

/// Returns the sessions directory: `~/.phx/sessions`.
pub fn sessions_dir() -> PathBuf {
    config_dir().join("sessions")
}

/// Returns a project-specific sessions directory derived from the cwd basename:
/// `~/.phx/sessions/<basename>`.
///
/// If the cwd has no file name component (e.g. `/`), the directory name
/// `"_root"` is used instead.
pub fn project_sessions_dir(cwd: &Path) -> PathBuf {
    let basename = cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "_root".into());
    sessions_dir().join(basename)
}

/// Returns the runtime directory.
///
/// Tries `$XDG_RUNTIME_DIR` first, then falls back to a platform-specific temp path.
pub fn runtime_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg);
    }
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/phx-{uid}"))
    }
    #[cfg(windows)]
    {
        std::env::temp_dir().join("phx")
    }
}

/// Returns the RPC socket path: `<runtime_dir>/phx.sock`.
pub fn rpc_socket_path() -> PathBuf {
    runtime_dir().join("phx.sock")
}

/// Returns the logs directory: `~/.phx/logs`.
pub fn logs_dir() -> PathBuf {
    config_dir().join("logs")
}

/// Returns the history file path: `~/.phx/history`.
pub fn history_file() -> PathBuf {
    config_dir().join("history")
}

/// Returns the user-level agents directory: `~/.phx/agents`.
pub fn user_agents_dir() -> PathBuf {
    config_dir().join("agents")
}

/// Returns the project-level agents directory: `.phx/agents` relative to
/// the given project root.
pub fn project_agents_dir(project: &Path) -> PathBuf {
    project.join(".phx/agents")
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn home_dir() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(ref p) = *HOME_OVERRIDE.lock().unwrap() {
            return p.clone();
        }
    }
    dirs::home_dir().expect("could not determine home directory")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_ends_with_phx() {
        let d = config_dir();
        assert!(
            d.ends_with(".phx"),
            "expected path ending in .phx, got {:?}",
            d
        );
    }

    #[test]
    fn user_config_file_is_json() {
        let f = user_config_file();
        assert_eq!(f.file_name().unwrap(), "phx.json");
    }

    #[test]
    fn project_config_file_is_relative() {
        let f = project_config_file();
        assert!(f.is_relative());
        assert_eq!(f, PathBuf::from(".phx/phx.json"));
    }

    #[test]
    fn sessions_dir_under_config() {
        let s = sessions_dir();
        assert!(
            s.ends_with(".phx/sessions"),
            "expected path ending in .phx/sessions, got {:?}",
            s
        );
    }

    #[test]
    fn project_sessions_dir_uses_basename() {
        let dir = project_sessions_dir(Path::new("/home/user/my-project"));
        assert!(
            dir.ends_with("my-project"),
            "expected path ending in my-project, got {:?}",
            dir
        );
    }

    #[test]
    fn project_sessions_dir_root_fallback() {
        let dir = project_sessions_dir(Path::new("/"));
        assert!(
            dir.ends_with("_root"),
            "expected path ending in _root, got {:?}",
            dir
        );
    }

    #[test]
    fn rpc_socket_path_is_sock() {
        let p = rpc_socket_path();
        assert_eq!(p.file_name().unwrap(), "phx.sock");
    }

    #[test]
    fn history_file_path() {
        let h = history_file();
        assert_eq!(h.file_name().unwrap(), "history");
        assert!(
            h.ends_with(".phx/history"),
            "expected path ending in .phx/history, got {:?}",
            h
        );
    }

    #[test]
    fn home_override_works() {
        let tmp = tempfile::tempdir().unwrap();
        set_home_override(tmp.path());

        let d = config_dir();
        assert_eq!(d, tmp.path().join(".phx"));

        clear_home_override();
    }

    #[test]
    fn runtime_dir_fallback() {
        // When XDG_RUNTIME_DIR is not set, we should get /tmp/phx-<uid>
        let rd = runtime_dir();
        // Just verify it's a valid path; the exact value depends on env
        assert!(!rd.as_os_str().is_empty());
    }

    #[test]
    fn data_dir_equals_config_dir() {
        assert_eq!(data_dir(), config_dir());
    }
}
