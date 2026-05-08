use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tokio::sync::{Mutex, Semaphore, broadcast};

use crate::config::schema::{Config, SessionProfile};
use crate::providers::registry::create_provider;
use crate::providers::traits::Provider;
use crate::session::agent_loop::{Session, SessionEvent, SessionStatus};
use crate::session::message::Message;
use crate::store::session_store::{SessionId, SessionStore};
use crate::tools::traits::ToolRegistry;
use crate::worktree::{WorktreeInfo, WorktreeManager};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChildStatus {
    Queued,
    Running,
    Done,
    Error(String),
    Cancelled,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChildInfo {
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub profile: String,
    pub status: ChildStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tool: Option<String>,
    pub tokens: ChildTokens,
    pub elapsed_s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ChildTokens {
    pub input: u64,
    pub output: u64,
}

pub struct ChildHandle {
    pub id: SessionId,
    pub provider_name: String,
    pub model_name: String,
    pub profile_name: String,
    pub prompt: String,
    pub status: ChildStatus,
    pub output: Option<String>,
    pub tokens: ChildTokens,
    pub active_tool: Option<String>,
    pub started_at: Instant,
    pub worktree: Option<WorktreeInfo>,
    pub cancel_flag: Arc<AtomicBool>,
    pub events_tx: broadcast::Sender<SessionEvent>,
}

impl ChildHandle {
    fn to_info(&self) -> ChildInfo {
        ChildInfo {
            session_id: self.id.0.clone(),
            provider: self.provider_name.clone(),
            model: self.model_name.clone(),
            profile: self.profile_name.clone(),
            status: self.status.clone(),
            active_tool: self.active_tool.clone(),
            tokens: self.tokens.clone(),
            elapsed_s: self.started_at.elapsed().as_secs_f64(),
            output: self.output.clone(),
            worktree_branch: self.worktree.as_ref().map(|w| w.branch.clone()),
        }
    }
}

pub struct SpawnConfig {
    pub provider: Box<dyn Provider>,
    pub provider_name: String,
    pub model_name: String,
    pub profile: SessionProfile,
    pub profile_name: String,
    pub prompt: String,
    pub tools: ToolRegistry,
    pub store: Arc<SessionStore>,
    pub project: PathBuf,
    pub worktree: Option<WorktreeInfo>,
    pub context_files: Vec<String>,
}

pub struct SessionPool {
    children: Arc<Mutex<HashMap<String, ChildHandle>>>,
    semaphore: Arc<Semaphore>,
    pub worktrees: Option<WorktreeManager>,
}

impl SessionPool {
    pub fn new(max_concurrent: usize, worktrees: Option<WorktreeManager>) -> Self {
        Self {
            children: Arc::new(Mutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            worktrees,
        }
    }

    pub async fn spawn(&self, cfg: SpawnConfig) -> SessionId {
        let SpawnConfig {
            provider,
            provider_name,
            model_name,
            profile,
            profile_name,
            prompt,
            tools,
            store,
            project,
            worktree,
            context_files,
        } = cfg;
        let id = SessionId::new();
        let (events_tx, _) = broadcast::channel(256);
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let handle = ChildHandle {
            id: id.clone(),
            provider_name: provider_name.clone(),
            model_name: model_name.clone(),
            profile_name,
            prompt: prompt.clone(),
            status: ChildStatus::Queued,
            output: None,
            tokens: ChildTokens::default(),
            active_tool: None,
            started_at: Instant::now(),
            worktree: worktree.clone(),
            cancel_flag: Arc::clone(&cancel_flag),
            events_tx: events_tx.clone(),
        };

        self.children.lock().await.insert(id.0.clone(), handle);

        let children = Arc::clone(&self.children);
        let semaphore = Arc::clone(&self.semaphore);
        let child_id = id.0.clone();
        let id_for_session = id.clone();
        let work_dir = worktree
            .as_ref()
            .map(|w| w.path.clone())
            .unwrap_or_else(|| project.clone());

        tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();

            {
                let mut map = children.lock().await;
                if let Some(h) = map.get_mut(&child_id) {
                    h.status = ChildStatus::Running;
                    h.started_at = Instant::now();
                }
            }

            let child_span = crate::otel::spans::session_span(&child_id);
            let _guard = child_span.enter();
            tracing::info!(
                provider = %provider_name,
                model = %model_name,
                "child agent started",
            );

            let mut session = Session::new(id_for_session, profile);
            session.provider_name = provider_name;
            session.model_name = model_name;
            session.set_cancel_flag(cancel_flag);

            // Pre-load context files
            if !context_files.is_empty() {
                let mut context_block = String::new();
                for file_path in &context_files {
                    let read_path = work_dir.join(file_path);
                    if let Ok(content) = std::fs::read_to_string(&read_path) {
                        context_block.push_str(&format!("--- {file_path} ---\n{content}\n\n"));
                    }
                }
                if !context_block.is_empty() {
                    session.add_message(Message::user(format!(
                        "Here are the relevant files for context:\n\n{context_block}"
                    )));
                }
            }

            session.add_message(Message::user(&prompt));

            // Subscribe to session events to track active tool
            let mut event_rx = session.subscribe();
            let children_for_events = Arc::clone(&children);
            let child_id_for_events = child_id.clone();
            let events_tx_clone = events_tx.clone();

            let event_tracker = tokio::spawn(async move {
                while let Ok(event) = event_rx.recv().await {
                    match &event {
                        SessionEvent::ToolCallStart { name, .. } => {
                            let mut map = children_for_events.lock().await;
                            if let Some(h) = map.get_mut(&child_id_for_events) {
                                h.active_tool = Some(name.clone());
                            }
                        }
                        SessionEvent::ToolCallEnd { .. } => {
                            let mut map = children_for_events.lock().await;
                            if let Some(h) = map.get_mut(&child_id_for_events) {
                                h.active_tool = None;
                            }
                        }
                        _ => {}
                    }
                    let _ = events_tx_clone.send(event);
                }
            });

            session
                .run(provider.as_ref(), &tools, &store, &work_dir, &[])
                .await;

            event_tracker.abort();

            let final_output = session
                .messages
                .iter()
                .rev()
                .find(|m| m.role == crate::session::message::Role::Assistant)
                .map(|m| m.content.clone())
                .unwrap_or_default();

            {
                let mut map = children.lock().await;
                if let Some(h) = map.get_mut(&child_id) {
                    match &session.state {
                        SessionStatus::Done => {
                            h.status = ChildStatus::Done;
                        }
                        SessionStatus::Error(e) => {
                            h.status = ChildStatus::Error(e.clone());
                        }
                        SessionStatus::Cancelled => {
                            h.status = ChildStatus::Cancelled;
                        }
                        _ => {
                            h.status = ChildStatus::Done;
                        }
                    }
                    h.output = Some(final_output);
                    h.tokens = ChildTokens {
                        input: session.token_input,
                        output: session.token_output,
                    };
                    h.active_tool = None;

                    tracing::info!(
                        tokens_in = session.token_input,
                        tokens_out = session.token_output,
                        status = ?h.status,
                        "child agent completed",
                    );
                }
            }

            drop(_guard);
        });

        id
    }

    pub async fn check(&self, ids: Option<&[String]>) -> Vec<ChildInfo> {
        let children = self.children.lock().await;
        children
            .iter()
            .filter(|(id, _)| ids.is_none_or(|ids| ids.contains(id)))
            .map(|(_, child)| child.to_info())
            .collect()
    }

    pub async fn collect(&self, session_id: &str) -> Result<ChildInfo, String> {
        let children = self.children.lock().await;
        let child = children
            .get(session_id)
            .ok_or_else(|| format!("session not found: {session_id}"))?;
        match &child.status {
            ChildStatus::Done | ChildStatus::Error(_) => Ok(child.to_info()),
            other => Err(format!("session not finished: {other:?}")),
        }
    }

    pub async fn cancel(&self, session_id: &str) -> Result<(), String> {
        let mut children = self.children.lock().await;
        let child = children
            .get_mut(session_id)
            .ok_or_else(|| format!("session not found: {session_id}"))?;

        child.cancel_flag.store(true, Ordering::Relaxed);
        child.status = ChildStatus::Cancelled;

        if let (Some(wt), Some(mgr)) = (&child.worktree, &self.worktrees) {
            let _ = mgr.remove(&wt.child_id, true);
        }

        Ok(())
    }

    pub async fn cancel_all(&self) {
        let mut children = self.children.lock().await;
        for (_, child) in children.iter_mut() {
            child.cancel_flag.store(true, Ordering::Relaxed);
            child.status = ChildStatus::Cancelled;
            if let (Some(wt), Some(mgr)) = (&child.worktree, &self.worktrees) {
                let _ = mgr.remove(&wt.child_id, true);
            }
        }
    }

    pub fn resolve_provider(
        config: &Config,
        provider_name: Option<&str>,
        model_override: Option<&str>,
        fallback_provider: &str,
    ) -> Result<(Box<dyn Provider>, String, String), String> {
        let name = provider_name.unwrap_or(fallback_provider);
        let mut profile = config
            .providers
            .get(name)
            .cloned()
            .ok_or_else(|| format!("unknown provider: {name}"))?;

        if let Some(model) = model_override {
            profile.model = model.to_string();
        }

        let model_name = profile.model.clone();
        let provider =
            create_provider(name, &profile).map_err(|e| format!("provider error: {e}"))?;
        Ok((provider, name.to_string(), model_name))
    }
}

impl Default for SessionPool {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self::new(cpus * 2, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pool_starts_empty() {
        let pool = SessionPool::default();
        let status = pool.check(None).await;
        assert!(status.is_empty());
    }

    #[tokio::test]
    async fn cancel_nonexistent_fails() {
        let pool = SessionPool::default();
        let result = pool.cancel("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn collect_nonexistent_fails() {
        let pool = SessionPool::default();
        let result = pool.collect("nonexistent").await;
        assert!(result.is_err());
    }
}
