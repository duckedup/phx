use async_trait::async_trait;

use crate::config::schema::ProviderProfile;
use crate::providers::traits::*;

pub struct OpenAIProvider {
    model: String,
    api_key: String,
    base_url: String,
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
        .unwrap_or_else(|| "https://api.openai.com".into());

    Ok(Box::new(OpenAIProvider {
        model: profile.model.clone(),
        api_key,
        base_url,
    }))
}

#[async_trait]
impl Provider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn send(&self, opts: SendOptions) -> Result<EventStream, ProviderError> {
        let client = reqwest::Client::new();

        let messages = build_messages(&opts);
        let tools = build_tools(&opts.tools);

        let mut body = serde_json::json!({
            "model": self.model,
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": messages,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
        }

        let resp = client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::HttpError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::BadResponse(format!("{status}: {text}")));
        }

        Ok(parse_openai_sse(resp))
    }
}

fn build_messages(opts: &SendOptions) -> Vec<serde_json::Value> {
    let mut result = vec![];

    let sys = opts.system_prompt.join("\n\n");
    if !sys.is_empty() {
        result.push(serde_json::json!({"role": "system", "content": sys}));
    }

    for msg in &opts.messages {
        match msg.role {
            ProviderRole::System => {
                result.push(serde_json::json!({"role": "system", "content": msg.content}));
            }
            ProviderRole::User => {
                result.push(serde_json::json!({"role": "user", "content": msg.content}));
            }
            ProviderRole::Assistant => {
                if let Some(tc) = &msg.tool_call {
                    result.push(serde_json::json!({
                        "role": "assistant",
                        "tool_calls": [{
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.args_json,
                            }
                        }],
                    }));
                } else {
                    result.push(serde_json::json!({"role": "assistant", "content": msg.content}));
                }
            }
            ProviderRole::Tool => {
                if let Some(tr) = &msg.tool_result {
                    result.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tr.id,
                        "content": tr.output,
                    }));
                }
            }
        }
    }
    result
}

fn build_tools(tools: &[ToolSchema]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect()
}

fn parse_openai_sse(resp: reqwest::Response) -> EventStream {
    use futures::StreamExt;

    let stream = async_stream::stream! {
        let mut bytes_stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut tool_calls: std::collections::HashMap<usize, (String, String, String)> = std::collections::HashMap::new();
        let mut input_tokens: u64 = 0;
        let mut output_tokens: u64 = 0;
        let mut cache_read_tokens: u64 = 0;
        let mut stop_reason = StopReason::EndTurn;

        while let Some(chunk) = bytes_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    yield Event::Error(e.to_string());
                    return;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                let Some(data) = line.strip_prefix("data: ") else { continue };
                if data == "[DONE]" {
                    for (_, (id, name, args)) in std::mem::take(&mut tool_calls) {
                        yield Event::ToolCall {
                            id,
                            name,
                            args_json: if args.is_empty() { "{}".into() } else { args },
                        };
                    }
                    yield Event::Done {
                        stop_reason: stop_reason.clone(),
                        usage: Usage { input_tokens, output_tokens, cache_creation_tokens: 0, cache_read_tokens },
                    };
                    return;
                }

                let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else { continue };

                if let Some(usage) = json.get("usage") {
                    input_tokens = usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(input_tokens);
                    output_tokens = usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(output_tokens);
                    if let Some(details) = usage.get("prompt_tokens_details") {
                        cache_read_tokens = details.get("cached_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    }
                }

                let Some(choices) = json.get("choices").and_then(|c| c.as_array()) else { continue };
                for choice in choices {
                    if let Some(finish) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                        stop_reason = match finish {
                            "stop" => StopReason::EndTurn,
                            "length" => StopReason::MaxTokens,
                            "tool_calls" => StopReason::ToolUse,
                            other => StopReason::Other(other.into()),
                        };
                    }

                    let Some(delta) = choice.get("delta") else { continue };

                    if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                        && !content.is_empty() {
                            yield Event::Token(content.into());
                        }

                    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tcs {
                            let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            let entry = tool_calls.entry(idx).or_insert_with(|| (String::new(), String::new(), String::new()));
                            if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                entry.0 = id.into();
                            }
                            if let Some(func) = tc.get("function") {
                                if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                                    entry.1 = name.into();
                                }
                                if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                                    entry.2.push_str(args);
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    Box::pin(stream)
}
