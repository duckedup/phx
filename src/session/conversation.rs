use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt;
use tokio::sync::mpsc;

use crate::providers::traits::{Event, Provider, SendOptions, StopReason, ToolSchema};
use crate::session::agent_loop::Session;
use crate::session::context;
use crate::session::message::{self, Message};
use crate::session::skills;
use crate::store::session_store::SessionStore;
use crate::tools::traits::ToolRegistry;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

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
        fields: Vec<crate::shared::ui_field_types::UiField>,
        response_tx: tokio::sync::oneshot::Sender<Option<String>>,
    },
    ContextLoaded(Vec<String>),
    ContextCompacted {
        removed: usize,
        remaining: usize,
    },
    Retrying {
        attempt: u32,
        max_retries: u32,
        wait_secs: u64,
        error: String,
    },
    RetryRecovered {
        attempts: u32,
    },
    Error(String),
    Done(Session),
    Cancelled(Session),
    ResumeHistory(Vec<Message>),
    RemoteSessionId(String),
}

pub struct ConvParams {
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<parking_lot::RwLock<ToolRegistry>>,
    pub store: SessionStore,
    pub project: PathBuf,
    pub config: crate::config::Config,
    pub system_prompt_override: Option<String>,
    pub plugin_runtime:
        Option<Arc<parking_lot::Mutex<crate::plugin::plugin_runtime::PluginRuntime>>>,
    pub tool_router: Option<crate::session::tool_router::ToolRouter>,
}

// ---------------------------------------------------------------------------
// Conveyor belt
// ---------------------------------------------------------------------------

