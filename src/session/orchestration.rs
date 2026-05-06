use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::store::session_store::SessionId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildStatus {
    Queued,
    Running,
    Done,
    Error(String),
    Cancelled,
}

#[derive(Debug)]
pub struct ChildSession {
    pub id: SessionId,
    pub profile_name: String,
    pub prompt: String,
    pub status: ChildStatus,
    pub output: Option<String>,
}

pub struct SessionPool {
    children: Arc<Mutex<HashMap<String, ChildSession>>>,
}

impl SessionPool {
    pub fn new() -> Self {
        Self {
            children: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn spawn(&self, profile_name: String, prompt: String) -> SessionId {
        let id = SessionId::new();
        let child = ChildSession {
            id: id.clone(),
            profile_name,
            prompt,
            status: ChildStatus::Queued,
            output: None,
        };
        self.children.lock().await.insert(id.0.clone(), child);
        // TODO: actually spawn a worker task that runs the child session
        id
    }

    pub async fn check(&self, ids: Option<&[String]>) -> Vec<(String, ChildStatus, String)> {
        let children = self.children.lock().await;
        children
            .iter()
            .filter(|(id, _)| ids.is_none_or(|ids| ids.contains(id)))
            .map(|(id, child)| (id.clone(), child.status.clone(), child.profile_name.clone()))
            .collect()
    }

    pub async fn collect(&self, session_id: &str) -> Result<String, String> {
        let mut children = self.children.lock().await;
        let child = children
            .get(session_id)
            .ok_or_else(|| format!("session not found: {session_id}"))?;
        match &child.status {
            ChildStatus::Done => {
                let output = child.output.clone().unwrap_or_default();
                children.remove(session_id);
                Ok(output)
            }
            ChildStatus::Error(e) => {
                let err = e.clone();
                children.remove(session_id);
                Err(err)
            }
            other => Err(format!("session not finished: {other:?}")),
        }
    }

    pub async fn cancel(&self, session_id: &str) -> Result<(), String> {
        let mut children = self.children.lock().await;
        let child = children
            .get_mut(session_id)
            .ok_or_else(|| format!("session not found: {session_id}"))?;
        child.status = ChildStatus::Cancelled;
        Ok(())
    }
}

impl Default for SessionPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_and_check() {
        let pool = SessionPool::new();
        let id = pool.spawn("default".into(), "do stuff".into()).await;
        let status = pool.check(None).await;
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].0, id.0);
        assert_eq!(status[0].1, ChildStatus::Queued);
    }

    #[tokio::test]
    async fn cancel_session() {
        let pool = SessionPool::new();
        let id = pool.spawn("default".into(), "do stuff".into()).await;
        pool.cancel(&id.0).await.unwrap();
        let status = pool.check(None).await;
        assert_eq!(status[0].1, ChildStatus::Cancelled);
    }
}
