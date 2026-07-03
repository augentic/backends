//! Minimal MCP Streamable HTTP server for live integration tests.

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

/// A unique token returned by the test server's `read_secret` tool.
pub const SENTINEL: &str = "OMNIA-MCP-SENTINEL-4e9c1a7b";

/// Accept connections until the listener is closed.
pub async fn serve(listener: TcpListener) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(async move {
            let _ = handle_conn(&mut socket).await;
        });
    }
}

async fn handle_conn(socket: &mut TcpStream) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];

    let header_end = loop {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..read]);
        if let Some(pos) = window_find(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > (1 << 20) {
            return Ok(());
        }
    };
    let content_length = content_length(&String::from_utf8_lossy(&buf[..header_end]));
    while buf.len() < header_end + content_length {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
    }

    let body_end = (header_end + content_length).min(buf.len());
    let (status, body) = mcp_reply(&buf[header_end..body_end]);
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: \
         close\r\n\r\n",
        body.len()
    );
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(&body).await?;
    socket.flush().await
}

fn window_find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn content_length(headers: &str) -> usize {
    headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
}

fn mcp_reply(body: &[u8]) -> (&'static str, Vec<u8>) {
    let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let Some(id) = request.get("id").cloned() else {
        return ("202 Accepted", Vec::new());
    };
    let result = match request.get("method").and_then(Value::as_str).unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "omnia-test", "version": "0" },
        }),
        "ping" => json!({}),
        "tools/list" => json!({
            "tools": [ {
                "name": "read_secret",
                "description": "Return the project secret token.",
                "inputSchema": { "type": "object", "properties": {} },
            } ],
        }),
        "tools/call" => {
            json!({ "content": [ { "type": "text", "text": SENTINEL } ], "isError": false })
        }
        _ => {
            let error = json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": "method not found" },
            });
            return ("200 OK", serde_json::to_vec(&error).unwrap_or_default());
        }
    };
    let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    ("200 OK", serde_json::to_vec(&response).unwrap_or_default())
}
