//! `wasi-model` implementation backed by a spawned `cursor-agent` session.
//!
//! The spawned-agent backend (RFC wasi-model §5.3): assemble the floor [`Prompt`]
//! into a single agent prompt (§3.1.1, reusing [`assemble`]) with a trailing
//! response-format instruction, launch a fresh headless `cursor-agent` scoped to
//! the lent working tree, and parse its aggregated `.result` back into the
//! validated answer the boundary returns. The agent owns its own tool loop and
//! reads/writes the tree directly, so this backend ignores the [`ToolHost`]
//! (unlike genai). The floor's `complete` binding re-validates the answer
//! (§3.1.3), so this backend only has to produce the parsed value.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::FutureExt as _;
use omnia_wasi_model::{
    Assembled, BackendAnswer, FutureResult, Prompt, ResponseFormat, ResponseFormatKind, ToolHost,
    WasiModelCtx, assemble,
};
use serde_json::Value;

use crate::{CURSOR_AGENT_BIN, Client};

impl WasiModelCtx for Client {
    fn complete(
        &self, prompt: Prompt, _tool_host: Arc<dyn ToolHost>,
    ) -> FutureResult<BackendAnswer> {
        // cursor owns its own loop and edits the tree directly, so the per-call
        // `ToolHost` is unused here (§5.3). Clone the cheap handles into the
        // 'static future.
        let model = self.model.clone();
        let workspace = self.workspace.clone();
        let timeout = self.timeout;

        async move {
            let kind = prompt.response_format.kind;

            // Assemble system + user channels (§3.1.1) and fold them into one
            // prompt string with a response-format instruction so `.result`
            // parses.
            let assembled =
                assemble(&prompt).map_err(|e| anyhow::anyhow!("prompt assembly failed: {e:?}"))?;
            let agent_prompt = build_agent_prompt(&assembled, &prompt.response_format);

            // Capability signal: an agent-driven build needs a node-local tree.
            // Stopgap for the RFC-55 `local-path` face — sourced from config
            // rather than the lent `grants.working-tree` descriptor for now.
            let Some(workspace) = workspace else {
                bail!("no local tree on this node");
            };

            let stdout = spawn_agent(&agent_prompt, model.as_deref(), &workspace, timeout).await?;
            let result = extract_result(&stdout)?;
            let value = parse_result(&result, kind)?;

            // No in-process tool loop, so there is no transcript to record; the
            // fixture still keys on the typed prompt (§5.4).
            Ok(BackendAnswer {
                value,
                transcript: None,
            })
        }
        .boxed()
    }
}

/// Fold the assembled channels (§3.1.1) into the single prompt string handed to
/// `cursor-agent`, with a trailing instruction pinning the agent's final answer
/// to the floor's `response-format` so `.result` parses (§5.3).
fn build_agent_prompt(assembled: &Assembled, response_format: &ResponseFormat) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(system) = &assembled.system {
        parts.push(system.clone());
    }
    for message in &assembled.messages {
        // Roles are advisory in a single-shot agent prompt; keep user content
        // verbatim and tag the rest so the structure survives.
        if message.role == "user" {
            parts.push(message.content.clone());
        } else {
            parts.push(format!("[{}]\n{}", message.role, message.content));
        }
    }
    parts.push(answer_instruction(response_format));
    parts.join("\n\n")
}

/// The trailing instruction that constrains the agent's final answer to the
/// requested `response-format` (§3.1.3).
fn answer_instruction(response_format: &ResponseFormat) -> String {
    match response_format.kind {
        ResponseFormatKind::JsonSchema => response_format.json_schema.as_ref().map_or_else(
            || {
                "When you are done, reply with only your final answer as a single JSON value and \
                 nothing else."
                    .to_owned()
            },
            |spec| {
                format!(
                    "When you are done, reply with only your final answer as a single JSON value \
                     conforming to this JSON Schema, and nothing else:\n{}",
                    spec.schema
                )
            },
        ),
        ResponseFormatKind::JsonObject => {
            "When you are done, reply with only your final answer as \
             a single JSON object and nothing else."
                .to_owned()
        }
        ResponseFormatKind::Text => {
            "When you are done, reply with only your final answer as plain text and nothing else."
                .to_owned()
        }
    }
}

