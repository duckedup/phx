use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[repr(i32)]
pub enum ErrorCode {
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,
}

pub const CONFIG_GET: &str = "config.get";
pub const COMMAND_LIST: &str = "command.list";
pub const COMMAND_DISPATCH: &str = "command.dispatch";
pub const SESSION_SEND: &str = "session.send";
pub const SESSION_LIST: &str = "session.list";
pub const SESSION_RESUME: &str = "session.resume";
pub const APPLY_MODEL_CHOICE: &str = "command.applyModelChoice";
pub const APPLY_SESSION_CHOICE: &str = "command.applySessionChoice";
pub const ADD_MODEL: &str = "command.addModel";

pub async fn write_success(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: &serde_json::Value,
    result: serde_json::Value,
) -> anyhow::Result<()> {
    let resp = serde_json::json!({
        "id": id,
        "result": result,
    });
    let mut line = serde_json::to_vec(&resp)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn write_error(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: &serde_json::Value,
    code: ErrorCode,
    message: &str,
) -> anyhow::Result<()> {
    let resp = serde_json::json!({
        "id": id,
        "error": {
            "code": code as i32,
            "message": message,
        },
    });
    let mut line = serde_json::to_vec(&resp)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn write_event_line(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: &serde_json::Value,
    event_type: &str,
    data: serde_json::Value,
) -> anyhow::Result<()> {
    let resp = serde_json::json!({
        "id": id,
        "event": event_type,
        "data": data,
    });
    let mut line = serde_json::to_vec(&resp)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn write_token_line(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: &serde_json::Value,
    text: &str,
) -> anyhow::Result<()> {
    write_event_line(writer, id, "token", serde_json::json!({"text": text})).await
}

pub async fn write_tool_call_line(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: &serde_json::Value,
    tool_id: &str,
    name: &str,
) -> anyhow::Result<()> {
    write_event_line(
        writer,
        id,
        "tool_call",
        serde_json::json!({"id": tool_id, "name": name}),
    )
    .await
}

pub async fn write_tool_result_line(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: &serde_json::Value,
    tool_id: &str,
    output: &str,
) -> anyhow::Result<()> {
    write_event_line(
        writer,
        id,
        "tool_result",
        serde_json::json!({"id": tool_id, "output": output}),
    )
    .await
}

pub async fn write_done_line(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: &serde_json::Value,
) -> anyhow::Result<()> {
    write_event_line(writer, id, "done", serde_json::json!({})).await
}

pub async fn write_error_line(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: &serde_json::Value,
    message: &str,
) -> anyhow::Result<()> {
    write_event_line(writer, id, "error", serde_json::json!({"message": message})).await
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn parse_request() {
        let json = r#"{"id":1,"method":"config.get","params":{}}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "config.get");
        assert_eq!(req.id, serde_json::json!(1));
    }

    #[test]
    fn parse_request_no_params() {
        let json = r#"{"id":"abc","method":"command.list"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "command.list");
        assert!(req.params.is_null());
    }

    #[tokio::test]
    async fn write_success_line() {
        let mut buf = Vec::new();
        write_success(
            &mut buf,
            &serde_json::json!(1),
            serde_json::json!({"ok": true}),
        )
        .await
        .unwrap();
        let line = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["result"]["ok"], true);
    }

    #[tokio::test]
    async fn write_error_line_format() {
        let mut buf = Vec::new();
        write_error(
            &mut buf,
            &serde_json::json!(2),
            ErrorCode::MethodNotFound,
            "unknown method",
        )
        .await
        .unwrap();
        let line = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["error"]["code"], -32601);
        assert_eq!(parsed["error"]["message"], "unknown method");
    }

    #[tokio::test]
    async fn write_token_line_format() {
        let mut buf = Vec::new();
        write_token_line(&mut buf, &serde_json::json!(3), "hello")
            .await
            .unwrap();
        let line = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["event"], "token");
        assert_eq!(parsed["data"]["text"], "hello");
    }

    #[tokio::test]
    async fn write_done_line_format() {
        let mut buf = Vec::new();
        write_done_line(&mut buf, &serde_json::json!(4))
            .await
            .unwrap();
        let line = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["event"], "done");
    }
}