pub fn spawn_conversation(
    mut session: Session,
    text: String,
    cfg: ConvParams,
    cancel: Arc<AtomicBool>,
) -> mpsc::UnboundedReceiver<ConvEvent> {
    let (tx, rx) = mpsc::unbounded_channel();

    let user_msg = Message::user(&text);
    session.add_message(user_msg);

    tokio::spawn(async move {
        let ConvParams {
            provider,
            tools,
            store,
            project,
            config,
            system_prompt_override,
            plugin_runtime,
            tool_router,
        } = cfg;

        let mut current_provider: Arc<dyn Provider> = Arc::clone(&provider);
        let all_tool_schemas: Vec<ToolSchema> = tools
            .read()
            .list_schemas()
            .into_iter()
            .map(|s| ToolSchema {
                name: s.name.to_string(),
                description: s.description.to_string(),
                parameters: s.parameters,
            })
            .collect();

        // The belt keeps spinning until the LLM says "done" with no tool calls,
        // or an error / cancellation stops it.
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(ConvEvent::Cancelled(session));
                return;
            }

            // --- Build context & prepare the LLM call ---
            let tool_schemas = filter_tool_schemas(
                &all_tool_schemas,
                &current_provider,
                &provider,
                &tool_router,
            );

            let custom_prompt = match &session.profile.system_prompt_path {
                Some(p) => tokio::fs::read_to_string(p).await.ok(),
                None => None,
            };
            let system_prompt = build_system_prompt(
                custom_prompt,
                &session.messages,
                &mut session.context_state,
                &project,
                &config,
                &system_prompt_override,
                &tx,
            );

            let provider_messages = build_provider_messages(&session);

            let opts = SendOptions {
                messages: provider_messages,
                tools: tool_schemas,
                system_prompt,
            };

            // --- Send to LLM, get the stream (with retry + backoff) ---
            let stream = match crate::http::send_with_retry(
                current_provider.as_ref() as &dyn Provider,
                &opts,
                Some(&cancel),
                |attempt, wait_secs, err| {
                    let _ = tx.send(ConvEvent::Retrying {
                        attempt,
                        max_retries: crate::http::MAX_RETRIES,
                        wait_secs,
                        error: err.to_string(),
                    });
                },
            )
            .await
            {
                crate::http::RetryOutcome::Success { stream, attempts } => {
                    if attempts > 0 {
                        let _ = tx.send(ConvEvent::RetryRecovered { attempts });
                    }
                    stream
                }
                crate::http::RetryOutcome::Failed(e) => {
                    let _ = tx.send(ConvEvent::Error(format!("Provider error: {e}")));
                    let _ = tx.send(ConvEvent::Done(session));
                    return;
                }
                crate::http::RetryOutcome::Cancelled => {
                    let _ = tx.send(ConvEvent::Cancelled(session));
                    return;
                }
            };

            futures::pin_mut!(stream);

            let mut assistant_text = String::new();
            let mut pending_tool_calls: Vec<crate::session::message::ToolCall> = vec![];
            let mut got_tool_use_stop = false;

            // --- Phase 1: Drain the LLM stream (tokens + tool call declarations) ---
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

            // --- Persist assistant text ---
            if !assistant_text.is_empty() {
                let _ = tx.send(ConvEvent::AssistantMessage(assistant_text.clone()));
                let msg = Message::assistant(std::mem::take(&mut assistant_text));
                session.persist_message(&store, &project, &msg).await;
                session.add_message(msg);
            }

            // --- Phase 2: Fire tools on real threads, collect via channel ---
            if !pending_tool_calls.is_empty() {
                for tc in &pending_tool_calls {
                    let tc_msg = Message::tool_call(tc.clone());
                    session.persist_message(&store, &project, &tc_msg).await;
                    session.add_message(tc_msg);
                    let summary =
                        crate::shared::tool_display::tool_call_summary(&tc.name, &tc.args_json);
                    let _ = tx.send(ConvEvent::ToolCall(summary));
                }

                // Check interactive UI info under lock (fast HashMap lookup)
                let ui_infos: Vec<Option<_>> = pending_tool_calls
                    .iter()
                    .map(|tc| {
                        plugin_runtime
                            .as_ref()
                            .and_then(|rt| rt.lock().dynamic_ui_info(&tc.name))
                    })
                    .collect();

                // Spawn each tool on its own task — tokio distributes across cores.
                // Use JoinHandle vec to preserve call order when collecting results.
                let mut handles = Vec::with_capacity(pending_tool_calls.len());
                for (tc, ui_info) in pending_tool_calls.iter().zip(ui_infos) {
                    let tools = Arc::clone(&tools);
                    let conv_tx = tx.clone();
                    let tc_id = tc.id.clone();
                    let tc_name = tc.name.clone();
                    let tc_args = tc.args_json.clone();

                    handles.push(tokio::spawn(async move {
                        execute_tool(&tc_id, &tc_name, &tc_args, ui_info, &tools, &conv_tx).await
                    }));
                }

                // Collect results in original tool-call order so the LLM sees
                // deterministic tool_result sequences across runs.
                // Race each handle against the cancel flag so Esc kills
                // long-running tools (e.g. bash) immediately.
                let mut was_cancelled = false;
                for mut handle in handles.drain(..) {
                    if cancel.load(Ordering::Relaxed) {
                        handle.abort();
                        was_cancelled = true;
                        continue;
                    }
                    let cancel_ref = Arc::clone(&cancel);
                    let cancel_fut = async move {
                        loop {
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            if cancel_ref.load(Ordering::Relaxed) {
                                return;
                            }
                        }
                    };
                    tokio::select! {
                        result = &mut handle => {
                            let tr = match result {
                                Ok(r) => r,
                                Err(e) => crate::session::message::ToolResult {
                                    id: String::new(),
                                    output: format!("tool task panicked: {e}"),
                                    is_error: true,
                                },
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
                        _ = cancel_fut => {
                            handle.abort();
                            was_cancelled = true;
                        }
                    }
                }
                if was_cancelled {
                    for tc in &pending_tool_calls {
                        let has_result = session
                            .messages
                            .iter()
                            .any(|m| m.tool_result.as_ref().is_some_and(|tr| tr.id == tc.id));
                        if !has_result {
                            let stub = crate::session::message::ToolResult {
                                id: tc.id.clone(),
                                output: "Cancelled.".into(),
                                is_error: true,
                            };
                            let msg = Message::tool_result(stub);
                            session.add_message(msg);
                        }
                    }
                    let _ = tx.send(ConvEvent::Cancelled(session));
                    return;
                }

                // --- Marble goes back to the top: send results to LLM ---
                if got_tool_use_stop {
                    current_provider = Arc::clone(&provider);

                    if let Some(router) = &tool_router {
                        let routed: Vec<Arc<dyn Provider>> = pending_tool_calls
                            .iter()
                            .filter_map(|tc| router.provider_for_tool(&tc.name))
                            .collect();
                        if !routed.is_empty()
                            && routed.len() == pending_tool_calls.len()
                            && routed.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1]))
                        {
                            current_provider = routed[0].clone();
                        }
                    }

                    continue; // belt spins again
                }
            }

            break; // LLM said end_turn with no tools — belt stops
        }

        session.persist_state(&store, &project).await;
        let _ = tx.send(ConvEvent::Done(session));
    });

    rx
}

// ---------------------------------------------------------------------------
// Tool execution — runs on its own spawned task (own thread/core)
// ---------------------------------------------------------------------------

