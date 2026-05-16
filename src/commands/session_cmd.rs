use std::path::Path;

use crate::commands::dispatcher::{CommandResult, SessionChoice};
use crate::store::session_store::SessionStore;

pub async fn handle_resume(store: &SessionStore, project: &Path) -> CommandResult {
    match store.list(project).await {
        Ok(sessions) => {
            let choices: Vec<SessionChoice> = sessions
                .into_iter()
                .map(|s| SessionChoice {
                    id: s.id.0,
                    display_name: s.display_name,
                    provider: s.provider,
                    model: s.model,
                })
                .collect();
            if choices.is_empty() {
                CommandResult::Message("No sessions to resume.".into())
            } else {
                CommandResult::SessionPicker(choices)
            }
        }
        Err(e) => CommandResult::Error(format!("failed to list sessions: {e}")),
    }
}

pub fn handle_clear() -> CommandResult {
    CommandResult::ClearSession
}

pub fn handle_compact() -> CommandResult {
    CommandResult::CompactSession
}
