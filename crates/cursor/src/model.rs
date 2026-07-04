use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use omnia_wasi_model::{
    Answer, FutureResult, Request, Role, Tool, ToolHost, ToolTurn, Transcript, WasiModelCtx,
    check_answer, parse_answer, repair_instruction,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, BufReader};
use tokio::process::Command;
use tracing::instrument;

use crate::{CURSOR_AGENT_BIN, Client, mcp};

const MAX_ATTEMPTS: usize = 2;
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
    fn complete(&self, request: Request, tool_host: Arc<dyn ToolHost>) -> FutureResult<Answer> {
        let workspace = tool_host.local_path().map(Path::to_path_buf);
        let timeout = self.timeout;

        Box::pin(async move {
            let format = &request.format;
            let mut prompt = build_prompt(&request);

            let Some(workspace) = workspace else {
                bail!("no local tree on this node");
            };
            std::fs::create_dir_all(&workspace)
                .with_context(|| format!("creating {}", workspace.display()))?;
            let workspace = workspace
                .canonicalize()
                .with_context(|| format!("canonicalizing {}", workspace.display()))?;

            // Per-prompt MCP grants carry their own endpoint URL.
            // No grant means no MCP wiring (MCP is opt-in per completion).
            let selected = select_mcp_servers(&request);
            let _mcp_guard = if selected.is_empty() {
                None
            } else {
                prompt = format!("{}\n\n{prompt}", mcp_hint(&selected));
                let map: BTreeMap<String, String> =
                    selected.iter().map(|s| (s.name.clone(), s.url.clone())).collect();
                Some(mcp::McpGuard::install(&workspace, &map)?)
            };

            let spawn = SpawnOptions {
                model: request.model.as_deref(),
                workspace: &workspace,
                timeout,
                approve_mcps: !selected.is_empty(),
            };

            tracing::info!(model = spawn.model, ?format, "cursor completion");

            let mut attempt = 0;
            loop {
                attempt += 1;
                let last = attempt == MAX_ATTEMPTS;
                let AgentOutput { result, transcript } = spawn_agent(&prompt, &spawn).await?;
                tracing::debug!(attempt, result = result.len(), "cursor-agent answer");

                match parse_answer(&result, format) {
                    Ok(value) => match check_answer(&value, format) {
                        // the wrong shape is better than no answer on the last attempt
                        Ok(()) => {
                            return Ok(Answer {
                                value,
                                usage: None,
                                transcript,
                            });
                        }
                        Err(_) if last => {
                            return Ok(Answer {
                                value,
                                usage: None,
                                transcript,
                            });
                        }
                        Err(reason) => {
                            tracing::debug!(attempt, %reason, "repairing cursor-agent answer");
                            prompt = append_repair(&prompt, &result, &reason);
                        }
                    },
                    Err(reason) if last => {
                        bail!(
                            "cursor-agent did not return an answer after {MAX_ATTEMPTS} attempts: {reason}"
                        );
                    }
                    Err(reason) => {
                        tracing::debug!(attempt, %reason, "repairing cursor-agent answer");
                        prompt = append_repair(&prompt, &result, &reason);
                    }
                }
            }
        })
    }
}

fn build_prompt(request: &Request) -> String {
    let mut parts: Vec<Cow<'_, str>> = Vec::new();
    if let Some(system) = &request.system {
        parts.push(Cow::Borrowed(system.as_str()));
    }
    for message in &request.messages {
        parts.push(match message.role {
            Role::User => Cow::Borrowed(message.content.as_str()),
            Role::System => Cow::Owned(format!("[system]\n{}", message.content)),
            Role::Assistant => Cow::Owned(format!("[assistant]\n{}", message.content)),
        });
    }
    parts.push(Cow::Owned(request.format.instruction()));
    parts.join("\n\n")
}

// Collect the prompt's MCP grants, each carrying its own endpoint URL.
fn select_mcp_servers(request: &Request) -> Vec<McpServer> {
    request
        .tools
        .iter()
        .filter_map(|tool| match tool {
            Tool::Mcp(grant) => Some(McpServer {
                name: grant.name.clone(),
                url: grant.url.clone(),
                tools: grant.tools.clone(),
            }),
            Tool::Function(_) => None,
        })
        .collect()
}

// A natural-language hint naming the granted MCP servers and any tool allowlist,
// prepended so the spawned agent prefers them over assumptions.
fn mcp_hint(servers: &[McpServer]) -> String {
    let lines: Vec<String> = servers
        .iter()
        .map(|server| {
            if server.tools.is_empty() {
                format!("- `{}`", server.name)
            } else {
                format!("- `{}` (use only: {})", server.name, server.tools.join(", "))
            }
        })
        .collect();
    format!(
        "The following read-only MCP servers are available. Consult their tools and resources for \
         authoritative reference material before answering, and prefer that material over \
         assumptions:\n{}",
        lines.join("\n")
    )
}

