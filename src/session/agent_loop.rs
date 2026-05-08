use std::path::Path;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::config::schema::SessionProfile;
use crate::plugin::hooks::{HookAction, HookDispatcher, HookEvent};
use crate::providers::traits::{Event, Provider, SendOptions, StopReason, ToolSchema};
use crate::session::message::{Message, ToolCall, ToolResult};
use crate::store::session_store::{SessionId, SessionState, SessionStore};
use crate::tools::traits::ToolRegistry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Running,
    Done,
    Error(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Token(String),
    ToolCallStart { id: String, name: String },
    ToolCallEnd { id: String, output: String },
    ContextLoaded(Vec<String>),
    ContextCompacted { removed: usize, remaining: usize },
    Done,
    Error(String),
}

pub struct Session {
    pub id: SessionId,
    pub messages: Vec<Message>,
    pub state: SessionStatus,
    pub token_input: u64,
    pub token_output: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    /// Input tokens from the most recent API call (current context size)
    pub last_turn_input: u64,
    pub profile: SessionProfile,
    pub persist: bool,
    pub provider_name: String,
    pub model_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub context_state: crate::session::context::ContextState,
    events_tx: broadcast::Sender<SessionEvent>,
}

impl Session {
    pub fn new(id: SessionId, profile: SessionProfile) -> Self {
        let persist = profile.persist;
        let (events_tx, _) = broadcast::channel(256);
        Self {
            id,
            messages: Vec::new(),
            state: SessionStatus::Idle,
            token_input: 0,
            token_output: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            last_turn_input: 0,
            profile,
            persist,
            provider_name: String::new(),
            model_name: String::new(),
            created_at: chrono::Utc::now(),
            context_state: crate::session::context::ContextState::default(),
            events_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events_tx.subscribe()
    }

    pub fn add_message(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    pub async fn persist_message(&self, store: &SessionStore, project: &Path, msg: &Message) {
        if !self.persist {
            return;
        }
        let val = serde_json::to_value(msg).unwrap_or_default();
        if let Err(e) = store.append_message(project, &self.id, &val).await {
            tracing::warn!("failed to persist message: {e}");
        }
    }

    pub async fn persist_state(&self, store: &SessionStore, project: &Path) {
        if !self.persist {
            return;
        }
        let state = SessionState {
            id: self.id.clone(),
            display_name: self.display_name(),
            provider: self.provider_name.clone(),
            model: self.model_name.clone(),
            created_at: self.created_at,
            updated_at: chrono::Utc::now(),
            token_input: self.token_input,
            token_output: self.token_output,
        };
        if let Err(e) = store.update_state(project, &state).await {
            tracing::warn!("failed to persist state: {e}");
        }
    }

    fn display_name(&self) -> String {
        self.messages
            .iter()
            .find(|m| m.role == crate::session::message::Role::User)
            .map(|m| {
                let s = m.content.chars().take(60).collect::<String>();
                if m.content.len() > 60 {
                    format!("{s}...")
                } else {
                    s
                }
            })
            .unwrap_or_else(|| "New session".into())
    }

    fn tool_schemas(&self, registry: &ToolRegistry) -> Vec<ToolSchema> {
        registry
            .list_schemas()
            .into_iter()
            .map(|s| ToolSchema {
                name: s.name.to_string(),
                description: s.description.to_string(),
                parameters: s.parameters,
            })
            .collect()
    }

    pub async fn run(
        &mut self,
        provider: &dyn Provider,
        tools: &ToolRegistry,
        store: &SessionStore,
        project: &Path,
        skills: &[crate::session::skills::Skill],
    ) {
        self.run_with_hooks(provider, tools, store, project, skills, None)
            .await;
    }

    pub async fn run_with_hooks(
        &mut self,
        provider: &dyn Provider,
        tools: &ToolRegistry,
        store: &SessionStore,
        project: &Path,
        skills: &[crate::session::skills::Skill],
        hooks: Option<Arc<HookDispatcher>>,
    ) {
        use futures::StreamExt;

        self.state = SessionStatus::Running;

        loop {
            let tool_schemas = self.tool_schemas(tools);
            let base_prompt = self
                .profile
                .system_prompt_path
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok());

            let home = crate::config::paths::config_dir()
                .parent()
                .unwrap_or(std::path::Path::new("/"))
                .to_path_buf();
            let ctx = crate::session::context::build_context(
                &home,
                project,
                &self.messages,
                &mut self.context_state,
                skills,
            );

            let system_prompt = base_prompt.map(|base| {
                if ctx.system_prompt_suffix.is_empty() {
                    base
                } else {
                    format!("{base}\n\n{}", ctx.system_prompt_suffix)
                }
            });

            if !ctx.newly_loaded.is_empty() {
                let _ = self
                    .events_tx
                    .send(SessionEvent::ContextLoaded(ctx.newly_loaded));
            }

            let provider_profile = crate::config::schema::ProviderProfile::default();
            let limits = crate::session::context::resolve_context_limits(
                &self.model_name,
                &provider_profile,
                &self.profile,
            );
            let prompt_ref = system_prompt.as_deref().unwrap_or("");
            let compaction =
                crate::session::context::enforce_limits(&mut self.messages, prompt_ref, &limits);
            if compaction.was_compacted {
                let _ = self.events_tx.send(SessionEvent::ContextCompacted {
                    removed: compaction.removed_count,
                    remaining: compaction.remaining_count,
                });
            }

            let provider_messages = self
                .messages
                .iter()
                .map(|m| crate::providers::traits::ProviderMessage {
                    role: match m.role {
                        crate::session::message::Role::System => {
                            crate::providers::traits::ProviderRole::System
                        }
                        crate::session::message::Role::User => {
                            crate::providers::traits::ProviderRole::User
                        }
                        crate::session::message::Role::Assistant => {
                            crate::providers::traits::ProviderRole::Assistant
                        }
                        crate::session::message::Role::ToolCall => {
                            crate::providers::traits::ProviderRole::Assistant
                        }
                        crate::session::message::Role::ToolResult => {
                            crate::providers::traits::ProviderRole::Tool
                        }
                    },
                    content: m.content.clone(),
                    tool_call: m.tool_call.as_ref().map(|tc| {
                        crate::providers::traits::ProviderToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            args_json: tc.args_json.clone(),
                        }
                    }),
                    tool_result: m.tool_result.as_ref().map(|tr| {
                        crate::providers::traits::ProviderToolResult {
                            id: tr.id.clone(),
                            output: tr.output.clone(),
                            is_error: tr.is_error,
                        }
                    }),
                })
                .collect();

            let opts = SendOptions {
                messages: provider_messages,
                tools: tool_schemas,
                system_prompt,
            };

            let stream = match provider.send(opts).await {
                Ok(s) => s,
                Err(e) => {
                    let msg = e.to_string();
                    self.state = SessionStatus::Error(msg.clone());
                    let _ = self.events_tx.send(SessionEvent::Error(msg));
                    return;
                }
            };

            futures::pin_mut!(stream);

            let mut assistant_text = String::new();
            let mut pending_tool_calls: Vec<ToolCall> = vec![];
            let mut got_tool_use_stop = false;

            while let Some(event) = stream.next().await {
                match event {
                    Event::Token(t) => {
                        assistant_text.push_str(&t);
                        let _ = self.events_tx.send(SessionEvent::Token(t));
                    }
                    Event::ToolCall {
                        id,
                        name,
                        args_json,
                    } => {
                        let _ = self.events_tx.send(SessionEvent::ToolCallStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        pending_tool_calls.push(ToolCall {
                            id,
                            name,
                            args_json,
                        });
                    }
                    Event::Done { stop_reason, usage } => {
                        self.token_input += usage.input_tokens;
                        self.token_output += usage.output_tokens;
                        self.cache_creation_tokens += usage.cache_creation_tokens;
                        self.cache_read_tokens += usage.cache_read_tokens;
                        self.last_turn_input = usage.input_tokens
                            + usage.cache_read_tokens
                            + usage.cache_creation_tokens;
                        if stop_reason == StopReason::ToolUse {
                            got_tool_use_stop = true;
                        }
                    }
                    Event::Error(e) => {
                        self.state = SessionStatus::Error(e.to_string());
                        let _ = self.events_tx.send(SessionEvent::Error(e.to_string()));
                        return;
                    }
                }
            }

            if !assistant_text.is_empty() {
                let msg = Message::assistant(std::mem::take(&mut assistant_text));
                self.persist_message(store, project, &msg).await;
                self.add_message(msg);
            }

            if !pending_tool_calls.is_empty() {
                for tc in &pending_tool_calls {
                    let tc_msg = Message::tool_call(tc.clone());
                    self.persist_message(store, project, &tc_msg).await;
                    self.add_message(tc_msg);

                    // Hook: tool_call_start
                    let hook_action = if let Some(ref h) = hooks {
                        let data = serde_json::json!({
                            "name": tc.name,
                            "args": serde_json::from_str::<serde_json::Value>(&tc.args_json).unwrap_or_default(),
                            "call_id": tc.id,
                        });
                        h.hook(HookEvent::ToolCallStart, data).await
                    } else {
                        HookAction::Allow
                    };

                    let tr = match hook_action {
                        HookAction::Block { reason } => ToolResult {
                            id: tc.id.clone(),
                            output: format!("blocked by plugin: {reason}"),
                            is_error: true,
                        },
                        _ => {
                            if let Some(tool) = tools.get(&tc.name) {
                                let args: serde_json::Value =
                                    serde_json::from_str(&tc.args_json).unwrap_or_default();
                                match tool.invoke(args).await {
                                    Ok(r) => ToolResult {
                                        id: tc.id.clone(),
                                        output: r.output,
                                        is_error: r.is_error,
                                    },
                                    Err(e) => ToolResult {
                                        id: tc.id.clone(),
                                        output: e.to_string(),
                                        is_error: true,
                                    },
                                }
                            } else {
                                ToolResult {
                                    id: tc.id.clone(),
                                    output: format!("unknown tool: {}", tc.name),
                                    is_error: true,
                                }
                            }
                        }
                    };

                    // Hook: tool_call_end notification
                    if let Some(ref h) = hooks {
                        h.notify(
                            HookEvent::ToolCallEnd,
                            serde_json::json!({
                                "call_id": tc.id,
                                "name": tc.name,
                                "output": tr.output,
                                "is_error": tr.is_error,
                            }),
                        )
                        .await;
                    }

                    let _ = self.events_tx.send(SessionEvent::ToolCallEnd {
                        id: tr.id.clone(),
                        output: tr.output.clone(),
                    });

                    let tr_msg = Message::tool_result(tr);
                    self.persist_message(store, project, &tr_msg).await;
                    self.add_message(tr_msg);
                }

                if got_tool_use_stop {
                    continue;
                }
            }

            self.state = SessionStatus::Done;
            self.persist_state(store, project).await;
            let _ = self.events_tx.send(SessionEvent::Done);
            break;
        }
    }

    pub fn cancel(&mut self) {
        self.state = SessionStatus::Cancelled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::traits::{Event, MockProvider, Usage};
    use crate::tools::build_registry_all;

    fn test_store(dir: &std::path::Path) -> SessionStore {
        SessionStore::new(dir.to_path_buf())
    }

    #[tokio::test]
    async fn single_turn_agent_loop() {
        let dir = tempfile::tempdir().unwrap();
        let store = test_store(dir.path());
        let provider = MockProvider::with_events(vec![
            Event::Token("Hello".into()),
            Event::Done {
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
            },
        ]);
        let tools = build_registry_all();
        let profile = SessionProfile::default();
        let mut session = Session::new(SessionId::new(), profile);
        session.add_message(Message::user("Hi"));
        session
            .run(
                &provider,
                &tools,
                &store,
                std::path::Path::new("/test"),
                &[],
            )
            .await;

        assert_eq!(session.state, SessionStatus::Done);
        assert_eq!(session.messages.last().unwrap().content, "Hello");
        assert_eq!(session.token_input, 10);
        assert_eq!(session.token_output, 5);
    }

    #[tokio::test]
    async fn provider_error_sets_error_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = test_store(dir.path());
        let provider = MockProvider::with_error("connection refused");
        let tools = build_registry_all();
        let mut session = Session::new(SessionId::new(), SessionProfile::default());
        session.add_message(Message::user("Hi"));
        session
            .run(
                &provider,
                &tools,
                &store,
                std::path::Path::new("/test"),
                &[],
            )
            .await;

        matches!(session.state, SessionStatus::Error(_));
    }
}
