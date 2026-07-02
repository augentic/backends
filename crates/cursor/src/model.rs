//! # Cursor Agent Backend

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::FutureExt as _;
use omnia_wasi_model::{
    Answer, Format, FutureResult, PreparedPrompt, ResponseFormat, ToolHost, ToolTurn, Transcript,
    WasiModelCtx,
};
use serde_json::Value;
use tokio::process::Command;

use crate::{CURSOR_AGENT_BIN, Client, mcp};

const MCP_PROMPT_HINT: &str = "A read-only MCP server named `omnia` is available. Consult its \
    tools and resources for authoritative reference material before answering, and prefer that \
    material over assumptions.";
const MAX_ATTEMPTS: usize = 3;
const PROMPT_INLINE_LIMIT: usize = 128_000;

struct SpawnOptions<'a> {
    model: Option<&'a str>,
    workspace: &'a Path,
    timeout: Duration,
    mcp_url: Option<&'a str>,
}

#[derive(Debug)]
struct AgentOutput {
    result: String,
    transcript: Option<Transcript>,
}

static PROMPT_COUNTER: AtomicU64 = AtomicU64::new(0);

impl WasiModelCtx for Client {
    fn complete(
        &self, request: PreparedPrompt, tool_host: Arc<dyn ToolHost>,
    ) -> FutureResult<Answer> {
        let model = self.model.clone();
        let workspace = tool_host
            .local_path()
            .map(Path::to_path_buf)
            .or_else(|| self.workspace.as_deref().map(Path::to_path_buf));
        let timeout = self.timeout;
        let mcp_url = self.mcp_url.clone();

        async move {
            let kind = request.prompt.response_format.kind;
            let mut agent_prompt = build_prompt(&request);

            let Some(workspace) = workspace else {
                bail!("no local tree on this node");
            };
            std::fs::create_dir_all(&workspace)?;
            let workspace = workspace
                .canonicalize()
                .with_context(|| format!("workspace {}", workspace.display()))?;

            let _mcp_guard = match mcp_url.as_deref() {
                Some(url) => {
                    agent_prompt = format!("{MCP_PROMPT_HINT}\n\n{agent_prompt}");
                    Some(mcp::McpGuard::install(&workspace, url)?)
                }
                None => None,
            };

            let spawn = SpawnOptions {
                model: model.as_deref(),
                workspace: &workspace,
                timeout,
                mcp_url: mcp_url.as_deref(),
            };

            for attempt in 1..=MAX_ATTEMPTS {
                let stdout = spawn_agent(&agent_prompt, &spawn).await?;
                let AgentOutput { result, transcript } = parse_agent_output(&stdout)?;

                match parse_result(&result, kind) {
                    Ok(value) => match check_answer(&value, kind) {
                        Ok(()) => {
                            return Ok(Answer { value, transcript });
                        }
                        Err(_reason) if attempt == MAX_ATTEMPTS => {
                            return Ok(Answer { value, transcript });
                        }
                        Err(reason) => {
                            agent_prompt = append_repair(&agent_prompt, &result, &reason);
                        }
                    },
                    Err(reason) if attempt == MAX_ATTEMPTS => {
                        bail!(
                            "cursor-agent did not return an answer after {MAX_ATTEMPTS} attempts: {reason}"
                        );
                    }
                    Err(reason) => {
                        agent_prompt = append_repair(&agent_prompt, &result, &reason);
                    }
                }
            }

            bail!("cursor-agent did not return an answer after {MAX_ATTEMPTS} attempts");
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

fn append_repair(prompt: &str, answer: &str, reason: &str) -> String {
    format!(
        "{prompt}\n\nYour previous answer did not satisfy the required response format ({reason}). \
         Your previous answer was:\n{answer}\n\nReply again with only the corrected answer and \
         nothing else."
    )
}

fn check_answer(value: &Value, kind: Format) -> Result<(), String> {
    match kind {
        Format::Text if !value.is_string() => Err("answer is not a JSON string".to_owned()),
        Format::JsonObject if !value.is_object() => Err("answer is not a JSON object".to_owned()),
        _ => Ok(()),
    }
}

/// Write oversized prompts to the workspace and return a short CLI argument.
fn prepare_prompt_arg(
    agent_prompt: &str, workspace: &Path,
) -> Result<(String, Option<PromptFile>)> {
    if agent_prompt.len() <= PROMPT_INLINE_LIMIT {
        return Ok((agent_prompt.to_owned(), None));
    }

    let cursor_dir = workspace.join(".cursor");
    std::fs::create_dir_all(&cursor_dir)
        .with_context(|| format!("creating {}", cursor_dir.display()))?;
    let id = PROMPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = cursor_dir.join(format!("omnia-prompt-{id}.txt"));
    std::fs::write(&path, agent_prompt)
        .with_context(|| format!("writing prompt file {}", path.display()))?;
    let arg = format!(
        "Follow every instruction in the file at `{}`. When you are done, reply exactly as that \
         file instructs.",
        path.display()
    );
    Ok((arg, Some(PromptFile { path })))
}

/// Removes a spill-to-disk prompt file when the spawn finishes.
struct PromptFile {
    path: PathBuf,
}

impl Drop for PromptFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path) {
            tracing::warn!(path = %self.path.display(), %error, "failed to remove prompt file");
        }
    }
}

