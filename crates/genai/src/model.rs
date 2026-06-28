//! `wasi-model` implementation backed by the multi-provider genai SDK.
//!
//! Maps the floor's typed [`Prompt`] onto a genai [`ChatRequest`]/[`ChatOptions`]
//! (§3.1.1 assembly precedence, reusing [`assemble`]), drives the in-process
//! tool loop — dispatching the floor's `resolve` tool into the caller's
//! `references` shelf through the lent [`ToolHost`] — self-checks the answer
//! against `response-format`, and returns a host-only [`BackendAnswer`] (the
//! parsed value plus a tool transcript for record/replay). The guest only ever
//! sees the validated answer string the `complete` binding derives from `value`;
//! the floor re-validates as the single authority (§3.1.3), so this self-check
//! is an optimization, not the gate.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use futures::FutureExt as _;
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatResponseFormat, JsonSpec, Tool, ToolCall,
    ToolResponse,
};
use omnia_wasi_model::{
    BackendAnswer, FutureResult, Prompt, Reference, ResponseFormatKind, ToolHost, ToolTurn,
    Transcript, WasiModelCtx, assemble, validate_answer,
};
use serde_json::{Value, json};

use crate::Client;

/// Hard cap on model round-trips for one completion: tool-call rounds plus
/// answer-repair attempts share this budget. It bounds cost and guarantees the
/// loop terminates (the per-call budget invariant of the RFC).
const MAX_TURNS: usize = 8;

impl WasiModelCtx for Client {
    fn complete(
        &self, prompt: Prompt, tool_host: Arc<dyn ToolHost>,
    ) -> FutureResult<BackendAnswer> {
        // Clone the swappable vendor handle + model id into the 'static future;
        // the genai client is cheap to clone (an `Arc` inside).
        let client = self.inner.clone();
        let model = Arc::clone(&self.model);

        async move {
            let kind = prompt.response_format.kind;
            let mut request = build_request(&prompt)?;
            let options = build_options(&prompt)?;

            let mut transcript = Transcript::default();

            for turn in 1..=MAX_TURNS {
                let response = client
                    .exec_chat(&*model, request.clone(), Some(&options))
                    .await
                    .with_context(|| format!("genai exec_chat failed for model `{model}`"))?;

                // Capture the text turn before consuming the response for tool calls.
                let text = response.first_text().map(ToOwned::to_owned);
                let tool_calls = response.into_tool_calls();

                if !tool_calls.is_empty() {
                    // The assistant turn carries all the tool calls; each tool
                    // response follows as its own `tool`-role message.
                    request = request.append_message(tool_calls.clone());
                    for call in tool_calls {
                        let result = dispatch_tool(&prompt, &tool_host, &call).await?;
                        transcript.turns.push(ToolTurn {
                            tool: call.fn_name,
                            args: call.fn_arguments,
                            result: Value::String(result.clone()),
                        });
                        request = request.append_message(ToolResponse::new(call.call_id, result));
                    }
                    continue;
                }

                // No tool calls: this is the model's (attempted) final answer.
                let Some(text) = text else {
                    bail!("genai returned neither content nor tool calls (model `{model}`)");
                };
                let last_turn = turn == MAX_TURNS;

                match parse_answer(&text, kind) {
                    Ok(value) => match validate_answer(&value, kind) {
                        Ok(()) => {
                            return Ok(BackendAnswer {
                                value,
                                transcript: Some(transcript),
                            });
                        }
                        // Budget spent: hand the value back so the floor's single
                        // validation authority produces the canonical error.
                        Err(_) if last_turn => {
                            return Ok(BackendAnswer {
                                value,
                                transcript: Some(transcript),
                            });
                        }
                        Err(reason) => {
                            request = append_repair(request, text, &format!("{reason:?}"));
                        }
                    },
                    Err(reason) if last_turn => {
                        bail!(
                            "genai did not return a valid answer for model `{model}` after \
                             {MAX_TURNS} attempts: {reason}"
                        );
                    }
                    Err(reason) => {
                        request = append_repair(request, text, &reason);
                    }
                }
            }

            bail!(
                "genai completion exceeded {MAX_TURNS} model round-trips without a final answer \
                 (model `{model}`)"
            )
        }
        .boxed()
    }
}

/// Assemble the floor [`Prompt`] into a genai [`ChatRequest`] (§3.1.1): explicit
/// turns beat the section template, `system` is always its own channel, and the
/// floor `resolve` tool is advertised only when a reference target is granted.
fn build_request(prompt: &Prompt) -> Result<ChatRequest> {
    let assembled =
        assemble(prompt).map_err(|e| anyhow::anyhow!("prompt assembly failed: {e:?}"))?;

    let messages = assembled
        .messages
        .iter()
        .map(|m| match m.role.as_str() {
            "system" => ChatMessage::system(m.content.clone()),
            "assistant" => ChatMessage::assistant(m.content.clone()),
            // The floor's roles are system/user/assistant; anything else is
            // treated as a user turn rather than dropped.
            _ => ChatMessage::user(m.content.clone()),
        })
        .collect();

    let mut request = ChatRequest::new(messages);
    if let Some(system) = assembled.system {
        request = request.with_system(system);
    }

    let mut tools: Vec<Tool> = Vec::new();
    if prompt.grants.references.is_some() {
        tools.push(resolve_tool());
    }
    for tool in &prompt.tools {
        let schema: Value = serde_json::from_str(&tool.parameters).with_context(|| {
            format!("guest tool `{}` has invalid JSON-Schema parameters", tool.name)
        })?;
        tools.push(
            Tool::new(tool.name.clone())
                .with_description(tool.description.clone())
                .with_schema(schema),
        );
    }
    if !tools.is_empty() {
        request = request.with_tools(tools);
    }

    Ok(request)
}

