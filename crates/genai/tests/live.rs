//! Key-gated live integration test for the genai backend — RFC wasi-model
//! "run 2" (the `resolve` acceptance gate).
//!
//! This is the cross-repo companion to omnia's deterministic
//! `resolve_dispatches_to_a_fresh_shelf_per_call` test: that one proves the
//! host→guest dispatch machinery with no network; this one proves the genai
//! backend itself — `Prompt`→`ChatRequest` mapping, the in-process tool loop,
//! `resolve` dispatch through the lent [`ToolHost`], and answer validation —
//! against a real provider, then shows the recorded fixture replays
//! deterministically under [`ModelDefault`].
//!
//! It is skipped unless `OMNI_GENAI_LIVE=1` is set (alongside a provider key
//! such as `OPENAI_API_KEY` and an `OMNI_MODEL`), so it never runs or touches
//! the network in CI.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use anyhow::Result;
use futures::FutureExt as _;
use omnia::Backend as _;
use omnia_genai::Client;
use omnia_wasi_model::{
    BackendAnswer, ConnectOptions as ReplayConnectOptions, DirEntry, FutureResult, ModelDefault,
    Prompt, Recording, Reference, ResponseFormat, ResponseFormatKind, Sections, ToolGrants,
    ToolHost, VerifyReport, WasiModelCtx,
};
use serde_json::Value;

/// Deterministic stand-in for the caller's `references` shelf: `resolve(name)`
/// returns `shelf:{name}` bytes, mirroring the omnia `examples/model` shelf
/// guest. The real host→guest dispatch is exercised in omnia; here we only need
/// the genai backend to drive a `resolve` tool call and consume the result.
#[derive(Debug)]
struct LiveShelf;

impl ToolHost for LiveShelf {
    fn resolve(&self, reference: Reference) -> FutureResult<Vec<u8>> {
        async move { Ok(format!("shelf:{}", reference.name).into_bytes()) }.boxed()
    }

    fn read(&self, _path: String) -> FutureResult<Vec<u8>> {
        async { Err(anyhow::anyhow!("read is unused in this test")) }.boxed()
    }

    fn list(&self, _path: String) -> FutureResult<Vec<DirEntry>> {
        async { Err(anyhow::anyhow!("list is unused in this test")) }.boxed()
    }

    fn write(&self, _path: String, _bytes: Vec<u8>) -> FutureResult<()> {
        async { Err(anyhow::anyhow!("write is unused in this test")) }.boxed()
    }

    fn verify(&self, _check: String) -> FutureResult<VerifyReport> {
        async { Err(anyhow::anyhow!("verify is unused in this test")) }.boxed()
    }
}

/// A prompt that forces a `resolve` tool call (a reference target is granted, so
/// the floor `resolve` tool is advertised) and a JSON-object answer embedding
/// the resolved value.
fn resolve_prompt() -> Prompt {
    Prompt {
        model: None,
        system: Some(
            "Call the `resolve` tool with name \"alpha\" to fetch a value, then reply with a JSON \
             object {\"resolved\": <the exact string the tool returned>}. Use the tool result \
             verbatim; do not invent it."
                .to_owned(),
        ),
        messages: vec![],
        sections: Some(Sections {
            role: None,
            task: "Resolve the reference named \"alpha\" and report what it resolved to."
                .to_owned(),
            context: None,
            constraints: vec![],
            examples: vec![],
            variables: vec![],
        }),
        generation: None,
        response_format: ResponseFormat {
            kind: ResponseFormatKind::JsonObject,
            json_schema: None,
        },
        tools: vec![],
        tool_choice: None,
        metadata: vec![],
        grants: ToolGrants {
            references: Some("shelf".to_owned()),
            working_tree_lent: false,
            verify: vec![],
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_genai_resolves_then_replays() -> Result<()> {
    if std::env::var_os("OMNI_GENAI_LIVE").is_none() {
        eprintln!(
            "skipping live genai run 2: set OMNI_GENAI_LIVE=1 (plus a provider key such as \
             OPENAI_API_KEY and OMNI_MODEL) to record and replay the resolve gate"
        );
        return Ok(());
    }

    let dir = std::env::temp_dir().join(format!("omnia-genai-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    // Run 2: the live genai backend, behind a `Recording` wrapper that writes the
    // run-1 fixture as a side effect of the live completion.
    let recording = Recording::new(Client::connect().await?, dir.clone());
    let prompt = resolve_prompt();
    let answer: BackendAnswer =
        recording.complete(prompt.clone(), Arc::new(LiveShelf)).await.map_err(|e| {
            anyhow::anyhow!("live genai completion failed (is OMNI_MODEL/the API key valid?): {e}")
        })?;

    // The model drove a `resolve` tool call, and the shelf bytes round-tripped
    // back into the loop.
    let transcript = answer.transcript.as_ref().expect("genai always records a transcript");
    let resolve_turn = transcript
        .turns
        .iter()
        .find(|turn| turn.tool == "resolve")
        .expect("the model must call the floor `resolve` tool");
    assert_eq!(
        resolve_turn.result,
        Value::String("shelf:alpha".to_owned()),
        "the resolve tool result must round-trip the shelf bytes"
    );

    // The answer is a JSON object (the floor's run-time gate for `json-object`)
    // and carries the resolved value the model fetched via the tool.
    assert!(answer.value.is_object(), "run-2 answer must be a JSON object: {:?}", answer.value);
    assert!(
        answer.value.to_string().contains("shelf:alpha"),
        "the resolved value must appear in the answer: {:?}",
        answer.value
    );

    // Run 1: the recorded fixture replays deterministically under `ModelDefault`
    // — no network, no tool host.
    let replay = ModelDefault::connect_with(ReplayConnectOptions {
        replay_dir: dir.clone(),
    })
    .await?;
    let replayed = replay.complete(prompt, Arc::new(LiveShelf)).await?;
    assert_eq!(replayed.value, answer.value, "ModelDefault must replay the exact recorded answer");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
