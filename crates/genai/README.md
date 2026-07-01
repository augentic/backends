# omnia-genai

[![crates.io](https://img.shields.io/crates/v/omnia-genai.svg)](https://crates.io/crates/omnia-genai)
[![docs.rs](https://docs.rs/omnia-genai/badge.svg)](https://docs.rs/omnia-genai)

Multi-provider generative-AI model backend for the Omnia WASI runtime,
implementing the `augentic:model/completion` boundary (`wasi-model`).

Wraps the [`genai`](https://crates.io/crates/genai) SDK (`OpenAI`, Anthropic,
Gemini, Groq, Ollama, …). The host assembles a guest's typed `Prompt` into a
`PreparedPrompt`, which this backend maps to a provider chat request; the
in-process tool loop is driven to completion, and the runtime core's
`resolve` tool is dispatched into the guest's `references` shelf via the lent
`ToolHost`. The guest only ever sees the validated answer string.

MSRV: Rust 1.95

## Configuration

| Variable       | Required | Default   | Description                                               |
| -------------- | -------- | --------- | --------------------------------------------------------- |
| `CURSOR_MODEL` | no       | `gpt-5.5` | Provider model id; genai routes to the provider by prefix |

Provider API keys are read by genai from the ambient environment
(`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, …) and are never
logged or recorded.

## Usage

```rust,ignore
use omnia::Backend;
use omnia_genai::Client;

let options = omnia_genai::ConnectOptions::from_env()?;
let client = Client::connect_with(options).await?;
```

## License

MIT OR Apache-2.0
