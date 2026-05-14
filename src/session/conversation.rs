use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt;
use tokio::sync::mpsc;

use crate::providers::traits::{
    Event, Provider, ProviderMessage, ProviderRole, ProviderToolCall, ProviderToolResult,
    SendOptions, StopReason, ToolSchema,
};
use crate::session::agent_loop::Session;
use crate::session::context;
use crate::session::message::{Message, Role};
use crate::session::skills;
use crate::store::session_store::SessionStore;
use crate::tools::traits::ToolRegistry;

pub enum ConvEvent {
    StreamToken(String),
    AssistantMessage(String),
    ToolCall(String),
    ToolResult {
        output: String,
        is_error: bool,
    },
    InteractiveUi {
        call_id: String,
        tool_name: String,
        fields: Vec<phoenix_shared::ui_field_types::UiField>,
        response_tx: tokio::sync::oneshot::Sender<Option<String>>,
    },
    ContextLoaded(Vec<String>),
    ContextCompacted {
        removed: usize,
        remaining: usize,
    },
    Error(String),
    Done(Session),
    Cancelled(Session),
}

pub struct ConvConfig {
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<parking_lot::RwLock<ToolRegistry>>,
    pub store: SessionStore,
    pub project: PathBuf,
    pub config: crate::config::schema::Config,
    pub system_prompt_override: Option<String>,
    pub plugin_runtime:
        Option<Arc<parking_lot::Mutex<crate::plugin::plugin_runtime::PluginRuntime>>>,
}

