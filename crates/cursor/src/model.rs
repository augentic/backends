use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures::FutureExt as _;
use omnia_wasi_model::{
    Answer, Format, FutureResult, PreparedRequest, Request, Role, Tool, ToolHost, ToolTurn,
    Transcript, WasiModelCtx,
};
use serde_json::Value;
use tokio::process::Command;

use crate::{CURSOR_AGENT_BIN, Client, mcp};

const MAX_ATTEMPTS: usize = 3;
const MAX_INLINE_SIZE: usize = 128_000;

// A prompt-granted MCP server and the endpoint URL the guest supplied for it.
struct McpServer {
    name: String,
    url: String,
    tools: Vec<String>,
}

struct SpawnOptions<'a> {
    model: Option<&'a str>,
    workspace: &'a Path,
    timeout: Duration,
    approve_mcps: bool,
}

#[derive(Debug)]
struct AgentOutput {
    result: String,
    transcript: Option<Transcript>,
}

static PROMPT_COUNTER: AtomicU64 = AtomicU64::new(0);

impl WasiModelCtx for Client {
    fn complete(
        &self, request: PreparedRequest, tool_host: Arc<dyn ToolHost>,
    ) -> FutureResult<Answer> {
        let workspace = tool_host.local_path().map(Path::to_path_buf);
        let timeout = self.timeout;

        async move {
            let format = request.request.format.clone();
            let mut prompt = build_prompt(&request);

            let Some(workspace) = workspace else {
                bail!("no local tree on this node");
            };
            std::fs::create_dir_all(&workspace)?;
            let workspace = workspace
                .canonicalize()
                .with_context(|| format!("workspace {}", workspace.display()))?;

            // Per-prompt MCP grants carry their own endpoint URL; no grant means
            // no MCP wiring (MCP is opt-in per completion).
            let selected = select_mcp_servers(&request.request)?;
            let _mcp_guard = if selected.is_empty() {
                None
            } else {
                prompt = format!("{}\n\n{prompt}", mcp_hint(&selected));
                let map: BTreeMap<String, String> =
                    selected.iter().map(|s| (s.name.clone(), s.url.clone())).collect();
                Some(mcp::McpGuard::install(&workspace, &map)?)
            };

            let spawn = SpawnOptions {
                model: request.request.model.as_deref(),
                workspace: &workspace,
                timeout,
                approve_mcps: !selected.is_empty(),
            };

            for attempt in 1..=MAX_ATTEMPTS {
                let stdout = spawn_agent(&prompt, &spawn).await?;
                let AgentOutput { result, transcript } = parse_output(&stdout)?;
                let last = attempt == MAX_ATTEMPTS;

                match parse_result(&result, &format) {
                    Ok(value) => match check_answer(&value, &format) {
                        // a value of the wrong shape is better than no answer
                        // on the last attempt
                        Ok(()) => return Ok(Answer { value, usage: None, transcript }),
                        Err(_) if last => return Ok(Answer { value, usage: None, transcript }),
                        Err(reason) => {
                            prompt = append_repair(&prompt, &result, &reason);
                        }
                    },
                    Err(reason) if last => {
                        bail!(
                            "cursor-agent did not return an answer after {MAX_ATTEMPTS} attempts: {reason}"
                        );
                    }
                    Err(reason) => {
                        prompt = append_repair(&prompt, &result, &reason);
                    }
                }
            }

            unreachable!("the final attempt always returns or bails");
        }
        .boxed()
    }
}

fn build_prompt(request: &PreparedRequest) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(system) = &request.system {
        parts.push(system.clone());
    }
    for message in &request.messages {
        match message.role {
            Role::User => parts.push(message.content.clone()),
            Role::System => parts.push(format!("[system]\n{}", message.content)),
            Role::Assistant => parts.push(format!("[assistant]\n{}", message.content)),
        }
    }
    parts.push(answer_instruction(&request.request.format));
    parts.join("\n\n")
}

// Collect the prompt's MCP grants, each carrying its own endpoint URL. A grant
// without a URL is an error: the backend has nowhere to point the spawned agent.
fn select_mcp_servers(request: &Request) -> Result<Vec<McpServer>> {
    let mut selected = Vec::new();
    for tool in &request.tools {
        let Tool::Mcp(grant) = tool else { continue };
        let url = grant.url.clone().ok_or_else(|| {
            anyhow!("MCP grant `{}` has no url; the guest must supply one", grant.name)
        })?;
        selected.push(McpServer {
            name: grant.name.clone(),
            url,
            tools: grant.tools.clone(),
        });
    }
    Ok(selected)
}

