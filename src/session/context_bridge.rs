use crate::shared::context_types::{
    ContextMessage, ContextMutation, ContextToolCall, ContextToolResult, SessionContext,
};

use super::agent_loop::Session;
use super::message::{Message, Role};

pub fn build_session_context(session: &Session, system_prompt: &str) -> SessionContext {
    let messages = session
        .messages
        .iter()
        .map(|m| ContextMessage {
            role: match m.role {
                Role::System => "system".into(),
                Role::User => "user".into(),
                Role::Assistant => "assistant".into(),
                Role::ToolCall => "tool_call".into(),
                Role::ToolResult => "tool_result".into(),
            },
            content: m.content.clone(),
            tool_call: m.tool_call.as_ref().map(|tc| ContextToolCall {
                id: tc.id.clone(),
                name: tc.name.clone(),
                args_json: tc.args_json.clone(),
            }),
            tool_result: m.tool_result.as_ref().map(|tr| ContextToolResult {
                id: tr.id.clone(),
                output: tr.output.clone(),
                is_error: tr.is_error,
            }),
        })
        .collect();

    let activated_skills: Vec<String> = session
        .context_state
        .activated_skills
        .iter()
        .cloned()
        .collect();

    SessionContext {
        messages,
        system_prompt: system_prompt.into(),
        activated_skills,
        touched_files: vec![],
    }
}

pub fn apply_mutation(session: &mut Session, mutation: ContextMutation) {
    match mutation {
        ContextMutation::InjectContext { content } => {
            session.add_message(Message::system(&content));
        }
        ContextMutation::AppendMessage { message } => {
            let role = match message.role.as_str() {
                "system" => Role::System,
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => Role::System,
            };
            session.add_message(Message {
                role,
                content: message.content,
                tool_call: None,
                tool_result: None,
            });
        }
        ContextMutation::Compact { keep_last } => {
            let len = session.messages.len();
            if keep_last < len {
                let drain_end = len - keep_last;
                session.messages.drain(1..drain_end);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::SessionProfile;
    use crate::session::agent_loop::Session;
    use crate::store::session_store::SessionId;

    #[test]
    fn build_context_from_empty_session() {
        let session = Session::new(SessionId::new(), SessionProfile::default());
        let ctx = build_session_context(&session, "You are an assistant.");
        assert!(ctx.messages.is_empty());
        assert_eq!(ctx.system_prompt, "You are an assistant.");
    }

    #[test]
    fn build_context_with_messages() {
        let mut session = Session::new(SessionId::new(), SessionProfile::default());
        session.add_message(Message::user("hello"));
        session.add_message(Message::assistant("hi"));
        let ctx = build_session_context(&session, "");
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.messages[0].role, "user");
        assert_eq!(ctx.messages[1].role, "assistant");
    }

    #[test]
    fn apply_inject_context() {
        let mut session = Session::new(SessionId::new(), SessionProfile::default());
        session.add_message(Message::user("hello"));
        apply_mutation(
            &mut session,
            ContextMutation::InjectContext {
                content: "You are now in plan mode.".into(),
            },
        );
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[1].role, Role::System);
        assert!(session.messages[1].content.contains("plan mode"));
    }
}
