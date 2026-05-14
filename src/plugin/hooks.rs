use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use async_trait::async_trait;

pub use crate::shared::hook_types::{HookAction, HookEvent};

use super::handle::PluginHandle;

const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(5);

#[async_trait]
pub trait HookSubscriber: Send + Sync {
    async fn on_notify(&self, event: &str, data: serde_json::Value) -> anyhow::Result<()>;
    async fn on_hook(&self, event: &str, data: serde_json::Value) -> anyhow::Result<HookAction>;
    fn name(&self) -> &str;
}

#[async_trait]
impl HookSubscriber for PluginHandle {
    async fn on_notify(&self, _event: &str, data: serde_json::Value) -> anyhow::Result<()> {
        self.notify("event/notify", data).await
    }

    async fn on_hook(&self, _event: &str, data: serde_json::Value) -> anyhow::Result<HookAction> {
        let value = self
            .request_with_timeout("event/hook", data, DEFAULT_HOOK_TIMEOUT)
            .await?;
        Ok(serde_json::from_value::<HookAction>(value).unwrap_or_default())
    }

    fn name(&self) -> &str {
        PluginHandle::name(self)
    }
}

struct HookRegistration {
    subscriber: Arc<dyn HookSubscriber>,
    subscribe: HashSet<HookEvent>,
    can_block: HashSet<HookEvent>,
}

pub struct HookDispatcher {
    registrations: RwLock<Vec<HookRegistration>>,
}

impl HookDispatcher {
    pub fn new() -> Self {
        Self {
            registrations: RwLock::new(Vec::new()),
        }
    }

    pub async fn register(
        &self,
        handle: Arc<PluginHandle>,
        subscribe: Vec<String>,
        can_block: Vec<String>,
    ) {
        self.register_subscriber(handle as Arc<dyn HookSubscriber>, subscribe, can_block)
            .await;
    }

    pub async fn register_subscriber(
        &self,
        subscriber: Arc<dyn HookSubscriber>,
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

        let mut regs = self.registrations.write().await;
        regs.push(HookRegistration {
            subscriber,
            subscribe: subscribe_set,
            can_block: block_set,
        });
    }

    pub async fn notify(&self, event: HookEvent, data: serde_json::Value) {
        let regs = self.registrations.read().await;
        for reg in regs.iter() {
            if !reg.subscribe.contains(&event) {
                continue;
            }
            let params = serde_json::json!({
                "event": event.as_str(),
                "data": data,
            });
            if let Err(e) = reg.subscriber.on_notify(event.as_str(), params).await {
                tracing::warn!(
                    "hook subscriber '{}' notify failed: {e}",
                    reg.subscriber.name()
                );
            }
        }
    }

    pub async fn hook(&self, event: HookEvent, data: serde_json::Value) -> HookAction {
        if !event.is_blockable() {
            self.notify(event, data).await;
            return HookAction::Allow;
        }

        let regs = self.registrations.read().await;
        let mut current_data = data;

        for reg in regs.iter() {
            if !reg.can_block.contains(&event) {
                if reg.subscribe.contains(&event) {
                    let params = serde_json::json!({
                        "event": event.as_str(),
                        "data": current_data,
                    });
                    let _ = reg.subscriber.on_notify(event.as_str(), params).await;
                }
                continue;
            }

            let params = serde_json::json!({
                "event": event.as_str(),
                "data": current_data,
            });

            match reg.subscriber.on_hook(event.as_str(), params).await {
                Ok(action) => match &action {
                    HookAction::Block { .. } => return action,
                    HookAction::Modify { data } => {
                        current_data = data.clone();
                    }
                    HookAction::Allow => {}
                },
                Err(e) => {
                    tracing::warn!(
                        "hook subscriber '{}' timed out or failed: {e}, allowing",
                        reg.subscriber.name()
                    );
                }
            }
        }

        HookAction::Allow
    }

    pub async fn has_subscribers(&self, event: &HookEvent) -> bool {
        let regs = self.registrations.read().await;
        regs.iter().any(|r| r.subscribe.contains(event))
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
