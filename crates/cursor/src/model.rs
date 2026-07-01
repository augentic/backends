//! `wasi-model` implementation backed by a spawned `cursor-agent` session.
//!
//! The spawned-agent backend (RFC wasi-model §5.3): fold the host-assembled
//! [`PreparedPrompt`] channels into a single agent prompt (the host applies
//! §3.1.1) with a trailing
//! response-format instruction, launch a fresh headless `cursor-agent` scoped to
//! the lent working tree, and parse its aggregated `.result` back into the
//! validated answer the boundary returns. The agent owns its own tool loop and
//! reads/writes the tree directly, so this backend uses the [`ToolHost`] only
//! for its `local-path` face ([`ToolHost::local_path`], RFC-55) — the agent's
//! `--workspace` — never its `read`/`list`/`write` (unlike genai). The runtime core's
//! `complete` binding re-validates the answer (§3.1.3), so this backend only
//! has to produce the parsed value.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::FutureExt as _;
use omnia_wasi_model::{
    Answer, Format, FutureResult, PreparedPrompt, ResponseFormat, ToolHost, WasiModelCtx,
};
use serde_json::Value;

use crate::{CURSOR_AGENT_BIN, Client};

impl WasiModelCtx for Client {
    fn complete(
        &self, request: PreparedPrompt, tool_host: Arc<dyn ToolHost>,
    ) -> FutureResult<Answer> {
        // Cursor owns its own loop and edits the tree directly
        let model = self.model.clone();
        let workspace = tool_host
            .local_path()
            .map(Path::to_path_buf)
            .or_else(|| self.workspace.as_deref().map(Path::to_path_buf));
        let timeout = self.timeout;

        async move {
            let kind = request.prompt.response_format.kind;
            let agent_prompt = build_prompt(&request);

            // an agent-driven build needs a node-local tree.
            let Some(workspace) = workspace else {
                bail!("no local tree on this node");
            };

            let stdout = spawn_agent(&agent_prompt, model.as_deref(), &workspace, timeout).await?;
            let result = extract_result(&stdout)?;
            let value = parse_result(&result, kind)?;

            // no in-process tool loop
            Ok(Answer {
                value,
                transcript: None,
            })
        }
        .boxed()
    }
}

fn build_prompt(request: &PreparedPrompt) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(system) = &request.system {
        parts.push(system.clone());
    }
    for message in &request.messages {
        if message.role == "user" {
            parts.push(message.content.clone());
        } else {
            parts.push(format!("[{}]\n{}", message.role, message.content));
        }
    }
    parts.push(answer_instruction(&request.prompt.response_format));
    parts.join("\n\n")
}

// Constrain the agent's answer to the requested `response-format`.
fn answer_instruction(response_format: &ResponseFormat) -> String {
    match response_format.kind {
        Format::JsonSchema => response_format.json_schema.as_ref().map_or_else(
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
        Format::JsonObject => "When you are done, reply with only your final answer as \
             a single JSON object and nothing else."
            .to_owned(),
        Format::Text => {
            "When you are done, reply with only your final answer as plain text and nothing else."
                .to_owned()
        }
    }
}

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

// Extract the result field from `cursor-agent`'s `--output-format json` payload.
fn extract_result(stdout: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(stdout).context("cursor-agent did not emit valid UTF-8")?;

    // extract result
    let envelope = if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        value
    } else {
        let Some(last) = text.lines().rev().find(|line| !line.trim().is_empty()) else {
            bail!("cursor-agent did not emit a JSON result object");
        };
        serde_json::from_str::<Value>(last.trim())?
    };

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

fn parse_result(result: &str, kind: Format) -> Result<Value> {
    match kind {
        Format::Text => Ok(Value::String(result.to_owned())),
        Format::JsonObject | Format::JsonSchema => {
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
        DirEntry, Format, FutureResult, PreparedPrompt, Prompt, Reference, ResponseFormat,
        Sections, ToolGrants, ToolHost, VerifyReport, WasiModelCtx,
    };
    use serde_json::json;

    use super::{answer_instruction, build_prompt, extract_result, parse_result};
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
                kind: Format::JsonSchema,
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
                workspace: None,
                verify: vec![],
            },
        }
    }

    #[tokio::test]
    async fn no_local_tree() {
        // With neither a lent working tree (the default `ToolHost::local_path`
        // is `None`) nor an `OMNIA_WORKSPACE` override, `complete` must fail
        // loud before any spawn — the §5.3 capability signal.
        let err = client(None)
            .complete(
                PreparedPrompt::try_from(schema_prompt()).expect("try_from"),
                Arc::new(NoopToolHost),
            )
            .await
            .expect_err("a backend with no local tree must fail");
        assert!(err.to_string().contains("no local tree on this node"), "unexpected error: {err}");
    }

    #[test]
    fn agent_prompt() {
        let request = PreparedPrompt::try_from(schema_prompt()).expect("try_from");
        let text = build_prompt(&request);
        assert!(text.contains("a terse judge"), "missing system channel: {text}");
        assert!(text.contains("decide pass or fail"), "missing user channel: {text}");
        assert!(text.contains("JSON Schema"), "missing schema instruction: {text}");
        assert!(text.contains("\"verdict\""), "missing schema body: {text}");
    }

    #[test]
    fn answer_kind() {
        let text = answer_instruction(&ResponseFormat {
            kind: Format::Text,
            json_schema: None,
        });
        assert!(text.contains("plain text"), "text instruction: {text}");

        let object = answer_instruction(&ResponseFormat {
            kind: Format::JsonObject,
            json_schema: None,
        });
        assert!(object.contains("JSON object"), "object instruction: {object}");
    }

    #[test]
    fn parse_json() {
        assert_eq!(parse_result("hello", Format::Text).unwrap(), json!("hello"));
        assert_eq!(
            parse_result(r#"{"verdict":"pass"}"#, Format::JsonObject).unwrap(),
            json!({ "verdict": "pass" })
        );
    }

    #[test]
    fn parse_code_fence() {
        let fenced = "```json\n{\"verdict\":\"pass\"}\n```";
        assert_eq!(parse_result(fenced, Format::JsonSchema).unwrap(), json!({ "verdict": "pass" }));
    }

    #[test]
    fn parse_non_json() {
        let err = parse_result("not json", Format::JsonObject).unwrap_err();
        assert!(err.to_string().contains("not valid JSON"), "unexpected: {err}");
    }

    #[test]
    fn extract_result_field() {
        let stdout = br#"{"type":"result","is_error":false,"result":"{\"verdict\":\"pass\"}"}"#;
        let result = extract_result(stdout).expect("extract result");
        assert_eq!(result, r#"{"verdict":"pass"}"#);
    }

    #[test]
    fn extract_result_error() {
        let stdout = br#"{"type":"result","is_error":true,"result":"boom"}"#;
        let err = extract_result(stdout).expect_err("an agent error must surface");
        assert!(err.to_string().contains("cursor-agent reported an error"), "unexpected: {err}");
    }

    #[test]
    fn extract_result_newline() {
        let stdout = b"{\"is_error\":false,\"result\":\"hi\"}\n";
        assert_eq!(extract_result(stdout).expect("extract"), "hi");
    }
}
