use async_trait::async_trait;

use crate::config::ProviderProfile;
use crate::providers::traits::*;

pub struct GoogleProvider {
    model: String,
    api_key: String,
    base_url: String,
    provider_name: String,
}

pub fn create(
    name: &str,
    profile: &ProviderProfile,
    credential: Option<&str>,
) -> Result<Box<dyn Provider>, ProviderError> {
    let api_key = credential
        .ok_or(ProviderError::MissingCredential)?
        .to_string();

    let base_url = profile
        .base_url
        .clone()
        .unwrap_or_else(|| "https://generativelanguage.googleapis.com".into());

    Ok(Box::new(GoogleProvider {
        model: profile.model.clone(),
        api_key,
        base_url,
        provider_name: name.into(),
    }))
}

#[async_trait]
impl Provider for GoogleProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn send(&self, opts: SendOptions) -> Result<EventStream, ProviderError> {
        let client = reqwest::Client::new();

        let contents = build_contents(&opts);
        let tools = build_tools(&opts.tools);

        let mut body = serde_json::json!({
            "contents": contents,
        });

        let sys = opts.system_prompt.join("\n\n");
        if !sys.is_empty() {
            body["system_instruction"] = serde_json::json!({"parts": [{"text": sys}]});
        }

        if !tools.is_empty() {
            body["tools"] = serde_json::json!([{"function_declarations": tools}]);
        }

        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?key={}&alt=sse",
            self.base_url, self.model, self.api_key
        );

        let resp = client
            .post(&url)
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

        Ok(parse_google_sse(resp))
    }
}

fn build_contents(opts: &SendOptions) -> Vec<serde_json::Value> {
    let mut contents = vec![];
    for msg in &opts.messages {
        match msg.role {
            ProviderRole::System => continue,
            ProviderRole::User => {
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": [{"text": msg.content}],
                }));
            }
            ProviderRole::Assistant => {
                if let Some(tc) = &msg.tool_call {
                    contents.push(serde_json::json!({
                        "role": "model",
                        "parts": [{"functionCall": {"name": tc.name, "args": serde_json::from_str::<serde_json::Value>(&tc.args_json).unwrap_or_default()}}],
                    }));
                } else {
                    contents.push(serde_json::json!({
                        "role": "model",
                        "parts": [{"text": msg.content}],
                    }));
                }
            }
            ProviderRole::Tool => {
                if let Some(tr) = &msg.tool_result {
                    contents.push(serde_json::json!({
                        "role": "function",
                        "parts": [{"functionResponse": {"name": "", "response": {"result": tr.output}}}],
                    }));
                }
            }
        }
    }
    contents
}

fn build_tools(tools: &[ToolSchema]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })
        })
        .collect()
}

fn parse_google_sse(resp: reqwest::Response) -> EventStream {
    use futures::StreamExt;

    let stream = async_stream::stream! {
        let mut bytes_stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut total_input: u64 = 0;
        let mut total_output: u64 = 0;

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

                let Some(data) = line.strip_prefix("data: ") else { continue };
                let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else { continue };

                if let Some(usage) = json.get("usageMetadata") {
                    total_input = usage.get("promptTokenCount").and_then(|v| v.as_u64()).unwrap_or(0);
                    total_output = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()).unwrap_or(0);
                }

                let Some(candidates) = json.get("candidates").and_then(|c| c.as_array()) else { continue };
                for candidate in candidates {
                    let Some(content) = candidate.get("content") else { continue };
                    let Some(parts) = content.get("parts").and_then(|p| p.as_array()) else { continue };

                    for part in parts {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            yield Event::Token(text.into());
                        }
                        if let Some(fc) = part.get("functionCall") {
                            let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("").into();
                            let args = fc.get("args").map(|a| a.to_string()).unwrap_or_else(|| "{}".into());
                            yield Event::ToolCall {
                                id: uuid::Uuid::now_v7().to_string(),
                                name,
                                args_json: args,
                            };
                        }
                    }

                    if let Some(finish) = candidate.get("finishReason").and_then(|f| f.as_str()) {
                        let reason = match finish {
                            "STOP" => StopReason::EndTurn,
                            "MAX_TOKENS" => StopReason::MaxTokens,
                            "TOOL_CALLS" | "FUNCTION_CALL" => StopReason::ToolUse,
                            other => StopReason::Other(other.into()),
                        };
                        yield Event::Done {
                            stop_reason: reason,
                            usage: Usage { input_tokens: total_input, output_tokens: total_output, ..Default::default() },
                        };
                    }
                }
            }
        }
    };

    Box::pin(stream)
}
