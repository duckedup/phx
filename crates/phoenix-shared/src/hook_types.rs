use serde::{Deserialize, Serialize};

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
}
