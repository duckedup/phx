use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite};

use crate::commands::dispatcher;
use crate::config::schema::Config;
use crate::providers;
use crate::rpc::protocol::*;
use crate::session::SessionEvent;
use crate::session::agent_loop::Session;
use crate::store::session_store::{SessionId, SessionStore};
use crate::tools;

pub async fn run(
    config: Config,
    input: impl AsyncBufRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
) -> anyhow::Result<()> {
    let store = SessionStore::new(crate::config::paths::sessions_dir());
    let tool_registry = tools::build_registry_all();
    let project = std::env::current_dir().unwrap_or_default();
    let mut session: Option<Session> = None;

    let mut lines = input.lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_error(
                    &mut output,
                    &serde_json::Value::Null,
                    ErrorCode::ParseError,
                    &e.to_string(),
                )
                .await?;
                continue;
            }
        };

        let id = req.id.clone();

        match req.method.as_str() {
            CONFIG_GET => {
                let result = serde_json::to_value(&config).unwrap_or_default();
                write_success(&mut output, &id, result).await?;
            }

            COMMAND_LIST => {
                let skills = crate::session::skills::discover_layered(
                    Some(&project),
                    &crate::config::paths::user_home(),
                    &config.skills.dirs,
                );
                let cmds = dispatcher::list_commands(&skills);
                let result: Vec<serde_json::Value> = cmds
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "name": c.name,
                            "summary": c.summary,
                            "is_skill": c.is_skill,
                        })
                    })
                    .collect();
                write_success(&mut output, &id, serde_json::json!(result)).await?;
            }

            COMMAND_DISPATCH => {
                let input_text = req
                    .params
                    .get("input")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let skills = crate::session::skills::discover_layered(
                    Some(&project),
                    &crate::config::paths::user_home(),
                    &config.skills.dirs,
                );
                let result =
                    dispatcher::dispatch(input_text, &config, &skills, &store, &project).await;
                let result_json = match result {
                    dispatcher::CommandResult::Message(msg) => {
                        serde_json::json!({"type": "message", "text": msg})
                    }
                    dispatcher::CommandResult::Error(err) => {
                        serde_json::json!({"type": "error", "text": err})
                    }
                    dispatcher::CommandResult::ModelPicker(choices) => {
                        let items: Vec<serde_json::Value> = choices
                            .iter()
                            .map(|c| serde_json::json!({"name": c.provider_name, "display": c.display}))
                            .collect();
                        serde_json::json!({"type": "model_picker", "items": items})
                    }
                    dispatcher::CommandResult::SessionPicker(choices) => {
                        let items: Vec<serde_json::Value> = choices
                            .iter()
                            .map(
                                |c| serde_json::json!({"id": c.id, "display_name": c.display_name}),
                            )
                            .collect();
                        serde_json::json!({"type": "session_picker", "items": items})
                    }
                    dispatcher::CommandResult::ThemePicker(themes) => {
                        let items: Vec<serde_json::Value> = themes
                            .iter()
                            .map(|t| serde_json::json!({"id": t.id, "name": t.name}))
                            .collect();
                        serde_json::json!({"type": "theme_picker", "items": items})
                    }
                    dispatcher::CommandResult::InjectContext {
                        name,
                        content,
                        model_override,
                    } => {
                        let mut val = serde_json::json!({"type": "inject_context", "name": name, "text": content});
                        if let Some(model) = model_override {
                            val["model"] = serde_json::Value::String(model);
                        }
                        val
                    }
                    dispatcher::CommandResult::ClearSession => {
                        serde_json::json!({"type": "clear_session"})
                    }
                    dispatcher::CommandResult::CompactSession => {
                        serde_json::json!({"type": "compact_session"})
                    }
                    _ => serde_json::json!({"type": "ok"}),
                };
                write_success(&mut output, &id, result_json).await?;
            }

            SESSION_SEND => {
                let msg_text = req
                    .params
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let sess = session.get_or_insert_with(|| {
                    Session::new(
                        SessionId::new(),
                        crate::config::schema::SessionProfile::default(),
                    )
                });

                sess.add_message(crate::session::Message::user(msg_text));

                let (provider_name, provider_profile) =
                    match crate::config::loader::active_provider(&config) {
                        Some(p) => p,
                        None => {
                            write_error_line(&mut output, &id, "no provider configured").await?;
                            write_done_line(&mut output, &id).await?;
                            continue;
                        }
                    };

                let provider = match providers::create_provider(provider_name, provider_profile) {
                    Ok(p) => p,
                    Err(e) => {
                        write_error_line(&mut output, &id, &e.to_string()).await?;
                        write_done_line(&mut output, &id).await?;
                        continue;
                    }
                };

                let mut rx = sess.subscribe();

                let skills = crate::session::skills::discover_layered(
                    Some(&project),
                    &crate::config::paths::user_home(),
                    &config.skills.dirs,
                );

                let id_clone = id.clone();
                let run_fut = sess.run(&*provider, &tool_registry, &store, &project, &skills);

                // Stream events as they arrive during session execution
                tokio::pin!(run_fut);
                loop {
                    tokio::select! {
                        biased;
                        event = rx.recv() => {
                            match event {
                                Ok(SessionEvent::Token(t)) => {
                                    write_token_line(&mut output, &id_clone, &t).await?;
                                }
                                Ok(SessionEvent::ToolCallStart { id: tid, name, .. }) => {
                                    write_tool_call_line(&mut output, &id_clone, &tid, &name).await?;
                                }
                                Ok(SessionEvent::ToolCallEnd { id: tid, output: out }) => {
                                    write_tool_result_line(&mut output, &id_clone, &tid, &out).await?;
                                }
                                Ok(SessionEvent::Error(e)) => {
                                    write_error_line(&mut output, &id_clone, &e).await?;
                                }
                                Ok(SessionEvent::Done) => {
                                    break;
                                }
                                Ok(_) => {}
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                    tracing::warn!("RPC event stream lagged by {n} messages");
                                }
                            }
                        }
                        _ = &mut run_fut => {
                            // Session finished — drain remaining events
                            while let Ok(event) = rx.try_recv() {
                                match event {
                                    SessionEvent::Token(t) => {
                                        write_token_line(&mut output, &id_clone, &t).await?;
                                    }
                                    SessionEvent::ToolCallStart { id: tid, name, .. } => {
                                        write_tool_call_line(&mut output, &id_clone, &tid, &name).await?;
                                    }
                                    SessionEvent::ToolCallEnd { id: tid, output: out } => {
                                        write_tool_result_line(&mut output, &id_clone, &tid, &out).await?;
                                    }
                                    SessionEvent::Error(e) => {
                                        write_error_line(&mut output, &id_clone, &e).await?;
                                    }
                                    _ => {}
                                }
                            }
                            break;
                        }
                    }
                }
                write_done_line(&mut output, &id).await?;
            }

            SESSION_LIST => match store.list(&project).await {
                Ok(sessions) => {
                    let items: Vec<serde_json::Value> = sessions
                        .iter()
                        .map(|s| {
                            serde_json::json!({
                                "id": s.id.0,
                                "display_name": s.display_name,
                                "provider": s.provider,
                                "model": s.model,
                                "updated_at": s.updated_at.to_rfc3339(),
                            })
                        })
                        .collect();
                    write_success(&mut output, &id, serde_json::json!(items)).await?;
                }
                Err(e) => {
                    write_error(&mut output, &id, ErrorCode::InternalError, &e.to_string()).await?;
                }
            },

            _ => {
                write_error(
                    &mut output,
                    &id,
                    ErrorCode::MethodNotFound,
                    &format!("unknown method: {}", req.method),
                )
                .await?;
            }
        }
    }

    Ok(())
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use std::io::Cursor;

    async fn run_rpc(input: &str) -> String {
        let config = Config::default();
        let reader = tokio::io::BufReader::new(Cursor::new(input.as_bytes().to_vec()));
        let mut output = Vec::new();
        run(config, reader, &mut output).await.unwrap();
        String::from_utf8(output).unwrap()
    }

    #[tokio::test]
    async fn config_get() {
        let output = run_rpc(r#"{"id":1,"method":"config.get","params":{}}"#).await;
        let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["id"], 1);
        assert!(parsed["result"].is_object());
    }

    #[tokio::test]
    async fn unknown_method() {
        let output = run_rpc(r#"{"id":2,"method":"bogus","params":{}}"#).await;
        let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn command_list() {
        let output = run_rpc(r#"{"id":3,"method":"command.list","params":{}}"#).await;
        let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert!(parsed["result"].is_array());
    }

    #[tokio::test]
    async fn parse_error() {
        let output = run_rpc("not json at all\n").await;
        let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["error"]["code"], -32700);
    }
}