pub fn spawn_conversation(
    mut session: Session,
    text: String,
    cfg: ConvConfig,
    cancel: Arc<AtomicBool>,
) -> mpsc::UnboundedReceiver<ConvEvent> {
    let (tx, rx) = mpsc::unbounded_channel();

    let user_msg = Message::user(&text);
    session.add_message(user_msg);

    tokio::spawn(async move {
        let ConvConfig {
            provider,
            tools,
            store,
            project,
            config,
            system_prompt_override,
            plugin_runtime,
        } = cfg;

        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(ConvEvent::Cancelled(session));
                return;
            }

            let tool_schemas: Vec<ToolSchema> = tools
                .read()
                .list_schemas()
                .into_iter()
                .map(|s| ToolSchema {
                    name: s.name.to_string(),
                    description: s.description.to_string(),
                    parameters: s.parameters,
                })
                .collect();

            let base_prompt = if let Some(ref override_prompt) = system_prompt_override {
                Some(override_prompt.clone())
            } else {
                session
                    .profile
                    .system_prompt_path
                    .as_ref()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .or_else(|| {
                        Some(
                            "You are Phoenix, a fast and capable coding assistant running in a terminal.\n\
                             \n\
                             You have access to tools for reading files, writing files, editing files, and \
                             running shell commands. Use them to help the user with software engineering tasks.\n\
                             \n\
                             Guidelines:\n\
                             - Be concise. The user is in a terminal — respect their screen space.\n\
                             - When editing code, preserve existing style and conventions.\n\
                             - Prefer editing existing files over creating new ones.\n\
                             - Use the bash tool for commands; use read/write/edit tools for files.\n\
                             - Show your work: explain what you're doing briefly, then do it.\n\
                             - If a task is ambiguous, make a reasonable assumption and proceed.\n\
                             - When you encounter errors, diagnose the root cause before retrying."
                                .to_string(),
                        )
                    })
            };

            let home = crate::config::paths::config_dir()
                .parent()
                .unwrap_or(std::path::Path::new("/"))
                .to_path_buf();
            let skill_list = skills::discover_layered(
                Some(&project),
                &crate::config::paths::user_home(),
                &config.skills.dirs,
            );
            let ctx = context::build_context(
                &home,
                &project,
                &session.messages,
                &mut session.context_state,
                &skill_list,
            );

            let system_prompt = base_prompt.map(|base| {
                if ctx.system_prompt_suffix.is_empty() {
                    base
                } else {
                    format!("{base}\n\n{}", ctx.system_prompt_suffix)
                }
            });

            if !ctx.newly_loaded.is_empty() {
                let _ = tx.send(ConvEvent::ContextLoaded(ctx.newly_loaded));
            }

            let active_profile = crate::config::loader::active_provider(&config)
                .map(|(_, p)| p.clone())
                .unwrap_or_default();
            let limits = context::resolve_context_limits(
                &session.model_name,
                &active_profile,
                &session.profile,
            );
            let prompt_ref = system_prompt.as_deref().unwrap_or("");
            let compaction = context::enforce_limits(&mut session.messages, prompt_ref, &limits);
            if compaction.was_compacted {
                let _ = tx.send(ConvEvent::ContextCompacted {
                    removed: compaction.removed_count,
                    remaining: compaction.remaining_count,
                });
            }

            let provider_messages: Vec<ProviderMessage> = session
                .messages
                .iter()
                .map(|m| ProviderMessage {
                    role: match m.role {
                        Role::System => ProviderRole::System,
                        Role::User => ProviderRole::User,
                        Role::Assistant => ProviderRole::Assistant,
                        Role::ToolCall => ProviderRole::Assistant,
                        Role::ToolResult => ProviderRole::Tool,
                    },
                    content: m.content.clone(),
                    tool_call: m.tool_call.as_ref().map(|tc| ProviderToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        args_json: tc.args_json.clone(),
                    }),
                    tool_result: m.tool_result.as_ref().map(|tr| ProviderToolResult {
                        id: tr.id.clone(),
                        output: tr.output.clone(),
                        is_error: tr.is_error,
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
                    let _ = tx.send(ConvEvent::Error(format!("Provider error: {e}")));
                    let _ = tx.send(ConvEvent::Done(session));
                    return;
                }
            };

            futures::pin_mut!(stream);

            let mut assistant_text = String::new();
            let mut pending_tool_calls: Vec<crate::session::message::ToolCall> = vec![];
            let mut got_tool_use_stop = false;

            loop {
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(ConvEvent::Cancelled(session));
                    return;
                }

                match stream.next().await {
                    Some(Event::Token(t)) => {
                        let _ = tx.send(ConvEvent::StreamToken(t.clone()));
                        assistant_text.push_str(&t);
                    }
                    Some(Event::ToolCall {
                        id,
                        name,
                        args_json,
                    }) => {
                        pending_tool_calls.push(crate::session::message::ToolCall {
                            id,
                            name,
                            args_json,
                        });
                    }
                    Some(Event::Done { stop_reason, usage }) => {
                        session.token_input += usage.input_tokens;
                        session.token_output += usage.output_tokens;
                        session.cache_creation_tokens += usage.cache_creation_tokens;
                        session.cache_read_tokens += usage.cache_read_tokens;
                        session.last_turn_input = usage.input_tokens
                            + usage.cache_read_tokens
                            + usage.cache_creation_tokens;
                        if stop_reason == StopReason::ToolUse {
                            got_tool_use_stop = true;
                        }
                        break;
                    }
                    Some(Event::Error(e)) => {
                        let _ = tx.send(ConvEvent::Error(format!("Error: {e}")));
                        let _ = tx.send(ConvEvent::Done(session));
                        return;
                    }
                    None => break,
                }
            }

            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(ConvEvent::Cancelled(session));
                return;
            }

            if !assistant_text.is_empty() {
                let _ = tx.send(ConvEvent::AssistantMessage(assistant_text.clone()));
                let msg = Message::assistant(std::mem::take(&mut assistant_text));
                session.persist_message(&store, &project, &msg).await;
                session.add_message(msg);
            }

            if !pending_tool_calls.is_empty() {
                for tc in &pending_tool_calls {
                    let tc_msg = Message::tool_call(tc.clone());
                    session.persist_message(&store, &project, &tc_msg).await;
                    session.add_message(tc_msg);

                    let summary = format!("🔧 {}", tc.name);
                    let _ = tx.send(ConvEvent::ToolCall(summary));

                    let dynamic_ui = plugin_runtime
                        .as_ref()
                        .and_then(|rt| rt.lock().request_dynamic_ui(&tc.name, &tc.args_json));

                    let tr = if let Some(fields) = dynamic_ui {
                        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
                        let _ = tx.send(ConvEvent::InteractiveUi {
                            call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            fields,
                            response_tx,
                        });
                        match response_rx.await {
                            Ok(Some(answers)) => crate::session::message::ToolResult {
                                id: tc.id.clone(),
                                output: answers,
                                is_error: false,
                            },
                            _ => crate::session::message::ToolResult {
                                id: tc.id.clone(),
                                output: "User cancelled.".into(),
                                is_error: true,
                            },
                        }
                    } else {
                        let maybe_tool = tools.read().get(&tc.name);
                        if let Some(tool) = maybe_tool {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc.args_json).unwrap_or_default();
                            let noop = crate::tools::traits::NoopInputRequester;
                            match tool.invoke(args, &noop).await {
                                Ok(r) => crate::session::message::ToolResult {
                                    id: tc.id.clone(),
                                    output: r.output,
                                    is_error: r.is_error,
                                },
                                Err(e) => crate::session::message::ToolResult {
                                    id: tc.id.clone(),
                                    output: e.to_string(),
                                    is_error: true,
                                },
                            }
                        } else {
                            crate::session::message::ToolResult {
                                id: tc.id.clone(),
                                output: format!("unknown tool: {}", tc.name),
                                is_error: true,
                            }
                        }
                    };

                    let output_display = truncate_str(&tr.output, 2000);
                    let _ = tx.send(ConvEvent::ToolResult {
                        output: output_display,
                        is_error: tr.is_error,
                    });

                    let tr_msg = Message::tool_result(tr);
                    session.persist_message(&store, &project, &tr_msg).await;
                    session.add_message(tr_msg);
                }

                if got_tool_use_stop {
                    continue;
                }
            }

            break;
        }

        session.persist_state(&store, &project).await;
        let _ = tx.send(ConvEvent::Done(session));
    });

    rx
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    match s.char_indices().nth(max_chars) {
        Some((byte_pos, _)) => format!("{}...", &s[..byte_pos]),
        None => s.to_string(),
    }
}
