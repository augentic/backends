#![doc = include_str!("../README.md")]
#![allow(clippy::multiple_crate_versions)]

mod mcp_json;
mod model;

#[cfg(test)]
pub(crate) mod test_support;

use std::fmt::Debug;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use omnia::Backend;
use tokio::process::Command;
use tracing::instrument;

/// The `cursor-agent` executable, resolved on `PATH`.
pub(crate) const CURSOR_AGENT_BIN: &str = "cursor-agent";

/// Spawned, filesystem-capable `cursor-agent` model backend.
#[derive(Clone, Debug)]
pub struct Client {
    model: Option<Arc<str>>,
    workspace: Option<Arc<Path>>,
    timeout: Duration,
    /// URL of an omnia-hosted MCP server to advertise to the spawned agent via
    /// `.cursor/mcp.json`, when configured.
    mcp_url: Option<Arc<str>>,
    /// When true, run each spawn in an isolated git worktree (`--worktree`).
    use_worktree: bool,
}

impl Backend for Client {
    type ConnectOptions = ConnectOptions;

    #[instrument(name = "Cursor::connect_with")]
    async fn connect_with(options: Self::ConnectOptions) -> Result<Self> {
        ensure_cursor().await?;

        tracing::info!(
            model = options.model.as_deref().unwrap_or("<cursor-agent default>"),
            workspace = options.workspace.as_deref().unwrap_or("<none>"),
            timeout_secs = options.timeout_secs,
            mcp_url = options.mcp_url.as_deref().unwrap_or("<none>"),
            use_worktree = options.use_worktree,
            "configured cursor backend"
        );

        Ok(Self {
            model: options.model.map(Arc::from),
            workspace: options.workspace.map(|w| Arc::from(Path::new(&w))),
            timeout: Duration::from_secs(options.timeout_secs),
            mcp_url: options.mcp_url.map(Arc::from),
            use_worktree: options.use_worktree,
        })
    }
}

/// Validate that `cursor-agent` is installed and runnable by invoking
/// `cursor-agent --version`.
async fn ensure_cursor() -> Result<()> {
    let output = Command::new(CURSOR_AGENT_BIN)
        .arg("--version")
        .output()
        .await
        .context("cursor-agent not found on PATH")?;

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

    /// Connection options for the `cursor-agent` backend.
    #[derive(Debug, Clone, FromEnv)]
    pub struct ConnectOptions {
        /// Model to use.
        #[env(from = "CURSOR_MODEL")]
        pub model: Option<String>,
        /// Workspace path.
        #[env(from = "OMNIA_WORKSPACE")]
        pub workspace: Option<String>,
        /// Wall-clock bound (seconds) on one `cursor-agent` spawn.
        #[env(from = "CURSOR_TIMEOUT_SECS", default = "120")]
        pub timeout_secs: u64,
        /// URL of an omnia-hosted MCP server to advertise to the spawned agent
        /// through `.cursor/mcp.json`. Unset disables MCP wiring.
        #[env(from = "CURSOR_MCP_URL")]
        pub mcp_url: Option<String>,
        /// Run each spawn in an isolated git worktree instead of editing the
        /// lent tree directly (`cursor-agent --worktree`).
        #[env(from = "CURSOR_USE_WORKTREE", default = "false")]
        pub use_worktree: bool,
    }
}
pub use config::ConnectOptions;

impl omnia::FromEnv for ConnectOptions {
    fn from_env() -> Result<Self> {
        Self::from_env().finalize().context("issue loading connection options")
    }
}
