#![doc = include_str!("../README.md")]
#![allow(clippy::multiple_crate_versions)]

mod mcp;
mod model;

#[cfg(test)]
pub(crate) mod test_support;

use std::collections::HashMap;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use omnia::Backend;
use tokio::process::Command;
use tracing::instrument;

pub(crate) const CURSOR_AGENT_BIN: &str = "cursor-agent";

/// Spawned, filesystem-capable `cursor-agent` model backend.
#[derive(Clone, Debug)]
pub struct Client {
    model: Option<Arc<str>>,
    workspace: Option<Arc<Path>>,
    timeout: Duration,
    /// Configured MCP servers, keyed by the logical name a prompt's `mcp` grant
    /// selects. Deployment topology (Law 2): URLs live here, never in the guest.
    mcp_servers: Arc<HashMap<String, String>>,
}

impl Backend for Client {
    type ConnectOptions = ConnectOptions;

    #[instrument(name = "Cursor::connect_with")]
    async fn connect_with(options: Self::ConnectOptions) -> Result<Self> {
        assert_cursor().await?;

        let mcp_servers = match options.mcp_servers {
            Some(json) => serde_json::from_str::<HashMap<String, String>>(&json)
                .context("CURSOR_MCP_SERVERS must be a JSON object mapping name to URL")?,
            None => HashMap::new(),
        };

        Ok(Self {
            model: options.model.map(Arc::from),
            workspace: options.workspace.map(|w| Arc::from(PathBuf::from(w))),
            timeout: Duration::from_secs(options.timeout_secs),
            mcp_servers: Arc::new(mcp_servers),
        })
    }
}

async fn assert_cursor() -> Result<()> {
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
        /// JSON object mapping logical MCP server names to endpoint URLs, e.g.
        /// `{"docs":"http://127.0.0.1:8080/mcp/docs"}`. A prompt's `mcp` tool
        /// grant selects servers by name; unset disables MCP wiring.
        #[env(from = "CURSOR_MCP_SERVERS")]
        pub mcp_servers: Option<String>,
    }
}
pub use config::ConnectOptions;

impl omnia::FromEnv for ConnectOptions {
    fn from_env() -> Result<Self> {
        Self::from_env().finalize().context("issue loading connection options")
    }
}
