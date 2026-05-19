use async_trait::async_trait;

use crate::config::ProviderProfile;
use crate::providers::traits::*;

pub struct OllamaProvider {
    model: String,
    base_url: String,
}

pub fn create(profile: &ProviderProfile) -> Result<Box<dyn Provider>, ProviderError> {
    let base_url = profile
        .base_url
        .clone()
        .unwrap_or_else(|| "http://localhost:11434".into());

    Ok(Box::new(OllamaProvider {
        model: profile.model.clone(),
        base_url,
    }))
}

#[async_trait]
impl Provider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn send(&self, opts: SendOptions) -> Result<EventStream, ProviderError> {
        let client = reqwest::Client::new();

        let messages = build_messages(&opts);
        let tools = build_tools(&opts.tools);

        let mut body = serde_json::json!({
            "model": self.model,
            "stream": true,
            "messages": messages,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
        }

        let resp = client
            .post(format!("{}/api/chat", self.base_url))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::HttpError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::BadResponse(format!("{status}: {text}")));
        }

        Ok(parse_ollama_stream(resp))
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
                        "tool_calls": [{"function": {"name": tc.name, "arguments": serde_json::from_str::<serde_json::Value>(&tc.args_json).unwrap_or_default()}}],
                    }));
                } else {
                    result.push(serde_json::json!({"role": "assistant", "content": msg.content}));
                }
            }
            ProviderRole::Tool => {
                if let Some(tr) = &msg.tool_result {
                    result.push(serde_json::json!({"role": "tool", "content": tr.output}));
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

fn parse_ollama_stream(resp: reqwest::Response) -> EventStream {
    use futures::StreamExt;

    let stream = async_stream::stream! {
        let mut bytes_stream = resp.bytes_stream();
        let mut buffer = String::new();

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

                if line.is_empty() { continue; }
                let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else { continue };

                if let Some(msg) = json.get("message") {
                    if let Some(content) = msg.get("content").and_then(|c| c.as_str())
                        && !content.is_empty() {
                            yield Event::Token(content.into());
                        }
                    if let Some(tcs) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tcs {
                            if let Some(func) = tc.get("function") {
                                let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("").into();
                                let args = func.get("arguments").map(|a| a.to_string()).unwrap_or_else(|| "{}".into());
                                yield Event::ToolCall {
                                    id: uuid::Uuid::now_v7().to_string(),
                                    name,
                                    args_json: args,
                                };
                            }
                        }
                    }
                }

                if json.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
                    let input = json.get("prompt_eval_count").and_then(|v| v.as_u64()).unwrap_or(0);
                    let output = json.get("eval_count").and_then(|v| v.as_u64()).unwrap_or(0);
                    yield Event::Done {
                        stop_reason: StopReason::EndTurn,
                        usage: Usage { input_tokens: input, output_tokens: output, ..Default::default() },
                    };
                }
            }
        }
    };

    Box::pin(stream)
}
