use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::error::{StoreError, StoreResult};

// ---------------------------------------------------------------------------
// SessionId
// ---------------------------------------------------------------------------

/// A session identifier wrapping a UUID v7 string.
///
/// UUID v7 is time-ordered, so lexicographic sort on the string representation
/// yields chronological order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl SessionId {
    /// Generate a new random session id (UUID v7).
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

// ---------------------------------------------------------------------------
// SessionState
// ---------------------------------------------------------------------------

/// Persisted metadata for a single session (lives in `state.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub id: SessionId,
    pub display_name: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub token_input: u64,
    #[serde(default)]
    pub token_output: u64,
}

// ---------------------------------------------------------------------------
// SessionStore
// ---------------------------------------------------------------------------

/// Filesystem-backed session store.
///
/// Layout under `root`:
/// ```text
/// {root}/{project}/{session_id}/state.json
/// {root}/{project}/{session_id}/messages.jsonl
/// ```
///
/// `project` is a caller-supplied slug (or path component) that partitions
/// sessions per working directory.
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    /// Create a new store rooted at the given directory (typically
    /// `paths::sessions_dir()`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    // -- path helpers -------------------------------------------------------

    fn project_key(project: &Path) -> PathBuf {
        let stripped = project.strip_prefix("/").unwrap_or(project);
        PathBuf::from(stripped.to_string_lossy().replace('/', "_"))
    }

    fn project_dir(&self, project: &Path) -> PathBuf {
        self.root.join(Self::project_key(project))
    }

    fn session_dir(&self, project: &Path, id: &SessionId) -> PathBuf {
        self.root.join(Self::project_key(project)).join(id.as_str())
    }

    fn state_path(&self, project: &Path, id: &SessionId) -> PathBuf {
        self.session_dir(project, id).join("state.json")
    }

    fn messages_path(&self, project: &Path, id: &SessionId) -> PathBuf {
        self.session_dir(project, id).join("messages.jsonl")
    }

    // -- public API ---------------------------------------------------------

    /// Create a new session: make its directory and write the initial
    /// `state.json`. The messages file is **not** created yet (it appears on
    /// the first `append_message`).
    pub async fn create(&self, project: impl AsRef<Path>, state: &SessionState) -> StoreResult<()> {
        let project = project.as_ref();
        let dir = self.session_dir(project, &state.id);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| StoreError::io(e, &dir))?;

        self.write_state_atomic(project, state).await
    }

    /// Append a single message (arbitrary JSON value) to the session's JSONL
    /// log.  The write is followed by `fsync` so that the data is durable
    /// before we return (DESIGN.md section 5.1A).
    pub async fn append_message(
        &self,
        project: impl AsRef<Path>,
        id: &SessionId,
        msg: &serde_json::Value,
    ) -> StoreResult<()> {
        let path = self.messages_path(project.as_ref(), id);

        // Ensure parent directory exists (idempotent).
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| StoreError::io(e, parent))?;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| StoreError::io(e, &path))?;

        let mut line = serde_json::to_string(msg)?;
        line.push('\n');

        file.write_all(line.as_bytes())
            .await
            .map_err(|e| StoreError::io(e, &path))?;

        // fsync for durability.
        file.flush().await.map_err(|e| StoreError::io(e, &path))?;
        file.sync_all()
            .await
            .map_err(|e| StoreError::io(e, &path))?;

        Ok(())
    }

    /// Atomically rewrite `state.json` for the given session.
    ///
    /// Writes to a temporary file in the same directory, then renames.
    pub async fn update_state(
        &self,
        project: impl AsRef<Path>,
        state: &SessionState,
    ) -> StoreResult<()> {
        self.write_state_atomic(project.as_ref(), state).await
    }

    /// List all sessions for a project, sorted by `updated_at` descending
    /// (newest first).  Returns an empty vec if the project directory does not
    /// exist.
    pub async fn list(&self, project: impl AsRef<Path>) -> StoreResult<Vec<SessionState>> {
        let dir = self.project_dir(project.as_ref());

        let mut read_dir = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::io(e, &dir)),
        };

        let mut states: Vec<SessionState> = Vec::new();

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| StoreError::io(e, &dir))?
        {
            let ft = entry
                .file_type()
                .await
                .map_err(|e| StoreError::io(e, entry.path()))?;
            if !ft.is_dir() {
                continue;
            }

            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') {
                continue;
            }

            let state_file = entry.path().join("state.json");
            match tokio::fs::read(&state_file).await {
                Ok(data) => {
                    if let Ok(st) = serde_json::from_slice::<SessionState>(&data) {
                        states.push(st);
                    }
                    // Silently skip corrupt state files.
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Directory exists but no state.json yet -- skip.
                    continue;
                }
                Err(e) => return Err(StoreError::io(e, &state_file)),
            }
        }

        // Sort newest first.
        states.sort_by_key(|s| std::cmp::Reverse(s.updated_at));

        Ok(states)
    }

    /// Load all messages for a session.  Returns them in file order (oldest
    /// first).
    pub async fn load_messages(
        &self,
        project: impl AsRef<Path>,
        id: &SessionId,
    ) -> StoreResult<Vec<serde_json::Value>> {
        let path = self.messages_path(project.as_ref(), id);

        let data = match tokio::fs::read_to_string(&path).await {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::NotFound(format!(
                    "messages file not found: {}",
                    path.display()
                )));
            }
            Err(e) => return Err(StoreError::io(e, &path)),
        };

        let mut messages = Vec::new();
        for line in data.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let val: serde_json::Value = serde_json::from_str(trimmed)?;
            messages.push(val);
        }

        Ok(messages)
    }

    /// Remove the entire session directory.
    pub async fn destroy(&self, project: impl AsRef<Path>, id: &SessionId) -> StoreResult<()> {
        let dir = self.session_dir(project.as_ref(), id);
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::io(e, &dir)),
        }
    }

    // -- internal -----------------------------------------------------------

    /// Write `state.json` atomically: write to a temp file in the same
    /// directory, fsync, then rename over the target.
    async fn write_state_atomic(&self, project: &Path, state: &SessionState) -> StoreResult<()> {
        let target = self.state_path(project, &state.id);
        let dir = self.session_dir(project, &state.id);

        // Ensure dir exists (no-op if already present).
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| StoreError::io(e, &dir))?;

        let tmp_path = dir.join(".state.json.tmp");

        let json = serde_json::to_string_pretty(state)?;

        tokio::fs::write(&tmp_path, json.as_bytes())
            .await
            .map_err(|e| StoreError::io(e, &tmp_path))?;

        // fsync the temp file before rename.
        fsync_path(&tmp_path).await?;

        tokio::fs::rename(&tmp_path, &target)
            .await
            .map_err(|e| StoreError::io(e, &target))?;

        Ok(())
    }
}

