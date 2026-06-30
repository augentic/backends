//! Key/PATH-gated live integration test for the cursor backend — RFC wasi-model
//! "run 3" (the spawned-agent acceptance gate).
//!
//! Mirrors the genai backend's `live.rs`: it spawns a real `cursor-agent`
//! against a node-local workspace and parses the validated answer back through the
//! `augentic:model/completion` boundary.
//!
//! It is skipped unless `OMNIA_CURSOR_LIVE=1` is set (alongside an installed,
//! authenticated `cursor-agent` — `CURSOR_API_KEY` or a prior `cursor-agent
//! login` — and optionally `OMNIA_MODEL` / `OMNIA_WORKSPACE`), so it never runs or
//! spawns a process in CI.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use futures::FutureExt as _;
use omnia::Backend as _;
use omnia_cursor::{Client, ConnectOptions};
use omnia_wasi_model::{
    Answer, DirEntry, FutureResult, Format, JsonSchemaSpec, PreparedPrompt, Prompt,
    Reference, ResponseFormat, Sections, ToolGrants, ToolHost, VerifyReport, WasiModelCtx,
};
use serde_json::json;

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
             cursor-agent and optionally OMNIA_MODEL / OMNIA_WORKSPACE) to exercise the \
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
        model: std::env::var("OMNIA_MODEL").ok(),
        workspace: Some(workspace.to_string_lossy().into_owned()),
        timeout_secs: 300,
    })
    .await?;

    let request =
        PreparedPrompt::assemble(verdict_prompt()).expect("assemble verdict prompt");
    let answer: Answer =
        client.complete(request, Arc::new(NoopToolHost)).await.map_err(|e| {
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
