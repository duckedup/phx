use crate::tui::app::App;

/// Side effects returned by `update()` and executed by the runtime.
/// `update()` is synchronous — anything async is a Cmd.
#[derive(Debug)]
pub enum Cmd {
    None,
    StartConversation(String),
    RunCommand(String),
    SendAgentMessage {
        session_id: String,
        text: String,
    },
    ResumeSession(String),
    RunToolCommand {
        tool_name: String,
        args_json: String,
    },
    ApplyReload(crate::tui::app::ReloadOutput),
    Batch(Vec<Cmd>),
}

impl Cmd {
    pub async fn execute(self, app: &mut App) {
        match self {
            Cmd::None => {}
            Cmd::StartConversation(text) => {
                if app.remote.is_some() {
                    crate::tui::conversation::spawn_remote_conversation(app, text);
                } else {
                    crate::tui::conversation::start_conversation(app, text);
                }
            }
            Cmd::RunCommand(input) => {
                crate::tui::commands::handle_command(app, &input).await;
            }
            Cmd::SendAgentMessage { session_id, text } => {
                app.session_pool.try_send_message(&session_id, &text);
            }
            Cmd::ResumeSession(session_id) => {
                if app.remote.is_some() {
                    crate::tui::conversation::resume_remote_session(app, &session_id);
                } else {
                    crate::tui::conversation::resume_session(app, &session_id).await;
                }
            }
            Cmd::RunToolCommand {
                tool_name,
                args_json,
            } => {
                app.invoke_tool_command(&tool_name, &args_json).await;
            }
            Cmd::ApplyReload(output) => {
                crate::tui::reload::apply_reload(app, output);
                app.is_reloading = false;
            }
            Cmd::Batch(cmds) => {
                for cmd in cmds {
                    Box::pin(cmd.execute(app)).await;
                }
            }
        }
    }
}
