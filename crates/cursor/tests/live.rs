//! Key/PATH-gated live integration test for the cursor backend — RFC wasi-model
//! "run 3" (the spawned-agent acceptance gate).
//!
//! Mirrors the genai backend's `live.rs`: it spawns a real `cursor-agent`
//! against a node-local workspace and parses the validated answer back through the
//! `omnia:model/completion` boundary.
//!
//! It is skipped unless `OMNIA_CURSOR_LIVE=1` is set (alongside an installed,
//! authenticated `cursor-agent` — `CURSOR_API_KEY` or a prior `cursor-agent
//! login` — and optionally `CURSOR_MODEL` / `OMNIA_WORKSPACE`), so it never runs or
//! spawns a process in CI.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use futures::FutureExt as _;
use omnia::Backend as _;
use omnia_cursor::{Client, ConnectOptions};
use omnia_wasi_model::{
    Answer, DirEntry, Format, FutureResult, JsonSchemaSpec, PreparedPrompt, Prompt, Reference,
    ResponseFormat, Sections, ToolGrants, ToolHost, VerifyReport, WasiModelCtx,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

/// A no-op `ToolHost`: the cursor backend owns its own loop and ignores it.
#[derive(Debug)]
struct NoopToolHost;

impl ToolHost for NoopToolHost {
    fn resolve(&self, _reference: Reference) -> FutureResult<Vec<u8>> {
        async { Err(anyhow::anyhow!("cursor ignores the tool host")) }.boxed()
    }

    fn read(&self, _path: String) -> FutureResult<Vec<u8>> {
        async { Err(anyhow::anyhow!("cursor ignores the tool host")) }.boxed()
    }

    fn list(&self, _path: String) -> FutureResult<Vec<DirEntry>> {
        async { Err(anyhow::anyhow!("cursor ignores the tool host")) }.boxed()
    }

    fn write(&self, _path: String, _bytes: Vec<u8>) -> FutureResult<()> {
        async { Err(anyhow::anyhow!("cursor ignores the tool host")) }.boxed()
    }

    fn verify(&self, _check: String) -> FutureResult<VerifyReport> {
        async { Err(anyhow::anyhow!("cursor ignores the tool host")) }.boxed()
    }
}

fn verdict_prompt() -> Prompt {
    Prompt {
        model: None,
        system: Some(
            "You are a terse judge. Decide whether the candidate passes and reply with the \
             required JSON object."
                .to_owned(),
        ),
        messages: vec![],
        sections: Some(Sections {
            role: None,
            task: "Judge the trivial candidate and return a verdict of \"pass\" with a one-line \
                   reason."
                .to_owned(),
            context: Some("The candidate is a no-op; it should pass.".to_owned()),
            constraints: vec![],
            examples: vec![],
            variables: vec![],
        }),
        generation: None,
        response_format: ResponseFormat {
            kind: Format::JsonSchema,
            json_schema: Some(JsonSchemaSpec {
                name: "verdict".to_owned(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "verdict": { "type": "string", "enum": ["pass", "fail"] },
                        "reason": { "type": "string" },
                    },
                    "required": ["verdict", "reason"],
                    "additionalProperties": false,
                })
                .to_string(),
                strict: Some(true),
            }),
        },
        tools: vec![],
        tool_choice: None,
        metadata: vec![],
        grants: ToolGrants {
            references: None,
            workspace: None,
            verify: vec![],
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_cursor_completes() -> Result<()> {
    if std::env::var_os("OMNIA_CURSOR_LIVE").is_none() {
        eprintln!(
            "skipping live cursor run 3: set OMNIA_CURSOR_LIVE=1 (plus an installed, authenticated \
             cursor-agent and optionally CURSOR_MODEL / OMNIA_WORKSPACE) to exercise the \
             spawned-agent gate"
        );
        return Ok(());
    }

    let workspace = std::env::var_os("OMNIA_WORKSPACE").map_or_else(
        || std::env::temp_dir().join(format!("omnia-cursor-live-ws-{}", std::process::id())),
        PathBuf::from,
    );
    std::fs::create_dir_all(&workspace)?;

    let client = Client::connect_with(ConnectOptions {
        model: std::env::var("CURSOR_MODEL").ok(),
        workspace: Some(workspace.to_string_lossy().into_owned()),
        timeout_secs: 300,
        mcp_url: None,
    })
    .await?;

    let request = PreparedPrompt::try_from(verdict_prompt()).expect("assemble verdict prompt");
    let answer: Answer = client.complete(request, Arc::new(NoopToolHost)).await.map_err(|e| {
        anyhow::anyhow!(
            "live cursor completion failed (is cursor-agent installed and authed?): {e}"
        )
    })?;

    assert!(answer.value.is_object(), "run-3 answer must be a JSON object: {:?}", answer.value);
    assert!(
        answer.value.get("verdict").and_then(serde_json::Value::as_str).is_some(),
        "run-3 answer must carry a string verdict: {:?}",
        answer.value
    );

    Ok(())
}

/// A unique token the in-test MCP server returns. The agent can only produce it
/// by calling the MCP tool, so its presence in the answer proves the wiring.
const SENTINEL: &str = "OMNIA-MCP-SENTINEL-4e9c1a7b";

/// A prompt that can only be answered by calling the `read_secret` MCP tool.
fn secret_prompt() -> Prompt {
    Prompt {
        model: None,
        system: Some("Answer only from tools. Do not guess or fabricate values.".to_owned()),
        messages: vec![],
        sections: Some(Sections {
            role: None,
            task: "Call the `read_secret` tool on the `omnia` MCP server to obtain the project \
                   secret token, then return it unchanged."
                .to_owned(),
            context: None,
            constraints: vec![],
            examples: vec![],
            variables: vec![],
        }),
        generation: None,
        response_format: ResponseFormat {
            kind: Format::JsonSchema,
            json_schema: Some(JsonSchemaSpec {
                name: "secret".to_owned(),
                schema: json!({
                    "type": "object",
                    "properties": { "secret": { "type": "string" } },
                    "required": ["secret"],
                    "additionalProperties": false,
                })
                .to_string(),
                strict: Some(true),
            }),
        },
        tools: vec![],
        tool_choice: None,
        metadata: vec![],
        grants: ToolGrants {
            references: None,
            workspace: None,
            verify: vec![],
        },
    }
}

// A minimal, dependency-free MCP Streamable HTTP server for the live test. It
// answers `initialize` / `tools/list` / `tools/call` and returns `SENTINEL` from
// its single `read_secret` tool. Hand-rolled over tokio so the backends crate
// takes on no new (vet-gated) dependency.
async fn serve_mcp(listener: TcpListener) {
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

    // Read the request head, then the body named by `Content-Length`.
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

// Answer one JSON-RPC message, returning the HTTP status line and body bytes.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_cursor_uses_mcp() -> Result<()> {
    if std::env::var_os("OMNIA_CURSOR_LIVE").is_none() {
        eprintln!(
            "skipping live cursor MCP run: set OMNIA_CURSOR_LIVE=1 (plus an installed, \
             authenticated cursor-agent) to exercise the MCP wiring end to end"
        );
        return Ok(());
    }

    // Stand up the in-test MCP server on an ephemeral loopback port.
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(serve_mcp(listener));

    let workspace =
        std::env::temp_dir().join(format!("omnia-cursor-mcp-live-{}", std::process::id()));
    std::fs::create_dir_all(&workspace)?;

    let client = Client::connect_with(ConnectOptions {
        model: std::env::var("CURSOR_MODEL").ok(),
        workspace: Some(workspace.to_string_lossy().into_owned()),
        timeout_secs: 300,
        mcp_url: Some(format!("http://127.0.0.1:{port}/mcp")),
    })
    .await?;

    let request = PreparedPrompt::try_from(secret_prompt()).expect("assemble secret prompt");
    let answer: Answer = client
        .complete(request, Arc::new(NoopToolHost))
        .await
        .map_err(|e| anyhow::anyhow!("live cursor MCP completion failed: {e}"))?;

    assert!(
        answer.value.to_string().contains(SENTINEL),
        "the agent must return the MCP-provided secret; got: {:?}",
        answer.value
    );

    Ok(())
}
