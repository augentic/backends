//! Provider conversation loop.
//!
//! Tool calls and rejected checks may add further provider rounds. All
//! rounds share one budget, bounding cost and guaranteeing termination.

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ToolCall, ToolResponse};
use omnia_wasi_model::{
    Answer, Error, Format, ToolHost, ToolTurn, Transcript, Usage, WasiModelCtx as _,
};
use serde_json::Value;

use super::observe::{self, Completion, Failure};
use super::options::Turn;
use super::tools;
use crate::Client;

const MAX_ROUNDS: usize = 8;

pub struct Conversation {
    client: genai::Client,
    model: String,
    chat: ChatRequest,
    options: ChatOptions,
    format: Format,
    check: bool,
    tool_host: Arc<dyn ToolHost>,
    max_result_bytes: usize,
    transcript: Transcript,
    completion: Option<Completion>,
}

impl Conversation {
    pub fn new(client: &Client, turn: Turn, tool_host: Arc<dyn ToolHost>) -> Self {
        let completion = Completion::start(&turn);

        Self {
            client: client.inner.clone(),
            model: turn.model,
            chat: turn.chat,
            options: turn.options,
            format: turn.format,
            check: turn.check,
            tool_host,
            max_result_bytes: client.limits().max_result_bytes,
            transcript: Transcript::default(),
            completion: Some(completion),
        }
    }

    pub async fn complete(mut self) -> Result<Answer> {
        let result = self.run().await;
        let attempts = self.completion.as_ref().map_or(0, Completion::attempts);
        let outcome = match &result {
            Ok(_) if attempts > 1 => "corrected",
            Ok(_) => "ok",
            Err(error) => observe::outcome_of(error),
        };
        if let Some(completion) = self.completion.take() {
            completion.finish(outcome);
        }
        result
    }

    async fn run(&mut self) -> Result<Answer> {
        for round in 1..=MAX_ROUNDS {
            let response = self
                .client
                .exec_chat(&self.model, self.chat.clone(), Some(&self.options))
                .await
                .with_context(|| format!("genai exec_chat failed for model `{}`", self.model))?;

            let text = response.first_text().map(ToOwned::to_owned);
            let usage = to_usage(&response.usage);
            let tool_calls = response.into_tool_calls();

            if !tool_calls.is_empty() {
                self.tool_round(tool_calls).await?;
                continue;
            }

            let Some(text) = text else {
                bail!("genai returned neither content nor tool calls (model `{}`)", self.model);
            };

            if let Some(completion) = &mut self.completion {
                completion.new_attempt();
                completion.record(text.len(), self.transcript.turns.len(), usage.as_ref());
            }

            let candidate = self.format.candidate(&text);
            if !self.check {
                return Ok(self.answer(candidate, usage));
            }

            match self.tool_host.check(candidate.clone()).await? {
                Ok(()) => return Ok(self.answer(candidate, usage)),
                // The guest's correction is the model's next turn, verbatim;
                // on the last round it is the typed failure the guest sees.
                Err(correction) if round == MAX_ROUNDS => {
                    bail!(Error::BudgetExhausted(correction));
                }
                Err(correction) => {
                    tracing::debug!(%correction, "check rejected the candidate");
                    self.chat = std::mem::take(&mut self.chat)
                        .append_message(ChatMessage::assistant(candidate))
                        .append_message(ChatMessage::user(correction));
                }
            }
        }

        Err(Failure::Exhausted { rounds: MAX_ROUNDS }.into())
    }

    async fn tool_round(&mut self, tool_calls: Vec<ToolCall>) -> Result<()> {
        let mut chat = std::mem::take(&mut self.chat).append_message(tool_calls.clone());
        for call in tool_calls {
            let result =
                tools::dispatch_tool(&self.tool_host, &call, self.max_result_bytes).await?;
            self.transcript.turns.push(ToolTurn {
                tool: call.fn_name,
                args: call.fn_arguments,
                result: Value::String(result.clone()),
            });
            chat = chat.append_message(ToolResponse::new(call.call_id, result));
        }
        self.chat = chat;
        Ok(())
    }

    fn answer(&mut self, answer: String, usage: Option<Usage>) -> Answer {
        Answer {
            answer,
            usage,
            transcript: Some(std::mem::take(&mut self.transcript)),
        }
    }
}

