use async_trait::async_trait;

use crate::config::ProviderProfile;
use crate::providers::traits::*;

pub struct AnthropicProvider {
    model: String,
    api_key: String,
    base_url: String,
    max_tokens: u32,
}

pub fn create(
    profile: &ProviderProfile,
    credential: Option<&str>,
) -> Result<Box<dyn Provider>, ProviderError> {
    let api_key = credential
        .ok_or(ProviderError::MissingCredential)?
        .to_string();
    let base_url = profile
        .base_url
        .clone()
        .unwrap_or_else(|| "https://api.anthropic.com".into());

    let max_tokens = profile.max_tokens.unwrap_or(16_384);

    Ok(Box::new(AnthropicProvider {
        model: profile.model.clone(),
        api_key,
        base_url,
        max_tokens,
    }))
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "claude"
    }

    async fn send(&self, opts: SendOptions) -> Result<EventStream, ProviderError> {
        let client = reqwest::Client::new();

        let messages = build_messages(&opts.messages);
        let tools = build_tools(&opts.tools);

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "stream": true,
            "messages": messages,
        });

        let system_blocks: Vec<serde_json::Value> = opts
            .system_prompt
            .iter()
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, text)| {
                let mut block = serde_json::json!({"type": "text", "text": text});
                // Cache breakpoint on the last block (stable prefix stays cached
                // even when later blocks change)
                if i == opts.system_prompt.len() - 1 {
                    block["cache_control"] = serde_json::json!({"type": "ephemeral"});
                }
                block
            })
            .collect();
        if !system_blocks.is_empty() {
            body["system"] = serde_json::json!(system_blocks);
        }
        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
        }

        let url = format!("{}/v1/messages", self.base_url);
        tracing::debug!(provider = "claude", model = %self.model, %url, "sending request");

        let resp = client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::error!(provider = "claude", %url, error = %e, "request failed");
                ProviderError::HttpError(e.to_string())
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            tracing::error!(
                provider = "claude",
                %url,
                %status,
                response_body = %text,
                "bad response from API",
            );
            return Err(ProviderError::BadResponse(format!("{status}: {text}")));
        }

        tracing::debug!(provider = "claude", status = %resp.status(), "stream started");
        let stream = parse_anthropic_sse(resp);
        Ok(stream)
    }
}

fn build_messages(messages: &[ProviderMessage]) -> Vec<serde_json::Value> {
    let mut result = vec![];
    for msg in messages {
        match msg.role {
            ProviderRole::System => continue,
            ProviderRole::User => {
                result.push(serde_json::json!({
                    "role": "user",
                    "content": msg.content,
                }));
            }
            ProviderRole::Assistant => {
                if let Some(tc) = &msg.tool_call {
                    result.push(serde_json::json!({
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": serde_json::from_str::<serde_json::Value>(&tc.args_json).unwrap_or_default(),
                        }],
                    }));
                } else {
                    result.push(serde_json::json!({
                        "role": "assistant",
                        "content": msg.content,
                    }));
                }
            }
            ProviderRole::Tool => {
                if let Some(tr) = &msg.tool_result {
                    result.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tr.id,
                            "content": tr.output,
                            "is_error": tr.is_error,
                        }],
                    }));
                }
            }
        }
    }
    result
}

fn build_tools(tools: &[ToolSchema]) -> Vec<serde_json::Value> {
    let len = tools.len();
    tools
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut tool = serde_json::json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters,
            });
            if i == len - 1 {
                tool["cache_control"] = serde_json::json!({"type": "ephemeral"});
            }
            tool
        })
        .collect()
}

fn parse_anthropic_sse(resp: reqwest::Response) -> EventStream {
    use futures::StreamExt;

    let stream = async_stream::stream! {
        let mut bytes_stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_args = String::new();
        let mut input_tokens: u64 = 0;
        let mut output_tokens: u64 = 0;
        let mut cache_creation_tokens: u64 = 0;
        let mut cache_read_tokens: u64 = 0;

        while let Some(chunk) = bytes_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(provider = "claude", error = %e, "stream read error");
                    yield Event::Error(e.to_string());
                    return;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find("\n\n") {
                let block = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                for line in block.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            continue;
                        }
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                            let event_type = json.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            match event_type {
                                "content_block_start" => {
                                    if let Some(cb) = json.get("content_block") {
                                        let cb_type = cb.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                        if cb_type == "tool_use" {
                                            current_tool_id = cb.get("id").and_then(|v| v.as_str()).unwrap_or("").into();
                                            current_tool_name = cb.get("name").and_then(|v| v.as_str()).unwrap_or("").into();
                                            current_tool_args.clear();
                                        }
                                    }
                                }
                                "content_block_delta" => {
                                    if let Some(delta) = json.get("delta") {
                                        let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                        match delta_type {
                                            "text_delta" => {
                                                if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                                    yield Event::Token(text.into());
                                                }
                                            }
                                            "input_json_delta" => {
                                                if let Some(partial) = delta.get("partial_json").and_then(|t| t.as_str()) {
                                                    current_tool_args.push_str(partial);
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                "content_block_stop"
                                    if !current_tool_name.is_empty() => {
                                        yield Event::ToolCall {
                                            id: std::mem::take(&mut current_tool_id),
                                            name: std::mem::take(&mut current_tool_name),
                                            args_json: if current_tool_args.is_empty() { "{}".into() } else { std::mem::take(&mut current_tool_args) },
                                        };
                                    }
                                "message_start" => {
                                    if let Some(usage) = json.get("message").and_then(|m| m.get("usage")) {
                                        input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                        cache_creation_tokens = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                        cache_read_tokens = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                    }
                                }
                                "message_delta" => {
                                    if let Some(usage) = json.get("usage") {
                                        output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                    }
                                    let stop_reason = json.get("delta")
                                        .and_then(|d| d.get("stop_reason"))
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("end_turn");
                                    let reason = match stop_reason {
                                        "end_turn" => StopReason::EndTurn,
                                        "max_tokens" => StopReason::MaxTokens,
                                        "tool_use" => StopReason::ToolUse,
                                        "stop_sequence" => StopReason::StopSequence,
                                        other => StopReason::Other(other.into()),
                                    };
                                    yield Event::Done {
                                        stop_reason: reason,
                                        usage: Usage { input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens },
                                    };
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    };

    Box::pin(stream)
}