async fn execute_tool(
    tc_id: &str,
    tc_name: &str,
    tc_args: &str,
    ui_info: Option<(PathBuf, Vec<String>, PathBuf)>,
    tools: &Arc<parking_lot::RwLock<ToolRegistry>>,
    conv_tx: &mpsc::UnboundedSender<ConvEvent>,
) -> crate::session::message::ToolResult {
    // Interactive UI path
    if let Some((path, args, project_dir)) = ui_info {
        let fields = crate::plugin::plugin_runtime::request_dynamic_ui_async(
            &path,
            &args,
            tc_name,
            tc_args,
            &project_dir,
        )
        .await;
        if let Some(fields) = fields {
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let _ = conv_tx.send(ConvEvent::InteractiveUi {
                call_id: tc_id.to_string(),
                tool_name: tc_name.to_string(),
                fields,
                response_tx,
            });
            return match response_rx.await {
                Ok(Some(answers)) => crate::session::message::ToolResult {
                    id: tc_id.to_string(),
                    output: answers,
                    is_error: false,
                },
                _ => crate::session::message::ToolResult {
                    id: tc_id.to_string(),
                    output: "User cancelled.".into(),
                    is_error: true,
                },
            };
        }
    }

    // Standard tool path — lock only for the HashMap lookup, release before invoke
    let maybe_tool = tools.read().get(tc_name);
    if let Some(tool) = maybe_tool {
        let args: serde_json::Value = serde_json::from_str(tc_args).unwrap_or_default();
        let noop = crate::tools::traits::NoopInputRequester;
        match tool.invoke(args, &noop).await {
            Ok(r) => crate::session::message::ToolResult {
                id: tc_id.to_string(),
                output: r.output,
                is_error: r.is_error,
            },
            Err(e) => crate::session::message::ToolResult {
                id: tc_id.to_string(),
                output: e.to_string(),
                is_error: true,
            },
        }
    } else {
        crate::session::message::ToolResult {
            id: tc_id.to_string(),
            output: format!("unknown tool: {tc_name}"),
            is_error: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers — context building (pure functions, no I/O in hot path)
// ---------------------------------------------------------------------------

fn filter_tool_schemas(
    all: &[ToolSchema],
    current_provider: &Arc<dyn Provider>,
    default_provider: &Arc<dyn Provider>,
    tool_router: &Option<crate::session::tool_router::ToolRouter>,
) -> Vec<ToolSchema> {
    if !Arc::ptr_eq(current_provider, default_provider)
        && let Some(router) = tool_router
    {
        return all
            .iter()
            .filter(|s| router.is_routed(&s.name))
            .cloned()
            .collect();
    }
    all.to_vec()
}

fn build_system_prompt(
    custom_prompt: Option<String>,
    messages: &[Message],
    context_state: &mut crate::session::context::ContextState,
    project: &std::path::Path,
    config: &crate::config::Config,
    system_prompt_override: &Option<String>,
    tx: &mpsc::UnboundedSender<ConvEvent>,
) -> Vec<String> {
    let base_prompt = if let Some(override_prompt) = system_prompt_override {
        Some(override_prompt.clone())
    } else {
        custom_prompt.or_else(|| {
            Some(
                "You are phx, a fast and capable coding assistant running in a terminal.\n\
                 \n\
                 You have access to tools for reading files, writing files, editing files, \
                 running shell commands, and spawning sub-agents. Use them to help the user \
                 with software engineering tasks.\n\
                 \n\
                 Guidelines:\n\
                 - Be concise. The user is in a terminal — respect their screen space.\n\
                 - When editing code, preserve existing style and conventions.\n\
                 - Prefer editing existing files over creating new ones.\n\
                 - Use the bash tool for commands; use read/write/edit tools for files.\n\
                 - Show your work: explain what you're doing briefly, then do it.\n\
                 - If a task is ambiguous, make a reasonable assumption and proceed.\n\
                 - When you encounter errors, diagnose the root cause before retrying.\n\
                 - For large tasks, use spawn_agent to delegate sub-tasks to parallel agents.\n\
                 - Use check_agents to see live status. Use collect_agent to get results when done.\n\
                 - Use merge_agent to merge a completed agent's worktree branch back."
                    .to_string(),
            )
        })
    };

    let home = crate::config::paths::config_dir()
        .parent()
        .unwrap_or(std::path::Path::new("/"))
        .to_path_buf();
    let skill_list = skills::discover_layered(
        Some(project),
        &crate::config::paths::user_home(),
        &config.skills.dirs,
    );
    let ctx = context::build_context(&home, project, messages, context_state, &skill_list);

    if !ctx.newly_loaded.is_empty() {
        let _ = tx.send(ConvEvent::ContextLoaded(ctx.newly_loaded));
    }

    let mut blocks = Vec::new();
    if let Some(base) = base_prompt {
        blocks.push(base);
    }
    if !ctx.system_prompt_suffix.is_empty() {
        blocks.push(ctx.system_prompt_suffix);
    }
    blocks
}

fn build_provider_messages(session: &Session) -> Vec<crate::providers::traits::ProviderMessage> {
    let messages = if session.profile.compression {
        crate::session::compress::compress_for_provider(&session.messages)
    } else {
        session.messages.clone()
    };
    message::to_provider_messages(&messages)
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