/// The floor `resolve` tool advertised to the model (§4). Its single `name`
/// argument mirrors [`Reference`]; the body is opaque to the floor and is
/// interpreted only by the caller's `references` shelf.
fn resolve_tool() -> Tool {
    Tool::new("resolve")
        .with_description(
            "Resolve an opaque reference against the caller's references shelf and return its \
             contents. Use this to fetch material the caller exposed by reference.",
        )
        .with_schema(json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The opaque reference body the shelf interprets."
                }
            },
            "required": ["name"],
            "additionalProperties": false,
        }))
}

/// Translate the floor's `response-format` and `generation` controls into genai
/// [`ChatOptions`].
fn build_options(prompt: &Prompt) -> Result<ChatOptions> {
    let mut options = ChatOptions::default();

    options = match (prompt.response_format.kind, &prompt.response_format.json_schema) {
        (ResponseFormatKind::JsonSchema, Some(spec)) => {
            let schema: Value = serde_json::from_str(&spec.schema)
                .context("response-format json-schema is not valid JSON")?;
            options.with_response_format(ChatResponseFormat::JsonSpec(JsonSpec::new(
                spec.name.clone(),
                schema,
            )))
        }
        // `json-object`, or `json-schema` with no spec attached: request the
        // provider's JSON mode (the strongest portable structured-output hint).
        (ResponseFormatKind::JsonObject | ResponseFormatKind::JsonSchema, _) => {
            options.with_response_format(ChatResponseFormat::JsonMode)
        }
        // `text`: a plain string answer, no structured-output hint.
        (ResponseFormatKind::Text, _) => options,
    };

    if let Some(generation) = &prompt.generation {
        if let Some(temperature) = generation.temperature {
            options = options.with_temperature(f64::from(temperature));
        }
        if let Some(top_p) = generation.top_p {
            options = options.with_top_p(f64::from(top_p));
        }
        if let Some(max_tokens) = generation.max_tokens {
            options = options.with_max_tokens(max_tokens);
        }
        if !generation.stop.is_empty() {
            options = options.with_stop_sequences(generation.stop.clone());
        }
    }

    Ok(options)
}

/// Execute one model tool call. Phase 2a wires only `resolve` (host-mediated
/// dynamic linking into the caller's `references` shelf); the other floor tools
/// (`read`/`list`/`write`/`verify`) and guest-declared tools are not executable
/// here yet, so the backend fails loudly rather than fabricating a result.
async fn dispatch_tool(
    prompt: &Prompt, tool_host: &Arc<dyn ToolHost>, call: &ToolCall,
) -> Result<String> {
    match call.fn_name.as_str() {
        "resolve" => {
            // The tool is only advertised with a granted target; re-check.
            if prompt.grants.references.is_none() {
                bail!("model called `resolve` but `grants.references` is not set");
            }
            let name = call
                .fn_arguments
                .get("name")
                .and_then(Value::as_str)
                .context("`resolve` tool call is missing a string `name` argument")?;
            let bytes = tool_host
                .resolve(Reference {
                    name: name.to_owned(),
                })
                .await
                .with_context(|| format!("resolving reference `{name}`"))?;
            // The shelf returns typed bytes; genai tool responses are strings, so
            // surface them as (lossy) UTF-8 text for the model to read.
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        }
        other => bail!(
            "model called tool `{other}`, which the genai backend cannot execute in Phase 2a \
             (only `resolve` is wired; `read`/`list`/`write`/`verify` and guest-declared tools \
             land in Phase 2b)"
        ),
    }
}

/// Interpret the model's text turn as the answer value for the requested kind.
/// `text` answers wrap the string verbatim; JSON kinds must parse.
fn parse_answer(text: &str, kind: ResponseFormatKind) -> Result<Value, String> {
    match kind {
        ResponseFormatKind::Text => Ok(Value::String(text.to_owned())),
        ResponseFormatKind::JsonObject | ResponseFormatKind::JsonSchema => {
            serde_json::from_str::<Value>(text)
                .map_err(|e| format!("the answer was not valid JSON: {e}"))
        }
    }
}

/// Append the rejected answer and a correction instruction so the next round
/// can repair it (bounded by [`MAX_TURNS`]).
fn append_repair(request: ChatRequest, answer: String, reason: &str) -> ChatRequest {
    let instruction = format!(
        "Your previous answer did not satisfy the required response format ({reason}). Reply \
         again with only the corrected answer and nothing else."
    );
    request
        .append_message(ChatMessage::assistant(answer))
        .append_message(ChatMessage::user(instruction))
}
