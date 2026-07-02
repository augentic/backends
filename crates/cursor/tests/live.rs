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

mod support;

use std::path::PathBuf;

use anyhow::Result;
use omnia::Backend as _;
use omnia_cursor::{Client, ConnectOptions};
use omnia_wasi_model::{
    Answer, Format, JsonSchemaSpec, PreparedPrompt, Prompt, ResponseFormat, Sections, ToolGrants,
    WasiModelCtx,
};
use serde_json::json;
use support::{SENTINEL, noop_tool_host, serve};
use tokio::net::TcpListener;

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
        use_worktree: false,
    })
    .await?;

    let request = PreparedPrompt::try_from(verdict_prompt()).expect("assemble verdict prompt");
    let answer: Answer = client.complete(request, noop_tool_host()).await.map_err(|e| {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_cursor_uses_mcp() -> Result<()> {
    if std::env::var_os("OMNIA_CURSOR_LIVE").is_none() {
        eprintln!(
            "skipping live cursor MCP run: set OMNIA_CURSOR_LIVE=1 (plus an installed, \
             authenticated cursor-agent) to exercise the MCP wiring end to end"
        );
        return Ok(());
    }

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(serve(listener));

    let workspace =
        std::env::temp_dir().join(format!("omnia-cursor-mcp-live-{}", std::process::id()));
    std::fs::create_dir_all(&workspace)?;

    let client = Client::connect_with(ConnectOptions {
        model: std::env::var("CURSOR_MODEL").ok(),
        workspace: Some(workspace.to_string_lossy().into_owned()),
        timeout_secs: 300,
        mcp_url: Some(format!("http://127.0.0.1:{port}/mcp")),
        use_worktree: false,
    })
    .await?;

    let request = PreparedPrompt::try_from(secret_prompt()).expect("assemble secret prompt");
    let answer: Answer = client
        .complete(request, noop_tool_host())
        .await
        .map_err(|e| anyhow::anyhow!("live cursor MCP completion failed: {e}"))?;

    assert!(
        answer.value.to_string().contains(SENTINEL),
        "the agent must return the MCP-provided secret; got: {:?}",
        answer.value
    );

    Ok(())
}