// `None` when the provider did not surface any counts.
fn to_usage(usage: &genai::chat::Usage) -> Option<Usage> {
    if usage.prompt_tokens.is_none() && usage.completion_tokens.is_none() {
        return None;
    }
    Some(Usage {
        input_tokens: usage.prompt_tokens.and_then(|v| u32::try_from(v).ok()).unwrap_or(0),
        output_tokens: usage.completion_tokens.and_then(|v| u32::try_from(v).ok()).unwrap_or(0),
        reasoning_tokens: usage
            .completion_tokens_details
            .as_ref()
            .and_then(|d| d.reasoning_tokens)
            .and_then(|v| u32::try_from(v).ok()),
    })
}

// The check loop over a scripted chat-completions provider (CI floor):
// accept, correct-then-accept, exhaust. `tests/live.rs` proves the same loop
// against a real provider.
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use genai::ServiceTarget;
    use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
    use http_body_util::{BodyExt as _, Full};
    use hyper::body::{Bytes, Incoming};
    use hyper::header::CONTENT_TYPE;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use omnia_wasi_model::{
        DirEntry, Error, Format, FutureResult, Grants, Message, Request, Role, ToolHost,
        WasiModelCtx as _,
    };
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    use super::MAX_ROUNDS;
    use crate::Client;

    /// Every request body a scripted provider received, in order.
    type Requests = Arc<Mutex<Vec<Value>>>;

    /// A client bound to a loopback chat-completions endpoint that answers
    /// request `n` with `replies[n]` (the last reply repeats) and records
    /// each request body.
    async fn scripted(replies: &[&str]) -> (Client, Requests) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind loopback");
        let addr = listener.local_addr().expect("local address");
        let replies: Arc<Vec<String>> = Arc::new(replies.iter().map(|r| (*r).to_owned()).collect());
        let requests = Requests::default();
        tokio::spawn(serve(listener, replies, Arc::clone(&requests)));

        let base = format!("http://{addr}/v1/");
        let resolver = ServiceTargetResolver::from_resolver_fn(
            move |mut target: ServiceTarget| -> genai::resolver::Result<ServiceTarget> {
                target.endpoint = Endpoint::from_owned(base.clone());
                target.auth = AuthData::from_single("test-key");
                Ok(target)
            },
        );
        let client = Client {
            // A name the SDK routes to its OpenAI chat-completions adapter.
            model: "gpt-4o-mini".to_owned(),
            inner: genai::Client::builder().with_service_target_resolver(resolver).build(),
        };
        (client, requests)
    }

    async fn serve(listener: TcpListener, replies: Arc<Vec<String>>, requests: Requests) {
        while let Ok((stream, _)) = listener.accept().await {
            let replies = Arc::clone(&replies);
            let requests = Arc::clone(&requests);
            tokio::spawn(async move {
                let service = service_fn(move |request: hyper::Request<Incoming>| {
                    let replies = Arc::clone(&replies);
                    let requests = Arc::clone(&requests);
                    async move {
                        let body = request.into_body().collect().await?.to_bytes();
                        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                        let round = {
                            let mut seen = requests.lock().expect("requests lock");
                            seen.push(body);
                            seen.len() - 1
                        };
                        let content = replies.get(round).or_else(|| replies.last());
                        Ok::<_, hyper::Error>(completion(content.map_or("", String::as_str)))
                    }
                });
                let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), service).await;
            });
        }
    }

    /// One OpenAI-shaped chat completion carrying `content`.
    fn completion(content: &str) -> hyper::Response<Full<Bytes>> {
        let body = json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "model": "gpt-4o-mini",
            "choices": [{
                "index": 0,
                "finish_reason": "stop",
                "message": { "role": "assistant", "content": content },
            }],
            "usage": { "prompt_tokens": 3, "completion_tokens": 1, "total_tokens": 4 },
        });
        hyper::Response::builder()
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body.to_string())))
            .expect("a well-formed response")
    }

    /// The guest's stand-in: rejects the first `rejections` candidates with a
    /// correction naming them, accepts the rest, and records every candidate.
    #[derive(Debug)]
    struct Check {
        rejections: usize,
        seen: AtomicUsize,
        candidates: Mutex<Vec<String>>,
    }

    impl Check {
        fn rejecting(rejections: usize) -> Arc<Self> {
            Arc::new(Self {
                rejections,
                seen: AtomicUsize::new(0),
                candidates: Mutex::new(Vec::new()),
            })
        }

        fn host(self: &Arc<Self>) -> Arc<dyn ToolHost> {
            let host: Arc<Self> = Arc::clone(self);
            host
        }

        fn candidates(&self) -> Vec<String> {
            self.candidates.lock().expect("candidates lock").clone()
        }
    }

    impl ToolHost for Check {
        fn call_tool(
            &self, name: String, _arguments: String,
        ) -> FutureResult<Result<String, String>> {
            Box::pin(
                async move { Err(anyhow::anyhow!("no function tools are declared: `{name}`")) },
            )
        }

        fn read(&self, _path: String) -> FutureResult<Vec<u8>> {
            Box::pin(async { Err(anyhow::anyhow!("no workspace is lent")) })
        }

        fn list(&self, _path: String) -> FutureResult<Vec<DirEntry>> {
            Box::pin(async { Err(anyhow::anyhow!("no workspace is lent")) })
        }

        fn write(&self, _path: String, _bytes: Vec<u8>) -> FutureResult<()> {
            Box::pin(async { Err(anyhow::anyhow!("no workspace is lent")) })
        }

        fn check(&self, candidate: String) -> FutureResult<Result<(), String>> {
            let seen = self.seen.fetch_add(1, Ordering::SeqCst);
            self.candidates.lock().expect("candidates lock").push(candidate.clone());
            let verdict = if seen < self.rejections {
                Err(format!(
                    "## Previous answer (rejected)\n\n{candidate}\n\n## Findings\n\nnot it"
                ))
            } else {
                Ok(())
            };
            Box::pin(async move { Ok(verdict) })
        }
    }

    fn request(check: bool) -> Request {
        Request {
            model: None,
            system: Some("answer with one word".to_owned()),
            messages: vec![Message {
                role: Role::User,
                content: "hi".to_owned(),
            }],
            generation: None,
            format: Format::Text,
            tools: vec![],
            grants: Grants { workspace: None },
            check,
        }
    }

    /// The `messages` of the `n`th request as `(role, content)` pairs.
    fn messages(requests: &Requests, n: usize) -> Vec<(String, String)> {
        let requests = requests.lock().expect("requests lock");
        requests[n]["messages"]
            .as_array()
            .expect("messages array")
            .iter()
            .map(|m| {
                (
                    m["role"].as_str().unwrap_or_default().to_owned(),
                    m["content"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn unchecked() {
        let (client, requests) = scripted(&["alpha"]).await;
        let check = Check::rejecting(usize::MAX);
        let answer = client.complete(request(false), check.host()).await.expect("completes");
        assert_eq!(answer.answer, "alpha");
        assert_eq!(answer.usage.map(|u| (u.input_tokens, u.output_tokens)), Some((3, 1)));
        assert!(check.candidates().is_empty(), "no check was asked for");
        assert_eq!(requests.lock().expect("requests lock").len(), 1);
    }

    #[tokio::test]
    async fn check_accepts() {
        let (client, requests) = scripted(&["alpha"]).await;
        let check = Check::rejecting(0);
        let answer = client.complete(request(true), check.host()).await.expect("completes");
        assert_eq!(answer.answer, "alpha");
        assert_eq!(check.candidates(), ["alpha"]);
        assert_eq!(requests.lock().expect("requests lock").len(), 1);
    }

    #[tokio::test]
    async fn check_corrects() {
        let (client, requests) = scripted(&["alpha", "beta"]).await;
        let check = Check::rejecting(1);
        let answer = client.complete(request(true), check.host()).await.expect("completes");
        assert_eq!(answer.answer, "beta", "the accepted candidate is the answer");
        assert_eq!(check.candidates(), ["alpha", "beta"]);

        // The second round carries the rejected candidate as the assistant
        // turn and the correction, verbatim, as the user turn after it.
        assert_eq!(requests.lock().expect("requests lock").len(), 2);
        let second = messages(&requests, 1);
        let tail = &second[second.len() - 2..];
        assert_eq!(tail[0], ("assistant".to_owned(), "alpha".to_owned()));
        assert_eq!(tail[1].0, "user");
        assert_eq!(tail[1].1, "## Previous answer (rejected)\n\nalpha\n\n## Findings\n\nnot it");
    }

    #[tokio::test]
    async fn check_exhausts() {
        let (client, requests) = scripted(&["alpha"]).await;
        let check = Check::rejecting(usize::MAX);
        let error = client
            .complete(request(true), check.host())
            .await
            .expect_err("every candidate is rejected");
        let Some(Error::BudgetExhausted(correction)) = error.downcast_ref::<Error>() else {
            panic!("expected the typed budget-exhausted: {error:?}");
        };
        assert!(correction.contains("## Findings\n\nnot it"), "the last correction: {correction}");
        assert_eq!(check.candidates().len(), MAX_ROUNDS, "every round offered a candidate");
        assert_eq!(requests.lock().expect("requests lock").len(), MAX_ROUNDS);
    }
}
