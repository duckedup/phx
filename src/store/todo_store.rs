use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::error::{StoreError, StoreResult};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Open,
    InProgress,
    Done,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: u64,
    pub title: String,
    pub status: TodoStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
}

/// An audit-trail entry recording a tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolLogEntry {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub result: serde_json::Value,
    pub timestamp_ms: i64,
}

/// On-disk representation of the combined todos + tool-log file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoreData {
    #[serde(default)]
    todos: Vec<Todo>,
    #[serde(default)]
    tool_log: Vec<ToolLogEntry>,
    /// Monotonically increasing counter used to assign `Todo::id`.
    #[serde(default)]
    next_id: u64,
}

// ---------------------------------------------------------------------------
// TodoStore
// ---------------------------------------------------------------------------

/// File-backed store for todos and the tool audit log.
///
/// All mutations acquire an **exclusive file lock** (via `fs2`) so that
/// concurrent processes (e.g. multiple phoenix sessions in the same project)
/// cannot corrupt the JSON file.  Reads acquire a **shared lock** so they
/// can proceed concurrently with each other but not with writes.
#[derive(Debug, Clone)]
pub struct TodoStore {
    path: PathBuf,
}

impl TodoStore {
    /// Create a new `TodoStore` persisting to `path` (typically
    /// `<data_dir>/todos.json`).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    // -- public API ---------------------------------------------------------

    /// Return all todos.
    pub fn list(&self) -> StoreResult<Vec<Todo>> {
        let data = self.read_locked()?;
        Ok(data.todos)
    }

    /// Insert or update a todo.
    ///
    /// If `todo.id == 0` a new id is assigned and the todo is inserted.
    /// Otherwise the existing todo with the same id is replaced; if no such
    /// todo exists, the new one is appended.
    ///
    /// Returns the (possibly updated) todo.
    pub fn upsert(&self, mut todo: Todo) -> StoreResult<Todo> {
        self.with_write_lock(|data| {
            if todo.id == 0 {
                data.next_id = data.next_id.max(1);
                todo.id = data.next_id;
                data.next_id += 1;
                data.todos.push(todo.clone());
            } else if let Some(existing) = data.todos.iter_mut().find(|t| t.id == todo.id) {
                *existing = todo.clone();
            } else {
                // New todo with a caller-specified id — track the counter.
                data.next_id = data.next_id.max(todo.id + 1);
                data.todos.push(todo.clone());
            }
            Ok(todo)
        })
    }

    /// Remove a todo by id.  Returns `true` if a todo was actually removed.
    pub fn remove(&self, id: u64) -> StoreResult<bool> {
        self.with_write_lock(|data| {
            let before = data.todos.len();
            data.todos.retain(|t| t.id != id);
            Ok(data.todos.len() < before)
        })
    }

    /// Append an entry to the tool audit log.
    pub fn append_tool_log(&self, entry: ToolLogEntry) -> StoreResult<()> {
        self.with_write_lock(|data| {
            data.tool_log.push(entry);
            Ok(())
        })
    }

    // -- internal -----------------------------------------------------------

