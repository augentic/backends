#![doc = include_str!("../README.md")]
#![cfg(not(target_arch = "wasm32"))]
// `omnia-wasi-model` pulls in the wasmtime dependency tree, which carries
// duplicate transitive crates outside this crate's control; silence the
// workspace `cargo` lint here (as `omnia-genai` does).
#![allow(clippy::multiple_crate_versions)]

mod model;

use std::fmt::Debug;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use omnia::Backend;
use tracing::instrument;

/// The `cursor-agent` executable, resolved on `PATH`.
pub(crate) const CURSOR_AGENT_BIN: &str = "cursor-agent";

/// Spawned, filesystem-capable `cursor-agent` model backend.
///
/// The spawned-agent shape of RFC wasi-model §5.3: each completion launches a
/// fresh, context-free `cursor-agent` session that owns its own tool loop and
/// edits the lent working tree directly, then returns a validated answer through
/// the `augentic:model/completion` boundary. There is no long-lived client — the
/// handle only carries the optional model id, the workspace path, and the
/// per-spawn timeout. The provider API key (`CURSOR_API_KEY`, or a prior
/// `cursor-agent login`) is read by the spawned process and never captured here
/// (§7.5).
#[derive(Clone, Debug)]
pub struct Client {
    /// Optional model id passed to `cursor-agent --model`; `None` lets the agent
    /// pick its own default.
    model: Option<Arc<str>>,
    /// Stopgap node-local working-tree path lent via `--workspace`. `None` is the
    /// "no local tree on this node" capability signal (§5.3).
    workspace: Option<Arc<Path>>,
    /// Wall-clock bound on one `cursor-agent` spawn.
    timeout: Duration,
}

impl Backend for Client {
    type ConnectOptions = ConnectOptions;

    #[instrument(name = "Cursor::connect_with")]
    async fn connect_with(options: Self::ConnectOptions) -> Result<Self> {
        // No long-lived process: each completion spawns a fresh session. Connect
        // only validates the `cursor-agent` binary is reachable, so a
        // misconfigured node fails at startup rather than mid-completion (§5.3).
        ensure_cursor_agent().await?;

        tracing::info!(
            model = options.model.as_deref().unwrap_or("<cursor-agent default>"),
            workspace = options.workspace.as_deref().unwrap_or("<none>"),
            timeout_secs = options.timeout_secs,
            "configured cursor backend"
        );

        Ok(Self {
            model: options.model.map(Arc::from),
            workspace: options.workspace.map(|w| Arc::from(Path::new(&w))),
            timeout: Duration::from_secs(options.timeout_secs),
        })
    }
}

/// Validate that `cursor-agent` is installed and runnable by invoking
/// `cursor-agent --version`.
async fn ensure_cursor_agent() -> Result<()> {
    let output = tokio::process::Command::new(CURSOR_AGENT_BIN)
        .arg("--version")
        .output()
        .await
        .with_context(|| {
            format!("`{CURSOR_AGENT_BIN}` not found on PATH (is the Cursor CLI installed?)")
        })?;

    if !output.status.success() {
        bail!(
            "`{CURSOR_AGENT_BIN} --version` failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(())
}

#[allow(missing_docs)]
mod config {
    use fromenv::FromEnv;

    /// Connection options for the spawned `cursor-agent` backend.
    #[derive(Debug, Clone, FromEnv)]
    pub struct ConnectOptions {
        /// Optional model id forwarded to `cursor-agent --model`. Unset lets the
        /// agent choose its own default (cursor model ids differ from genai's, so
        /// there is no portable default to pin here).
        #[env(from = "OMNI_MODEL")]
        pub model: Option<String>,
        /// Stopgap working-tree path lent to the agent via `--workspace`, standing
        /// in for the RFC-55 `local-path` face until that host lands. When unset,
        /// `complete` reports the "no local tree on this node" capability signal.
        #[env(from = "OMNI_WORKSPACE")]
        pub workspace: Option<String>,
        /// Wall-clock bound (seconds) on one `cursor-agent` spawn; a hung agent
        /// run fails loudly rather than blocking the completion indefinitely.
        #[env(from = "OMNI_CURSOR_TIMEOUT_SECS", default = "120")]
        pub timeout_secs: u64,
    }
}
pub use config::ConnectOptions;

impl omnia::FromEnv for ConnectOptions {
    fn from_env() -> Result<Self> {
        Self::from_env().finalize().context("issue loading connection options")
    }
}
