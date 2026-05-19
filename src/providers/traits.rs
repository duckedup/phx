use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
    Other(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone)]
pub struct ProviderToolCall {
    pub id: String,
    pub name: String,
    pub args_json: String,
}

#[derive(Debug, Clone)]
pub struct ProviderToolResult {
    pub id: String,
    pub output: String,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub struct ProviderMessage {
    pub role: ProviderRole,
    pub content: String,
    pub tool_call: Option<ProviderToolCall>,
    pub tool_result: Option<ProviderToolResult>,
}

pub struct SendOptions {
    pub messages: Vec<ProviderMessage>,
    pub tools: Vec<ToolSchema>,
    /// System prompt blocks. Each block may receive its own cache breakpoint
    /// on providers that support it (e.g. Anthropic). Other providers join them.
    pub system_prompt: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Token(String),
    ToolCall {
        id: String,
        name: String,
        args_json: String,
    },
    Done {
        stop_reason: StopReason,
        usage: Usage,
    },
    Error(String),
}

pub type EventStream = Pin<Box<dyn Stream<Item = Event> + Send>>;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("missing credential")]
    MissingCredential,
    #[error("HTTP error: {0}")]
    HttpError(String),
    #[error("bad response: {0}")]
    BadResponse(String),
    #[error("cancelled")]
    Cancelled,
    #[error("timeout")]
    Timeout,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn send(&self, opts: SendOptions) -> Result<EventStream, ProviderError>;
}

// --- Mock provider for tests ---

pub struct MockProvider {
    events: Vec<Event>,
    error: Option<String>,
}

impl MockProvider {
    pub fn with_events(events: Vec<Event>) -> Self {
        Self {
            events,
            error: None,
        }
    }

    pub fn with_error(msg: &str) -> Self {
        Self {
            events: vec![],
            error: Some(msg.into()),
        }
    }

    pub fn single_turn(text: &str) -> Self {
        Self::with_events(vec![
            Event::Token(text.into()),
            Event::Done {
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: text.len() as u64,
                    ..Default::default()
                },
            },
        ])
    }

    pub fn tool_call_then_response(
        tool_id: &str,
        tool_name: &str,
        tool_args: &str,
        final_text: &str,
    ) -> Self {
        Self::with_events(vec![
            Event::ToolCall {
                id: tool_id.into(),
                name: tool_name.into(),
                args_json: tool_args.into(),
            },
            Event::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
            },
            Event::Token(final_text.into()),
            Event::Done {
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 20,
                    output_tokens: final_text.len() as u64,
                    ..Default::default()
                },
            },
        ])
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn send(&self, _opts: SendOptions) -> Result<EventStream, ProviderError> {
        if let Some(err) = &self.error {
            return Err(ProviderError::HttpError(err.clone()));
        }
        let events = self.events.clone();
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn mock_provider_streams_events() {
        let provider = MockProvider::with_events(vec![
            Event::Token("hello".into()),
            Event::Token(" world".into()),
            Event::Done {
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 5,
                    output_tokens: 2,
                    ..Default::default()
                },
            },
        ]);

        let stream = provider
            .send(SendOptions {
                messages: vec![],
                tools: vec![],
                system_prompt: vec![],
            })
            .await
            .unwrap();

        let events: Vec<Event> = stream.collect().await;
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], Event::Token(t) if t == "hello"));
        assert!(matches!(&events[2], Event::Done { .. }));
    }

    #[tokio::test]
    async fn mock_provider_error() {
        let provider = MockProvider::with_error("connection refused");
        let result = provider
            .send(SendOptions {
                messages: vec![],
                tools: vec![],
                system_prompt: vec![],
            })
            .await;
        assert!(result.is_err());
    }
}