/// Spawn one headless `cursor-agent` run scoped to `workspace`, bounded by
/// `timeout`, returning its stdout.
///
/// Uses the documented headless surface: `--print` runs non-interactive to
/// completion, `--force` grants write access so the agent can edit the tree,
/// `--trust` skips the workspace-trust prompt, `--output-format json` emits a
/// single result object, and `--workspace` scopes it to the lent tree.
async fn spawn_agent(
    agent_prompt: &str, model: Option<&str>, workspace: &Path, timeout: Duration,
) -> Result<Vec<u8>> {
    let mut command = tokio::process::Command::new(CURSOR_AGENT_BIN);
    command
        .arg("--print")
        .arg("--force")
        .arg("--trust")
        .arg("--output-format")
        .arg("json")
        .arg("--workspace")
        .arg(workspace);
    if let Some(model) = model {
        command.arg("--model").arg(model);
    }
    command.arg(agent_prompt);

    // `cursor-agent --print` is known to occasionally hang after finishing, so
    // the spawn is wrapped in a wall-clock bound (§5.3). The enclosing per-call
    // `guest_timeout` is the outer bound.
    let output = match tokio::time::timeout(timeout, command.output()).await {
        Err(_elapsed) => bail!("cursor-agent timed out after {}s", timeout.as_secs()),
        Ok(result) => result.with_context(|| format!("spawning `{CURSOR_AGENT_BIN}`"))?,
    };

    if !output.status.success() {
        bail!(
            "cursor-agent exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(output.stdout)
}

/// Pull the aggregated final answer out of `cursor-agent`'s `--output-format
/// json` payload: a single JSON object whose `result` field holds the answer.
fn extract_result(stdout: &[u8]) -> Result<String> {
    let envelope = parse_json_envelope(stdout)
        .context("cursor-agent did not emit a JSON result object (did the run fail?)")?;

    if envelope.get("is_error").and_then(Value::as_bool) == Some(true) {
        bail!(
            "cursor-agent reported an error: {}",
            envelope.get("result").and_then(Value::as_str).unwrap_or("<no detail>")
        );
    }

    envelope
        .get("result")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .context("cursor-agent JSON output has no string `result` field")
}

/// Parse the agent envelope, tolerating a trailing newline or stray preamble by
/// falling back to the last non-empty line (the `json` format emits one object).
fn parse_json_envelope(stdout: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(stdout).ok()?;
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        return Some(value);
    }
    let last = text.lines().rev().find(|line| !line.trim().is_empty())?;
    serde_json::from_str::<Value>(last.trim()).ok()
}

/// Interpret the agent's `result` text as the answer value for `kind`, mirroring
/// the genai backend: `text` wraps the string; JSON kinds parse (tolerating a
/// Markdown code fence the agent may add despite instructions). The floor
/// re-validates the shape (§3.1.3).
fn parse_result(result: &str, kind: ResponseFormatKind) -> Result<Value> {
    match kind {
        ResponseFormatKind::Text => Ok(Value::String(result.to_owned())),
        ResponseFormatKind::JsonObject | ResponseFormatKind::JsonSchema => {
            let json = strip_code_fence(result);
            serde_json::from_str::<Value>(json)
                .with_context(|| format!("cursor-agent answer was not valid JSON: {json}"))
        }
    }
}

/// Strip a wrapping Markdown code fence (```` ```json … ``` ````), if present.
fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    // Drop the remainder of the opening fence line (an optional language tag).
    let body = rest.split_once('\n').map_or(rest, |(_, body)| body).trim();
    body.strip_suffix("```").unwrap_or(body).trim()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use futures::FutureExt as _;
    use omnia_wasi_model::{
        DirEntry, FutureResult, Prompt, Reference, ResponseFormat, ResponseFormatKind, Sections,
        ToolGrants, ToolHost, VerifyReport, WasiModelCtx, assemble,
    };
    use serde_json::json;

    use super::{answer_instruction, build_agent_prompt, extract_result, parse_result};
    use crate::Client;

    /// A no-op `ToolHost`: cursor ignores it, so every method just errors.
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

    /// Build a `Client` directly, bypassing `connect_with` (and its `PATH` check)
    /// so these tests run in CI without `cursor-agent` installed.
    fn client(workspace: Option<&str>) -> Client {
        Client {
            model: None,
            workspace: workspace.map(|w| Arc::from(Path::new(w))),
            // Nominal: no unit test reaches a spawn, so the bound is unused.
            timeout: Duration::from_secs(1),
        }
    }

    fn schema_prompt() -> Prompt {
        Prompt {
            model: None,
            system: None,
            messages: vec![],
            sections: Some(Sections {
                role: Some("a terse judge".to_owned()),
                task: "decide pass or fail".to_owned(),
                context: None,
                constraints: vec![],
                examples: vec![],
                variables: vec![],
            }),
            generation: None,
            response_format: ResponseFormat {
                kind: ResponseFormatKind::JsonSchema,
                json_schema: Some(omnia_wasi_model::JsonSchemaSpec {
                    name: "verdict".to_owned(),
                    schema: json!({
                        "type": "object",
                        "properties": { "verdict": { "type": "string" } },
                        "required": ["verdict"],
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
                working_tree_lent: true,
                verify: vec![],
            },
        }
    }

    #[tokio::test]
    async fn no_local_tree_is_a_capability_signal() {
        // Without a workspace, `complete` must fail loud before any spawn — the
        // §5.3 capability signal (here keyed on the config stopgap).
        let err = client(None)
            .complete(schema_prompt(), Arc::new(NoopToolHost))
            .await
            .expect_err("a backend with no local tree must fail");
        assert!(err.to_string().contains("no local tree on this node"), "unexpected error: {err}");
    }

    #[test]
    fn build_agent_prompt_includes_channels_and_schema() {
        let prompt = schema_prompt();
        let assembled = assemble(&prompt).expect("assemble");
        let text = build_agent_prompt(&assembled, &prompt.response_format);
        // The assembled system + user channels survive into the agent prompt.
        assert!(text.contains("a terse judge"), "missing system channel: {text}");
        assert!(text.contains("decide pass or fail"), "missing user channel: {text}");
        // The trailing instruction carries the JSON Schema (§5.3).
        assert!(text.contains("JSON Schema"), "missing schema instruction: {text}");
        assert!(text.contains("\"verdict\""), "missing schema body: {text}");
    }

    #[test]
    fn answer_instruction_tracks_the_kind() {
        let text = answer_instruction(&ResponseFormat {
            kind: ResponseFormatKind::Text,
            json_schema: None,
        });
        assert!(text.contains("plain text"), "text instruction: {text}");

        let object = answer_instruction(&ResponseFormat {
            kind: ResponseFormatKind::JsonObject,
            json_schema: None,
        });
        assert!(object.contains("JSON object"), "object instruction: {object}");
    }

    #[test]
    fn parse_result_wraps_text_and_parses_json() {
        assert_eq!(parse_result("hello", ResponseFormatKind::Text).unwrap(), json!("hello"));
        assert_eq!(
            parse_result(r#"{"verdict":"pass"}"#, ResponseFormatKind::JsonObject).unwrap(),
            json!({ "verdict": "pass" })
        );
    }

    #[test]
    fn parse_result_strips_a_code_fence() {
        let fenced = "```json\n{\"verdict\":\"pass\"}\n```";
        assert_eq!(
            parse_result(fenced, ResponseFormatKind::JsonSchema).unwrap(),
            json!({ "verdict": "pass" })
        );
    }

    #[test]
    fn parse_result_rejects_non_json() {
        let err = parse_result("not json", ResponseFormatKind::JsonObject).unwrap_err();
        assert!(err.to_string().contains("not valid JSON"), "unexpected: {err}");
    }

    #[test]
    fn extract_result_reads_the_result_field() {
        let stdout = br#"{"type":"result","is_error":false,"result":"{\"verdict\":\"pass\"}"}"#;
        let result = extract_result(stdout).expect("extract result");
        assert_eq!(result, r#"{"verdict":"pass"}"#);
    }

    #[test]
    fn extract_result_surfaces_an_agent_error() {
        let stdout = br#"{"type":"result","is_error":true,"result":"boom"}"#;
        let err = extract_result(stdout).expect_err("an agent error must surface");
        assert!(err.to_string().contains("cursor-agent reported an error"), "unexpected: {err}");
    }

    #[test]
    fn extract_result_tolerates_a_trailing_newline() {
        let stdout = b"{\"is_error\":false,\"result\":\"hi\"}\n";
        assert_eq!(extract_result(stdout).expect("extract"), "hi");
    }
}
