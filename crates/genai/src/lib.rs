#![doc = include_str!("../README.md")]
// The genai SDK's dependency tree pulls duplicate transitive crates (e.g.
// `schemars`, `indexmap`); these are outside this crate's control and cannot be
// unified without patching upstream, so silence the workspace `cargo` lint here.
#![allow(clippy::multiple_crate_versions)]

mod model;

use std::fmt::Debug;
use std::sync::Arc;

use anyhow::{Context, Result};
use omnia::Backend;
use tracing::instrument;

/// Multi-provider generative-AI model backend.
#[derive(Clone)]
pub struct Client {
    inner: genai::Client,
    model: Arc<str>,
}

impl Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("model", &self.model).finish_non_exhaustive()
    }
}

impl Backend for Client {
    type ConnectOptions = ConnectOptions;

    #[instrument(name = "GenAi::connect_with")]
    async fn connect_with(options: Self::ConnectOptions) -> Result<Self> {
        // The genai client reads provider API keys from the ambient environment
        // (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, …) on demand. Keys are never
        // captured here, logged, or recorded into fixtures (§7.5).
        let inner = genai::Client::default();
        tracing::info!(model = %options.model, "configured genai backend");
        Ok(Self {
            inner,
            model: Arc::from(options.model),
        })
    }
}

#[allow(missing_docs)]
mod config {
    use fromenv::FromEnv;

    /// Connection options for the generative-AI backend.
    #[derive(Debug, Clone, FromEnv)]
    pub struct ConnectOptions {
        /// Provider model id (e.g. `gpt-5.5`, `claude-…`, `gemini-…`). genai
        /// routes to the provider by the model id's prefix.
        #[env(from = "CURSOR_MODEL", default = "gpt-5.5")]
        pub model: String,
    }
}
pub use config::ConnectOptions;

impl omnia::FromEnv for ConnectOptions {
    fn from_env() -> Result<Self> {
        Self::from_env().finalize().context("issue loading connection options")
    }
}
