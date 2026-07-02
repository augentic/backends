#![doc = include_str!("../README.md")]
#![allow(clippy::multiple_crate_versions)]

mod mcp;
mod model;

#[cfg(test)]
pub(crate) mod test_support;

use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use omnia::Backend;
use tokio::process::Command;
use tracing::instrument;

pub(crate) const CURSOR_AGENT_BIN: &str = "cursor-agent";

/// Wall-clock bound on one `cursor-agent` spawn; orphaned processes are killed on timeout.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Spawned, filesystem-capable `cursor-agent` model backend.
#[derive(Clone, Debug)]
pub struct Client {
    workspace: Option<Arc<Path>>,
    timeout: Duration,
}

impl Backend for Client {
    type ConnectOptions = ConnectOptions;

    #[instrument(name = "Cursor::connect_with")]
    async fn connect_with(options: Self::ConnectOptions) -> Result<Self> {
        assert_cursor().await?;

        Ok(Self {
            workspace: options.workspace.map(|w| Arc::from(PathBuf::from(w))),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
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
        /// Workspace path.
        #[env(from = "OMNIA_WORKSPACE")]
        pub workspace: Option<String>,
    }
}
pub use config::ConnectOptions;

impl omnia::FromEnv for ConnectOptions {
    fn from_env() -> Result<Self> {
        Self::from_env().finalize().context("issue loading connection options")
    }
}