/// Open a file read-only and fsync it (ensures data written by a previous
/// handle is durable).
async fn fsync_path(path: &Path) -> StoreResult<()> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| StoreError::io(e, path))?;
    file.sync_all().await.map_err(|e| StoreError::io(e, path))?;
    Ok(())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_state(id: &str, updated_at_secs: i64) -> SessionState {
        SessionState {
            id: SessionId(id.to_string()),
            display_name: format!("session-{id}"),
            provider: "test".into(),
            model: "test-model".into(),
            created_at: Utc.timestamp_opt(1_000_000, 0).unwrap(),
            updated_at: Utc.timestamp_opt(updated_at_secs, 0).unwrap(),
            token_input: 0,
            token_output: 0,
        }
    }

    #[tokio::test]
    async fn round_trip_create_append_load() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(tmp.path());
        let project = "test-project";

        let state = make_state("sess-001", 1_700_000_000);
        store.create(project, &state).await.unwrap();

        // Append two messages.
        let m1 = serde_json::json!({"role": "user", "content": "hello"});
        let m2 = serde_json::json!({"role": "assistant", "content": "hi there"});
        store.append_message(project, &state.id, &m1).await.unwrap();
        store.append_message(project, &state.id, &m2).await.unwrap();

        // Load messages back.
        let loaded = store.load_messages(project, &state.id).await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0]["role"], "user");
        assert_eq!(loaded[0]["content"], "hello");
        assert_eq!(loaded[1]["role"], "assistant");
        assert_eq!(loaded[1]["content"], "hi there");
    }

    #[tokio::test]
    async fn list_returns_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(tmp.path());
        let project = "proj";

        // Create three sessions with different updated_at timestamps.
        let s1 = make_state("aaa", 1_000);
        let s2 = make_state("bbb", 3_000);
        let s3 = make_state("ccc", 2_000);

        store.create(project, &s1).await.unwrap();
        store.create(project, &s2).await.unwrap();
        store.create(project, &s3).await.unwrap();

        let list = store.list(project).await.unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].id.as_str(), "bbb"); // newest
        assert_eq!(list[1].id.as_str(), "ccc");
        assert_eq!(list[2].id.as_str(), "aaa"); // oldest
    }

    #[tokio::test]
    async fn list_missing_project_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(tmp.path());
        let list = store.list("nonexistent").await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn destroy_removes_session() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(tmp.path());
        let project = "proj";

        let state = make_state("doomed", 1_000);
        store.create(project, &state).await.unwrap();

        // Verify directory exists.
        let session_path = tmp.path().join(project).join("doomed");
        assert!(session_path.join("state.json").exists());

        store.destroy(project, &state.id).await.unwrap();

        // Directory should be gone.
        assert!(!session_path.exists());
    }

    #[tokio::test]
    async fn destroy_nonexistent_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(tmp.path());
        let id = SessionId("ghost".into());
        // Should not error.
        store.destroy("proj", &id).await.unwrap();
    }

    #[tokio::test]
    async fn update_state_is_atomic() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(tmp.path());
        let project = "proj";

        let mut state = make_state("up", 1_000);
        store.create(project, &state).await.unwrap();

        // Mutate and update.
        state.display_name = "updated name".into();
        state.token_input = 42;
        store.update_state(project, &state).await.unwrap();

        // Read back via list.
        let list = store.list(project).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].display_name, "updated name");
        assert_eq!(list[0].token_input, 42);

        // No leftover temp file.
        assert!(
            !tmp.path()
                .join("proj")
                .join("up")
                .join(".state.json.tmp")
                .exists()
        );
    }

    #[tokio::test]
    async fn load_messages_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(tmp.path());
        let id = SessionId("nope".into());
        let result = store.load_messages("proj", &id).await;
        assert!(matches!(result, Err(StoreError::NotFound(_))));
    }

    #[tokio::test]
    async fn session_id_generates_valid_uuid_v7() {
        let id = SessionId::new();
        // Should parse as a valid UUID.
        let parsed = uuid::Uuid::parse_str(id.as_str()).unwrap();
        assert_eq!(parsed.get_version(), Some(uuid::Version::SortRand));
    }
}
