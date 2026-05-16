use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tokio::sync::{Mutex, Semaphore, broadcast};

use crate::config::schema::{Config, SessionProfile};
use crate::providers::registry::create_provider;
use crate::providers::traits::Provider;
use crate::session::agent_loop::{Session, SessionEvent};
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
    pub task: String,
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

fn task_label(prompt: &str, max_len: usize) -> String {
    let first_line = prompt.lines().next().unwrap_or(prompt);
    let trimmed = first_line.trim();
    if trimmed.chars().count() <= max_len {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(max_len - 1).collect();
        format!("{truncated}…")
    }
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
    pub inject_tx: tokio::sync::mpsc::UnboundedSender<String>,
}

impl ChildHandle {
    fn to_info(&self) -> ChildInfo {
        ChildInfo {
            session_id: self.id.0.clone(),
            task: task_label(&self.prompt, 80),
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

    fn to_sidebar_info(&self) -> ChildInfo {
        ChildInfo {
            session_id: self.id.0.clone(),
            task: task_label(&self.prompt, 80),
            provider: self.provider_name.clone(),
            model: self.model_name.clone(),
            profile: self.profile_name.clone(),
            status: self.status.clone(),
            active_tool: self.active_tool.clone(),
            tokens: self.tokens.clone(),
            elapsed_s: self.started_at.elapsed().as_secs_f64(),
            output: None,
            worktree_branch: None,
        }
    }
}

pub struct SpawnConfig {
    pub provider: Arc<dyn Provider>,
    pub provider_name: String,
    pub model_name: String,
    pub profile: SessionProfile,
    pub profile_name: String,
    pub prompt: String,
    pub tools: Arc<parking_lot::RwLock<ToolRegistry>>,
    pub store: Arc<SessionStore>,
    pub project: PathBuf,
    pub worktree: Option<WorktreeInfo>,
    pub context_files: Vec<String>,
    pub config: Config,
    pub system_prompt_override: Option<String>,
}

pub struct AgentSpawned {
    pub session_id: String,
    pub task: String,
    pub provider: String,
    pub model: String,
    pub conv_rx: tokio::sync::mpsc::UnboundedReceiver<crate::session::conversation::ConvEvent>,
}

#[derive(Clone, Debug)]
pub struct AgentDone {
    pub session_id: String,
    pub success: bool,
}

pub struct SessionPool {
    children: Arc<Mutex<HashMap<String, ChildHandle>>>,
    semaphore: Arc<Semaphore>,
    pub worktrees: Option<WorktreeManager>,
    spawned_tx: tokio::sync::mpsc::UnboundedSender<AgentSpawned>,
    spawned_rx: Mutex<tokio::sync::mpsc::UnboundedReceiver<AgentSpawned>>,
    done_tx: tokio::sync::broadcast::Sender<AgentDone>,
}

impl SessionPool {
    pub fn new(max_concurrent: usize, worktrees: Option<WorktreeManager>) -> Self {
        let (spawned_tx, spawned_rx) = tokio::sync::mpsc::unbounded_channel();
        let (done_tx, _) = tokio::sync::broadcast::channel(64);
        Self {
            children: Arc::new(Mutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            worktrees,
            spawned_tx,
            spawned_rx: Mutex::new(spawned_rx),
            done_tx,
        }
    }

    pub fn subscribe_done(&self) -> tokio::sync::broadcast::Receiver<AgentDone> {
        self.done_tx.subscribe()
    }

    pub fn drain_spawned(&self) -> Vec<AgentSpawned> {
        let mut result = Vec::new();
        if let Ok(mut rx) = self.spawned_rx.try_lock() {
            while let Ok(spawned) = rx.try_recv() {
                result.push(spawned);
            }
        }
        result
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
            config,
            system_prompt_override,
        } = cfg;
        let id = SessionId::new();
        let (events_tx, _) = broadcast::channel(256);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let (inject_tx, _inject_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

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
            inject_tx,
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

        let spawned_tx = self.spawned_tx.clone();
        let task = task_label(&prompt, 24);

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
            session.provider_name = provider_name.clone();
            session.model_name = model_name.clone();
            session.set_cancel_flag(Arc::clone(&cancel_flag));

            if !context_files.is_empty() {
                let mut context_block = String::new();
                for file_path in &context_files {
                    let read_path = work_dir.join(file_path);
                    if let Ok(content) = tokio::fs::read_to_string(&read_path).await {
                        context_block.push_str(&format!("--- {file_path} ---\n{content}\n\n"));
                    }
                }
                if !context_block.is_empty() {
                    session.add_message(Message::user(format!(
                        "Here are the relevant files for context:\n\n{context_block}"
                    )));
                }
            }

            use crate::session::conversation::ConvConfig;

            let tool_router = crate::session::tool_router::ToolRouter::from_config(&config);
            let conv_cfg = ConvConfig {
                provider,
                tools,
                store: (*store).clone(),
                project: work_dir,
                config,
                system_prompt_override,
                plugin_runtime: None,
                tool_router,
            };
            let conv_rx = crate::session::conversation::spawn_conversation(
                session,
                prompt,
                conv_cfg,
                cancel_flag,
            );

            let _ = spawned_tx.send(AgentSpawned {
                session_id: child_id.clone(),
                task,
                provider: provider_name,
                model: model_name,
                conv_rx,
            });

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

    pub fn try_check_filtered(&self, ids: Option<&[String]>) -> Option<Vec<ChildInfo>> {
        let children = self.children.try_lock().ok()?;
        Some(
            children
                .iter()
                .filter(|(id, _)| ids.is_none_or(|ids| ids.contains(id)))
                .map(|(_, child)| child.to_info())
                .collect(),
        )
    }

    pub fn mark_done(&self, session_id: &str, success: bool) {
        let mut worktree_child_id = None;
        if let Ok(mut children) = self.children.try_lock()
            && let Some(handle) = children.get_mut(session_id)
        {
            handle.status = if success {
                ChildStatus::Done
            } else {
                ChildStatus::Error("cancelled".into())
            };
            if success && let Some(wt) = &handle.worktree {
                worktree_child_id = Some(wt.child_id.clone());
            }
        }
        if let (Some(child_id), Some(mgr)) = (worktree_child_id, &self.worktrees) {
            let mgr = mgr.clone();
            tokio::spawn(async move {
                let msg = format!("phx: agent {child_id} — completed");
                if let Err(e) = mgr.auto_commit(&child_id, &msg).await {
                    tracing::warn!("auto-commit failed for {child_id}: {e}");
                }
            });
        }
        let _ = self.done_tx.send(AgentDone {
            session_id: session_id.to_string(),
            success,
        });
    }

    pub fn try_check(&self) -> Option<Vec<ChildInfo>> {
        let children = self.children.try_lock().ok()?;
        Some(
            children
                .values()
                .map(|child| child.to_sidebar_info())
                .collect(),
        )
    }

    pub fn try_send_message(&self, session_id: &str, text: &str) -> bool {
        let Some(children) = self.children.try_lock().ok() else {
            return false;
        };
        let Some(child) = children.get(session_id) else {
            return false;
        };
        child.inject_tx.send(text.to_string()).is_ok()
    }

    pub async fn collect(&self, session_id: &str) -> Result<ChildInfo, String> {
        let children = self.children.lock().await;
        let child = children
            .get(session_id)
            .ok_or_else(|| format!("session not found: {session_id}"))?;
        match &child.status {
            ChildStatus::Done | ChildStatus::Error(_) | ChildStatus::Cancelled => {
                Ok(child.to_info())
            }
            other => Err(format!("session not finished: {other:?}")),
        }
    }

    pub fn try_cancel(&self, session_id: &str) {
        if let Ok(mut children) = self.children.try_lock()
            && let Some(child) = children.get_mut(session_id)
        {
            child.cancel_flag.store(true, Ordering::Relaxed);
            child.status = ChildStatus::Cancelled;
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
            let _ = mgr.remove(&wt.child_id, true).await;
        }

        Ok(())
    }

    pub async fn cancel_all(&self) {
        let mut children = self.children.lock().await;
        for (_, child) in children.iter_mut() {
            child.cancel_flag.store(true, Ordering::Relaxed);
            child.status = ChildStatus::Cancelled;
            if let (Some(wt), Some(mgr)) = (&child.worktree, &self.worktrees) {
                let _ = mgr.remove(&wt.child_id, true).await;
            }
        }
    }

    pub fn resolve_provider(
        config: &Config,
        provider_name: Option<&str>,
        model_override: Option<&str>,
        fallback_provider: &str,
    ) -> Result<(Arc<dyn Provider>, String, String), String> {
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
        let provider: Arc<dyn Provider> =
            Arc::from(create_provider(name, &profile).map_err(|e| format!("provider error: {e}"))?);
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

#[cfg(all(test, not(miri)))]
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