#[instrument(
    skip(prompt, options),
    fields(
        model = options.model,
        workspace = %options.workspace.display(),
        approve_mcps = options.approve_mcps,
    )
)]
async fn spawn_agent(prompt: &str, options: &SpawnOptions<'_>) -> Result<AgentOutput> {
    // if the prompt is too large, spill it to a file and pass the file path to the agent
    let (prompt, _file): (Cow<'_, str>, Option<PromptFile>) = if prompt.len() <= MAX_INLINE_SIZE {
        (Cow::Borrowed(prompt), None)
    } else {
        let (arg, file) = into_prompt_file(prompt, options.workspace)?;
        (Cow::Owned(arg), Some(file))
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
    cmd.arg(prompt.as_ref());

    let mut child = cmd.spawn().with_context(|| format!("spawning `{CURSOR_AGENT_BIN}`"))?;
    let stdout = child.stdout.take().context("child stdout is piped")?;
    let stderr = child.stderr.take().context("child stderr is piped")?;

    // Parse stdout as it streams so memory stays bounded on chatty runs, and
    // drain stderr concurrently so the child can never block on a full pipe.
    let drive = async {
        let (parsed, stderr) = tokio::join!(parse_stream(stdout), drain(stderr));
        let status =
            child.wait().await.with_context(|| format!("waiting on `{CURSOR_AGENT_BIN}`"))?;
        anyhow::Ok((parsed, stderr, status))
    };

    // On timeout `drive` is dropped, and `kill_on_drop` reaps the orphaned agent.
    let (parsed, stderr, status) =
        tokio::time::timeout(options.timeout, drive).await.map_err(|_elapsed| {
            anyhow!("cursor-agent timed out after {}s", options.timeout.as_secs())
        })??;

    if !status.success() {
        bail!("cursor-agent exited with {status}: {}", String::from_utf8_lossy(&stderr).trim());
    }

    parsed
}

async fn parse_stream(stdout: impl AsyncRead + Unpin) -> Result<AgentOutput> {
    let mut lines = BufReader::new(stdout).lines();
    let mut parser = OutputParser::default();
    while let Some(line) = lines.next_line().await? {
        parser.line(&line)?;
    }
    parser.finish()
}

async fn drain(mut stream: impl AsyncRead + Unpin) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = stream.read_to_end(&mut buffer).await;
    buffer
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
fn into_prompt_file(prompt: &str, workspace: &Path) -> Result<(String, PromptFile)> {
    let cursor_dir = workspace.join(".cursor");
    std::fs::create_dir_all(&cursor_dir)
        .with_context(|| format!("creating {}", cursor_dir.display()))?;

    // The name carries the pid: concurrent host processes may lend the same workspace.
    let id = PROMPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = cursor_dir.join(format!("omnia-prompt-{}-{id}.txt", std::process::id()));
    std::fs::write(&path, prompt)
        .with_context(|| format!("writing prompt file {}", path.display()))?;

    let arg = format!(
        "Follow every instruction in the file at `{}`. When you are done, reply exactly as that \
         file instructs.",
        path.display()
    );

    Ok((arg, PromptFile { path }))
}

fn append_repair(prompt: &str, answer: &str, reason: &str) -> String {
    format!("{prompt}\n\nYour previous answer was:\n{answer}\n\n{}", repair_instruction(reason))
}

/// The subset of `cursor-agent` stream events the backend consumes; everything
/// else (assistant deltas, thinking, …) parses to `Other` without building a
/// JSON tree.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Result {
        is_error: Option<bool>,
        result: Option<String>,
    },
    ToolCall {
        subtype: String,
        call_id: Option<String>,
        tool_call: Option<Value>,
    },
    #[serde(other)]
    Other,
}

// Incremental parser for `stream-json` NDJSON (with a fallback for a legacy
// single-line `json` payload).
#[derive(Default)]
struct OutputParser {
    result: Option<String>,
    pending_tools: HashMap<String, (String, Value)>,
    turns: Vec<ToolTurn>,
    lines: u32,
    first_line: Option<String>,
}