    /// Read the store data under a **shared** lock.
    fn read_locked(&self) -> StoreResult<StoreData> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StoreData::default());
            }
            Err(e) => return Err(StoreError::io(e, &self.path)),
        };

        FileExt::lock_shared(&file).map_err(|_| StoreError::Locked {
            path: self.path.clone(),
        })?;

        let data = Self::read_data(&file, &self.path)?;

        FileExt::unlock(&file).map_err(|e| StoreError::io(e, &self.path))?;

        Ok(data)
    }

    /// Execute `f` while holding an exclusive lock on the store file.
    /// Reads the current contents, passes them to `f`, then writes the
    /// (possibly modified) contents back.
    fn with_write_lock<T>(
        &self,
        f: impl FnOnce(&mut StoreData) -> StoreResult<T>,
    ) -> StoreResult<T> {
        // Ensure parent directory exists.
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::io(e, parent))?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.path)
            .map_err(|e| StoreError::io(e, &self.path))?;

        FileExt::lock_exclusive(&file).map_err(|_| StoreError::Locked {
            path: self.path.clone(),
        })?;

        let mut data = Self::read_data(&file, &self.path)?;

        let result = f(&mut data)?;

        // Truncate and rewrite.
        file.set_len(0).map_err(|e| StoreError::io(e, &self.path))?;

        // Seek to start after truncation (set_len doesn't move the cursor on
        // all platforms, but serde_json::to_writer writes from the current
        // position so we use a fresh BufWriter from the beginning).
        use std::io::Seek;
        (&file)
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|e| StoreError::io(e, &self.path))?;

        let json = serde_json::to_string_pretty(&data)?;
        (&file)
            .write_all(json.as_bytes())
            .map_err(|e| StoreError::io(e, &self.path))?;
        (&file).flush().map_err(|e| StoreError::io(e, &self.path))?;
        file.sync_all().map_err(|e| StoreError::io(e, &self.path))?;

        FileExt::unlock(&file).map_err(|e| StoreError::io(e, &self.path))?;

        Ok(result)
    }

    /// Deserialize store data from an open file handle.  Returns the default
    /// if the file is empty.
    fn read_data(mut file: &File, path: &std::path::Path) -> StoreResult<StoreData> {
        let mut buf = String::new();
        file.read_to_string(&mut buf)
            .map_err(|e| StoreError::io(e, path))?;
        if buf.trim().is_empty() {
            return Ok(StoreData::default());
        }
        let data: StoreData = serde_json::from_str(&buf)?;
        Ok(data)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_upsert_list_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TodoStore::new(tmp.path().join("todos.json"));

        // Initially empty.
        assert!(store.list().unwrap().is_empty());

        // Insert.
        let t1 = store
            .upsert(Todo {
                id: 0,
                title: "buy milk".into(),
                status: TodoStatus::Open,
                assignee: None,
            })
            .unwrap();
        assert_eq!(t1.id, 1);

        let t2 = store
            .upsert(Todo {
                id: 0,
                title: "write tests".into(),
                status: TodoStatus::InProgress,
                assignee: Some("alice".into()),
            })
            .unwrap();
        assert_eq!(t2.id, 2);

        // List.
        let all = store.list().unwrap();
        assert_eq!(all.len(), 2);

        // Update.
        let updated = store
            .upsert(Todo {
                id: 1,
                title: "buy oat milk".into(),
                status: TodoStatus::Done,
                assignee: None,
            })
            .unwrap();
        assert_eq!(updated.id, 1);
        assert_eq!(updated.title, "buy oat milk");

        let all = store.list().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(
            all.iter().find(|t| t.id == 1).unwrap().status,
            TodoStatus::Done
        );

        // Remove.
        assert!(store.remove(1).unwrap());
        assert!(!store.remove(999).unwrap()); // no-op
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn append_tool_log() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TodoStore::new(tmp.path().join("todos.json"));

        store
            .append_tool_log(ToolLogEntry {
                session_id: "s1".into(),
                parent_session_id: None,
                tool_name: "read_file".into(),
                args: serde_json::json!({"path": "/etc/hosts"}),
                result: serde_json::json!({"ok": true}),
                timestamp_ms: 1_700_000_000_000,
            })
            .unwrap();

        // Verify it persisted by reading the raw file.
        let raw = std::fs::read_to_string(tmp.path().join("todos.json")).unwrap();
        let data: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(data["tool_log"].as_array().unwrap().len(), 1);
        assert_eq!(data["tool_log"][0]["tool_name"], "read_file");
    }

    #[test]
    fn concurrent_upserts_are_consistent() {
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("todos.json");
        let store = Arc::new(TodoStore::new(path));

        let mut handles = Vec::new();
        let n = 20;

        for i in 0..n {
            let s = Arc::clone(&store);
            handles.push(std::thread::spawn(move || {
                s.upsert(Todo {
                    id: 0,
                    title: format!("task-{i}"),
                    status: TodoStatus::Open,
                    assignee: None,
                })
                .unwrap()
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let all = store.list().unwrap();
        assert_eq!(all.len(), n);

        // All ids should be unique.
        let mut ids: Vec<u64> = all.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn empty_file_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("todos.json");
        // Create an empty file.
        std::fs::write(&path, "").unwrap();
        let store = TodoStore::new(&path);
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn nonexistent_file_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TodoStore::new(tmp.path().join("nope.json"));
        assert!(store.list().unwrap().is_empty());
    }
}