async fn spawn_agent(agent_prompt: &str, options: &SpawnOptions<'_>) -> Result<Vec<u8>> {
    let (prompt_arg, _prompt_file) = prepare_prompt_arg(agent_prompt, options.workspace)?;

    let mut command = Command::new(CURSOR_AGENT_BIN);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .arg("--print")
        .arg("--force")
        .arg("--trust")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--workspace")
        .arg(options.workspace);
    if options.mcp_url.is_some() {
        command.arg("--approve-mcps");
    }
    // if options.use_worktree {
    //     command.arg("--worktree");
    // }
    if let Some(model) = options.model {
        command.arg("--model").arg(model);
    }
    command.arg(prompt_arg);

    let child = command.spawn().with_context(|| format!("spawning `{CURSOR_AGENT_BIN}`"))?;

    let wait = child.wait_with_output();
    tokio::pin!(wait);

    let output = tokio::select! {
        () = tokio::time::sleep(options.timeout) => {
            bail!("cursor-agent timed out after {}s", options.timeout.as_secs());
        }
        result = wait => {
            result.with_context(|| format!("waiting on `{CURSOR_AGENT_BIN}`"))?
        }
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

/// Parse `stream-json` NDJSON (or a legacy single-line `json` payload).
fn parse_agent_output(stdout: &[u8]) -> Result<AgentOutput> {
    let text = std::str::from_utf8(stdout).context("cursor-agent did not emit valid UTF-8")?;

    let mut result: Option<String> = None;
    let mut pending_tools: HashMap<String, (String, Value)> = HashMap::new();
    let mut turns: Vec<ToolTurn> = Vec::new();
    let mut parsed_lines = 0_u32;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        parsed_lines += 1;
        let event: Value =
            serde_json::from_str(line).with_context(|| format!("invalid JSON event: {line}"))?;
        match event.get("type").and_then(Value::as_str) {
            Some("result") => {
                if event.get("is_error").and_then(Value::as_bool) == Some(true) {
                    bail!(
                        "cursor-agent reported an error: {}",
                        event.get("result").and_then(Value::as_str).unwrap_or("<no detail>")
                    );
                }
                result = event.get("result").and_then(Value::as_str).map(ToOwned::to_owned);
            }
            Some("tool_call") => match event.get("subtype").and_then(Value::as_str) {
                Some("started") => {
                    if let (Some(call_id), Some((tool, args))) = (
                        event.get("call_id").and_then(Value::as_str),
                        event.get("tool_call").and_then(tool_call_identity),
                    ) {
                        pending_tools.insert(call_id.to_owned(), (tool, args));
                    }
                }
                Some("completed") => {
                    if let (Some(call_id), Some(tool_call)) =
                        (event.get("call_id").and_then(Value::as_str), event.get("tool_call"))
                    {
                        let (tool, args) = pending_tools.remove(call_id).unwrap_or_else(|| {
                            tool_call_identity(tool_call)
                                .unwrap_or_else(|| ("unknown".to_owned(), Value::Null))
                        });
                        let tool_result = tool_call_result(tool_call).unwrap_or(Value::Null);
                        turns.push(ToolTurn {
                            tool,
                            args,
                            result: tool_result,
                        });
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    if let Some(result) = result {
        let transcript = if turns.is_empty() { None } else { Some(Transcript { turns }) };
        return Ok(AgentOutput { result, transcript });
    }

    if parsed_lines == 1 {
        return parse_legacy_json_output(text.trim());
    }

    bail!("cursor-agent did not emit a terminal result event");
}

fn parse_legacy_json_output(text: &str) -> Result<AgentOutput> {
    let envelope: Value =
        serde_json::from_str(text).context("cursor-agent did not emit a JSON result object")?;
    if envelope.get("is_error").and_then(Value::as_bool) == Some(true) {
        bail!(
            "cursor-agent reported an error: {}",
            envelope.get("result").and_then(Value::as_str).unwrap_or("<no detail>")
        );
    }
    let result = envelope
        .get("result")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .context("cursor-agent JSON output has no string `result` field")?;
    Ok(AgentOutput {
        result,
        transcript: None,
    })
}

fn tool_call_identity(tool_call: &Value) -> Option<(String, Value)> {
    let object = tool_call.as_object()?;
    for (key, value) in object {
        if key.ends_with("ToolCall") {
            let tool = key.strip_suffix("ToolCall")?.to_owned();
            let args = value.get("args").cloned().unwrap_or_else(|| value.clone());
            return Some((tool, args));
        }
        if key == "function" {
            let name = value.get("name").and_then(Value::as_str).unwrap_or("function").to_owned();
            let args = value.get("arguments").cloned().unwrap_or_else(|| value.clone());
            return Some((name, args));
        }
    }
    None
}

fn tool_call_result(tool_call: &Value) -> Option<Value> {
    let object = tool_call.as_object()?;
    for value in object.values() {
        if let Some(result) = value.get("result") {
            return Some(result.clone());
        }
    }
    None
}

fn parse_result(result: &str, kind: Format) -> Result<Value, String> {
    match kind {
        Format::Text => Ok(Value::String(result.to_owned())),
        Format::JsonObject | Format::JsonSchema => {
            let json = strip_code_fence(result);
            serde_json::from_str::<Value>(json)
                .map_err(|error| format!("cursor-agent answer was not valid JSON: {error}"))
        }
    }
}

/// Strip a wrapping Markdown code fence (```` ```json … ``` ````), if present.
fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let body = rest.split_once('\n').map_or(rest, |(_, body)| body).trim();
    body.strip_suffix("```").unwrap_or(body).trim()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use omnia_wasi_model::{
        Format, PreparedPrompt, Prompt, ResponseFormat, Sections, ToolGrants, WasiModelCtx,
    };
    use serde_json::json;

    use super::{
        AgentOutput, PROMPT_INLINE_LIMIT, answer_instruction, append_repair, build_prompt,
        check_answer, parse_agent_output, parse_result, prepare_prompt_arg,
    };
    use crate::test_support::{NoopToolHost, client};

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
        assert!(err.contains("not valid JSON"), "unexpected: {err}");
    }

    #[test]
    fn check_answer_kind() {
        check_answer(&json!("ok"), Format::Text).expect("string text answer");
        check_answer(&json!({ "ok": true }), Format::JsonObject).expect("object answer");
        assert!(check_answer(&json!(1), Format::Text).is_err());
        assert!(check_answer(&json!("nope"), Format::JsonObject).is_err());
    }

    #[test]
    fn append_repair_includes_reason() {
        let repaired = append_repair("base", "bad", "not JSON");
        assert!(repaired.contains("base"));
        assert!(repaired.contains("bad"));
        assert!(repaired.contains("not JSON"));
    }

    #[test]
    fn parse_legacy_result_field() {
        let stdout = br#"{"type":"result","is_error":false,"result":"{\"verdict\":\"pass\"}"}"#;
        let AgentOutput { result, transcript } =
            parse_agent_output(stdout).expect("extract result");
        assert_eq!(result, r#"{"verdict":"pass"}"#);
        assert!(transcript.is_none());
    }

    #[test]
    fn parse_result_error() {
        let stdout = br#"{"type":"result","is_error":true,"result":"boom"}"#;
        let err = parse_agent_output(stdout).expect_err("an agent error must surface");
        assert!(err.to_string().contains("cursor-agent reported an error"), "unexpected: {err}");
    }

    #[test]
    fn parse_stream_json_with_tool_calls() {
        let stdout = br#"{"type":"tool_call","subtype":"started","call_id":"c1","tool_call":{"readToolCall":{"args":{"path":"README.md"}}}}
{"type":"tool_call","subtype":"completed","call_id":"c1","tool_call":{"readToolCall":{"args":{"path":"README.md"},"result":{"success":{"content":"hi"}}}}}
{"type":"result","subtype":"success","is_error":false,"result":"{\"verdict\":\"pass\"}"}"#;
        let AgentOutput { result, transcript } = parse_agent_output(stdout).expect("parse stream");
        assert_eq!(result, r#"{"verdict":"pass"}"#);
        let transcript = transcript.expect("tool transcript");
        assert_eq!(transcript.turns.len(), 1);
        assert_eq!(transcript.turns[0].tool, "read");
        assert_eq!(transcript.turns[0].args, json!({ "path": "README.md" }));
    }

    #[test]
    fn spill_large_prompt_to_file() {
        let workspace =
            std::env::temp_dir().join(format!("omnia-cursor-prompt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(&workspace).expect("temp workspace");

        let large = "x".repeat(PROMPT_INLINE_LIMIT + 1);
        let (arg, spill) = prepare_prompt_arg(&large, &workspace).expect("spill prompt");
        assert!(spill.is_some(), "large prompt must spill to disk");
        assert!(arg.contains("omnia-prompt-"), "arg references prompt file: {arg}");
        drop(spill);
        let _ = std::fs::remove_dir_all(&workspace);
    }
}
