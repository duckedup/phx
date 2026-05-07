use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use super::handle::PluginHandle;

const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    Token,
    ToolCallStart,
    ToolCallEnd,
    CompactionStart,
    MessagePreSend,
    ContextLoaded,
    ContextCompacted,
}

impl HookEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::SessionEnd => "session_end",
            Self::Token => "token",
            Self::ToolCallStart => "tool_call_start",
            Self::ToolCallEnd => "tool_call_end",
            Self::CompactionStart => "compaction_start",
            Self::MessagePreSend => "message_pre_send",
            Self::ContextLoaded => "context_loaded",
            Self::ContextCompacted => "context_compacted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "session_start" => Some(Self::SessionStart),
            "session_end" => Some(Self::SessionEnd),
            "token" => Some(Self::Token),
            "tool_call_start" => Some(Self::ToolCallStart),
            "tool_call_end" => Some(Self::ToolCallEnd),
            "compaction_start" => Some(Self::CompactionStart),
            "message_pre_send" => Some(Self::MessagePreSend),
            "context_loaded" => Some(Self::ContextLoaded),
            "context_compacted" => Some(Self::ContextCompacted),
            _ => None,
        }
    }

    pub fn is_blockable(&self) -> bool {
        matches!(
            self,
            Self::ToolCallStart | Self::ToolCallEnd | Self::CompactionStart | Self::MessagePreSend
        )
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum HookAction {
    #[default]
    Allow,
    Block {
        reason: String,
    },
    Modify {
        data: serde_json::Value,
    },
}

struct PluginSubscription {
    handle: Arc<PluginHandle>,
    subscribe: HashSet<HookEvent>,
    can_block: HashSet<HookEvent>,
}

pub struct HookDispatcher {
    plugins: RwLock<Vec<PluginSubscription>>,
}

impl HookDispatcher {
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(Vec::new()),
        }
    }

    pub async fn register(
        &self,
        handle: Arc<PluginHandle>,
        subscribe: Vec<String>,
        can_block: Vec<String>,
    ) {
        let subscribe_set: HashSet<HookEvent> = subscribe
            .iter()
            .filter_map(|s| HookEvent::parse(s))
            .collect();
        let block_set: HashSet<HookEvent> = can_block
            .iter()
            .filter_map(|s| HookEvent::parse(s))
            .collect();

        let mut plugins = self.plugins.write().await;
        plugins.push(PluginSubscription {
            handle,
            subscribe: subscribe_set,
            can_block: block_set,
        });
    }

    pub async fn notify(&self, event: HookEvent, data: serde_json::Value) {
        let plugins = self.plugins.read().await;
        for sub in plugins.iter() {
            if !sub.subscribe.contains(&event) {
                continue;
            }
            let params = serde_json::json!({
                "event": event.as_str(),
                "data": data,
            });
            if let Err(e) = sub.handle.notify("event/notify", params).await {
                tracing::warn!("plugin '{}' notify failed: {e}", sub.handle.name());
            }
        }
    }

    pub async fn hook(&self, event: HookEvent, data: serde_json::Value) -> HookAction {
        if !event.is_blockable() {
            self.notify(event, data).await;
            return HookAction::Allow;
        }

        let plugins = self.plugins.read().await;
        let mut current_data = data;

        for sub in plugins.iter() {
            if !sub.can_block.contains(&event) {
                if sub.subscribe.contains(&event) {
                    let params = serde_json::json!({
                        "event": event.as_str(),
                        "data": current_data,
                    });
                    let _ = sub.handle.notify("event/notify", params).await;
                }
                continue;
            }

            let params = serde_json::json!({
                "event": event.as_str(),
                "data": current_data,
            });

            let result = sub
                .handle
                .request_with_timeout("event/hook", params, DEFAULT_HOOK_TIMEOUT)
                .await;

            match result {
                Ok(value) => {
                    if let Ok(action) = serde_json::from_value::<HookAction>(value) {
                        match &action {
                            HookAction::Block { .. } => return action,
                            HookAction::Modify { data } => {
                                current_data = data.clone();
                            }
                            HookAction::Allow => {}
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "plugin '{}' hook timed out or failed: {e}, allowing",
                        sub.handle.name()
                    );
                }
            }
        }

        HookAction::Allow
    }

    pub async fn has_subscribers(&self, event: &HookEvent) -> bool {
        let plugins = self.plugins.read().await;
        plugins.iter().any(|s| s.subscribe.contains(event))
    }
}

impl Default for HookDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_event_roundtrip() {
        for event in [
            HookEvent::SessionStart,
            HookEvent::ToolCallStart,
            HookEvent::CompactionStart,
            HookEvent::MessagePreSend,
        ] {
            let s = event.as_str();
            let back = HookEvent::parse(s).unwrap();
            assert_eq!(back, event);
        }
    }

    #[test]
    fn blockable_events() {
        assert!(HookEvent::ToolCallStart.is_blockable());
        assert!(HookEvent::ToolCallEnd.is_blockable());
        assert!(HookEvent::CompactionStart.is_blockable());
        assert!(HookEvent::MessagePreSend.is_blockable());
        assert!(!HookEvent::Token.is_blockable());
        assert!(!HookEvent::SessionStart.is_blockable());
    }

    #[test]
    fn unknown_event_returns_none() {
        assert!(HookEvent::parse("bogus").is_none());
    }

    #[test]
    fn hook_action_deserialize() {
        let allow: HookAction = serde_json::from_str(r#"{"action": "allow"}"#).unwrap();
        assert!(matches!(allow, HookAction::Allow));

        let block: HookAction =
            serde_json::from_str(r#"{"action": "block", "reason": "dangerous"}"#).unwrap();
        assert!(matches!(block, HookAction::Block { .. }));

        let modify: HookAction =
            serde_json::from_str(r#"{"action": "modify", "data": {"args": {}}}"#).unwrap();
        assert!(matches!(modify, HookAction::Modify { .. }));
    }

    #[tokio::test]
    async fn empty_dispatcher_allows() {
        let dispatcher = HookDispatcher::new();
        let action = dispatcher
            .hook(HookEvent::ToolCallStart, serde_json::json!({}))
            .await;
        assert!(matches!(action, HookAction::Allow));
    }

    #[tokio::test]
    async fn no_subscribers() {
        let dispatcher = HookDispatcher::new();
        assert!(!dispatcher.has_subscribers(&HookEvent::ToolCallStart).await);
    }
}