// A natural-language hint naming the granted MCP servers and any tool allowlist,
// prepended so the spawned agent prefers them over assumptions.
fn mcp_hint(servers: &[McpServer]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for server in servers {
        if server.tools.is_empty() {
            lines.push(format!("- `{}`", server.name));
        } else {
            lines.push(format!("- `{}` (use only: {})", server.name, server.tools.join(", ")));
        }
    }
    format!(
        "The following read-only MCP servers are available. Consult their tools and resources for \
         authoritative reference material before answering, and prefer that material over \
         assumptions:\n{}",
        lines.join("\n")
    )
}

async fn spawn_agent(prompt: &str, options: &SpawnOptions<'_>) -> Result<Vec<u8>> {
    // if the prompt is too large, spill it to a file and pass the file path to the agent
    let (prompt, _file) = if prompt.len() <= MAX_INLINE_SIZE {
        (prompt.to_owned(), None)
    } else {
        into_prompt_file(prompt, options.workspace)?
    };

    let mut cmd = Command::new(CURSOR_AGENT_BIN);
    cmd.kill_on_drop(true)
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
    if options.approve_mcps {
        cmd.arg("--approve-mcps");
    }
    if let Some(model) = options.model {
        cmd.arg("--model").arg(model);
    }
    cmd.arg(prompt);

    let child = cmd.spawn().with_context(|| format!("spawning `{CURSOR_AGENT_BIN}`"))?;

    let output = tokio::time::timeout(options.timeout, child.wait_with_output())
        .await
        .map_err(|_elapsed| anyhow!("cursor-agent timed out after {}s", options.timeout.as_secs()))?
        .with_context(|| format!("waiting on `{CURSOR_AGENT_BIN}`"))?;

    if !output.status.success() {
        bail!(
            "cursor-agent exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(output.stdout)
}

// Removes a spill-to-disk prompt file when the spawn finishes.
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

// Write oversized prompts to the workspace and return a short CLI argument.
fn into_prompt_file(prompt: &str, workspace: &Path) -> Result<(String, Option<PromptFile>)> {
    let cursor_dir = workspace.join(".cursor");
    std::fs::create_dir_all(&cursor_dir)
        .with_context(|| format!("creating {}", cursor_dir.display()))?;

    let id = PROMPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = cursor_dir.join(format!("omnia-prompt-{id}.txt"));
    std::fs::write(&path, prompt)
        .with_context(|| format!("writing prompt file {}", path.display()))?;

    let arg = format!(
        "Follow every instruction in the file at `{}`. When you are done, reply exactly as that \
         file instructs.",
        path.display()
    );

    Ok((arg, Some(PromptFile { path })))
}

fn answer_instruction(format: &Format) -> String {
    match format {
        Format::Schema(spec) => format!(
            "When you are done, reply with only your final answer as a single JSON value \
             conforming to this JSON Schema, and nothing else:\n{}",
            spec.schema
        ),
        Format::Json => "When you are done, reply with only your final answer as \
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

fn check_answer(value: &Value, format: &Format) -> Result<(), String> {
    match format {
        Format::Text if !value.is_string() => Err("answer is not a JSON string".to_owned()),
        Format::Json if !value.is_object() => Err("answer is not a JSON object".to_owned()),
        _ => Ok(()),
    }
}

// Parse `stream-json` NDJSON (or a legacy single-line `json` payload).
fn parse_output(stdout: &[u8]) -> Result<AgentOutput> {
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
                result = result_payload(&event)?;
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
        return parse_legacy_output(text.trim());
    }

    bail!("cursor-agent did not emit a terminal result event");
}

fn parse_legacy_output(text: &str) -> Result<AgentOutput> {
    let envelope: Value =
        serde_json::from_str(text).context("cursor-agent did not emit a JSON result object")?;
    let result = result_payload(&envelope)?
        .context("cursor-agent JSON output has no string `result` field")?;
    Ok(AgentOutput {
        result,
        transcript: None,
    })
}

// Extract the `result` string from a result envelope, surfacing agent errors.
fn result_payload(envelope: &Value) -> Result<Option<String>> {
    let result = envelope.get("result").and_then(Value::as_str);
    if envelope.get("is_error").and_then(Value::as_bool) == Some(true) {
        bail!("cursor-agent reported an error: {}", result.unwrap_or("<no detail>"));
    }
    Ok(result.map(ToOwned::to_owned))
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

fn parse_result(result: &str, format: &Format) -> Result<Value, String> {
    match format {
        Format::Text => Ok(Value::String(result.to_owned())),
        Format::Json | Format::Schema(_) => {
            let json = strip_code_fence(result);
            serde_json::from_str::<Value>(json)
                .map_err(|error| format!("cursor-agent answer was not valid JSON: {error}"))
        }
    }
}

// Strip a wrapping Markdown code fence (```json ... ```), if present.
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
        Format, Grants, PreparedRequest, Request, Schema, Sections, WasiModelCtx,
    };
    use serde_json::json;

    use super::{
        AgentOutput, MAX_INLINE_SIZE, build_prompt, into_prompt_file, parse_output, parse_result,
    };
    use crate::test_support::{NoopToolHost, client};

    fn schema_request() -> Request {
        Request {
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
            format: Format::Schema(Schema {
                name: "verdict".to_owned(),
                schema: json!({
                    "type": "object",
                    "properties": { "verdict": { "type": "string" } },
                    "required": ["verdict"],
                })
                .to_string(),
            }),
            tools: vec![],
            grants: Grants {
                references: None,
                workspace: None,
                verify: vec![],
            },
        }
    }

    #[tokio::test]
    async fn no_local_tree() {
        let err = client()
            .complete(
                PreparedRequest::try_from(schema_request()).expect("try_from"),
                Arc::new(NoopToolHost),
            )
            .await
            .expect_err("a backend with no local tree must fail");
        assert!(err.to_string().contains("no local tree on this node"), "unexpected error: {err}");
    }

    #[test]
    fn agent_prompt() {
        let request = PreparedRequest::try_from(schema_request()).expect("try_from");
        let text = build_prompt(&request);
        assert!(text.contains("a terse judge"), "missing system channel: {text}");
        assert!(text.contains("decide pass or fail"), "missing user channel: {text}");
        assert!(text.contains("JSON Schema"), "missing schema instruction: {text}");
        assert!(text.contains("\"verdict\""), "missing schema body: {text}");
    }

    fn verdict_schema() -> Format {
        Format::Schema(Schema {
            name: "verdict".to_owned(),
            schema: "{\"type\":\"object\"}".to_owned(),
        })
    }

    #[test]
    fn parse_json() {
        assert_eq!(parse_result("hello", &Format::Text).unwrap(), json!("hello"));
        assert_eq!(
            parse_result(r#"{"verdict":"pass"}"#, &Format::Json).unwrap(),
            json!({ "verdict": "pass" })
        );
    }

    #[test]
    fn parse_code_fence() {
        let fenced = "```json\n{\"verdict\":\"pass\"}\n```";
        assert_eq!(parse_result(fenced, &verdict_schema()).unwrap(), json!({ "verdict": "pass" }));
    }

    #[test]
    fn parse_non_json() {
        let err = parse_result("not json", &Format::Json).unwrap_err();
        assert!(err.contains("not valid JSON"), "unexpected: {err}");
    }

    #[test]
    fn parse_legacy_result_field() {
        let stdout = br#"{"type":"result","is_error":false,"result":"{\"verdict\":\"pass\"}"}"#;
        let AgentOutput { result, transcript } = parse_output(stdout).expect("extract result");
        assert_eq!(result, r#"{"verdict":"pass"}"#);
        assert!(transcript.is_none());
    }

    #[test]
    fn parse_result_error() {
        let stdout = br#"{"type":"result","is_error":true,"result":"boom"}"#;
        let err = parse_output(stdout).expect_err("an agent error must surface");
        assert!(err.to_string().contains("cursor-agent reported an error"), "unexpected: {err}");
    }

    #[test]
    fn parse_stream_json() {
        let stdout = br#"{"type":"tool_call","subtype":"started","call_id":"c1","tool_call":{"readToolCall":{"args":{"path":"README.md"}}}}
{"type":"tool_call","subtype":"completed","call_id":"c1","tool_call":{"readToolCall":{"args":{"path":"README.md"},"result":{"success":{"content":"hi"}}}}}
{"type":"result","subtype":"success","is_error":false,"result":"{\"verdict\":\"pass\"}"}"#;
        let AgentOutput { result, transcript } = parse_output(stdout).expect("parse stream");
        assert_eq!(result, r#"{"verdict":"pass"}"#);
        let transcript = transcript.expect("tool transcript");
        assert_eq!(transcript.turns.len(), 1);
        assert_eq!(transcript.turns[0].tool, "read");
        assert_eq!(transcript.turns[0].args, json!({ "path": "README.md" }));
    }

    #[test]
    fn spill_large_prompt() {
        let workspace =
            std::env::temp_dir().join(format!("omnia-cursor-prompt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(&workspace).expect("temp workspace");

        let large = "x".repeat(MAX_INLINE_SIZE + 1);
        let (arg, spill) = into_prompt_file(&large, &workspace).expect("spill prompt");
        assert!(spill.is_some(), "large prompt must spill to disk");
        assert!(arg.contains("omnia-prompt-"), "arg references prompt file: {arg}");
        drop(spill);
        let _ = std::fs::remove_dir_all(&workspace);
    }
}