impl OutputParser {
    fn line(&mut self, line: &str) -> Result<()> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(());
        }
        self.lines += 1;
        if self.lines == 1 {
            self.first_line = Some(line.to_owned());
        }

        // One garbled line must not cost an otherwise-successful answer.
        let event = match serde_json::from_str::<Event>(line) {
            Ok(event) => event,
            Err(error) => {
                tracing::debug!(%error, line, "skipping unparsable cursor-agent event");
                return Ok(());
            }
        };

        match event {
            Event::Result { is_error, result } => {
                if is_error == Some(true) {
                    bail!(
                        "cursor-agent reported an error: {}",
                        result.as_deref().unwrap_or("<no detail>")
                    );
                }
                if result.is_some() {
                    self.result = result;
                }
            }
            Event::ToolCall {
                subtype,
                call_id,
                tool_call,
            } => {
                self.tool_call(&subtype, call_id, tool_call);
            }
            Event::Other => {}
        }
        Ok(())
    }

    fn tool_call(&mut self, subtype: &str, call_id: Option<String>, tool_call: Option<Value>) {
        match subtype {
            "started" => {
                if let (Some(call_id), Some(identity)) =
                    (call_id, tool_call.as_ref().and_then(tool_call_identity))
                {
                    self.pending_tools.insert(call_id, identity);
                }
            }
            "completed" => {
                if let (Some(call_id), Some(tool_call)) = (call_id, tool_call) {
                    let (tool, args) = self.pending_tools.remove(&call_id).unwrap_or_else(|| {
                        tool_call_identity(&tool_call)
                            .unwrap_or_else(|| ("unknown".to_owned(), Value::Null))
                    });
                    let result = tool_call_result(&tool_call).unwrap_or(Value::Null);
                    self.turns.push(ToolTurn { tool, args, result });
                }
            }
            _ => {}
        }
    }

    fn finish(self) -> Result<AgentOutput> {
        if let Some(result) = self.result {
            let transcript =
                if self.turns.is_empty() { None } else { Some(Transcript { turns: self.turns }) };
            return Ok(AgentOutput { result, transcript });
        }

        if self.lines == 1
            && let Some(line) = &self.first_line
        {
            return parse_legacy_output(line);
        }

        bail!("cursor-agent did not emit a terminal result event");
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use omnia_wasi_model::{
        DirEntry, Format, FutureResult, Grants, Message, Reference, Request, Role, Schema,
        ToolHost, VerifyReport, WasiModelCtx as _,
    };
    use serde_json::json;

    use super::{AgentOutput, MAX_INLINE_SIZE, OutputParser, build_prompt, into_prompt_file};
    use crate::Client;

    fn parse_output(stdout: &[u8]) -> anyhow::Result<AgentOutput> {
        let text = std::str::from_utf8(stdout).expect("test payloads are UTF-8");
        let mut parser = OutputParser::default();
        for line in text.lines() {
            parser.line(line)?;
        }
        parser.finish()
    }

    fn schema_request() -> Request {
        Request {
            model: None,
            system: Some("a terse judge".to_owned()),
            messages: vec![Message {
                role: Role::User,
                content: "decide pass or fail".to_owned(),
            }],
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
            .complete(schema_request(), Arc::new(NoopToolHost))
            .await
            .expect_err("a backend with no local tree must fail");
        assert!(err.to_string().contains("no local tree on this node"), "unexpected error: {err}");
    }

    #[test]
    fn agent_prompt() {
        let text = build_prompt(&schema_request());
        assert!(text.contains("a terse judge"), "missing system channel: {text}");
        assert!(text.contains("decide pass or fail"), "missing user channel: {text}");
        assert!(text.contains("JSON Schema"), "missing schema instruction: {text}");
        assert!(text.contains("\"verdict\""), "missing schema body: {text}");
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
    fn skip_garbled_line() {
        let stdout =
            b"warning: not an event\n{\"type\":\"result\",\"is_error\":false,\"result\":\"ok\"}";
        let AgentOutput { result, .. } = parse_output(stdout).expect("garbled line is skipped");
        assert_eq!(result, "ok");
    }

    #[test]
    fn spill_large_prompt() {
        let workspace =
            std::env::temp_dir().join(format!("omnia-cursor-prompt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(&workspace).expect("temp workspace");

        let large = "x".repeat(MAX_INLINE_SIZE + 1);
        let (arg, spill) = into_prompt_file(&large, &workspace).expect("spill prompt");
        assert!(arg.contains("omnia-prompt-"), "arg references prompt file: {arg}");
        assert!(spill.path.exists(), "the prompt file is on disk while the guard lives");
        let path = spill.path.clone();
        drop(spill);
        assert!(!path.exists(), "the prompt file is removed on drop");
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[derive(Debug)]
    pub struct NoopToolHost;

    impl ToolHost for NoopToolHost {
        fn resolve(&self, _reference: Reference) -> FutureResult<Vec<u8>> {
            Box::pin(async { Err(anyhow::anyhow!("cursor ignores the tool host")) })
        }

        fn read(&self, _path: String) -> FutureResult<Vec<u8>> {
            Box::pin(async { Err(anyhow::anyhow!("cursor ignores the tool host")) })
        }

        fn list(&self, _path: String) -> FutureResult<Vec<DirEntry>> {
            Box::pin(async { Err(anyhow::anyhow!("cursor ignores the tool host")) })
        }

        fn write(&self, _path: String, _bytes: Vec<u8>) -> FutureResult<()> {
            Box::pin(async { Err(anyhow::anyhow!("cursor ignores the tool host")) })
        }

        fn verify(&self, _check: String) -> FutureResult<VerifyReport> {
            Box::pin(async { Err(anyhow::anyhow!("cursor ignores the tool host")) })
        }
    }

    /// Build a [`Client`] directly, bypassing `connect_with` (and its `PATH` check).
    pub fn client() -> Client {
        Client {
            timeout: Duration::from_secs(1),
        }
    }
}
