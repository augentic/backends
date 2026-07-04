#![doc = include_str!("../README.md")]
#![allow(clippy::multiple_crate_versions)]

mod mcp;
mod model;

use std::fmt::Debug;
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
    timeout: Duration,
}

impl Backend for Client {
    type ConnectOptions = ConnectOptions;

    #[instrument(name = "Cursor::connect_with")]
    async fn connect_with(options: Self::ConnectOptions) -> Result<Self> {
        let ConnectOptions = options;
        assert_cursor().await?;

        Ok(Self {
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

/// Connection options for the `cursor-agent` backend.
///
/// The backend reads no environment configuration; the working tree is lent per
/// completion through the guest's `grants.workspace`, which the host resolves to
/// a node-local path on the tool host.
#[derive(Debug, Clone, Default)]
pub struct ConnectOptions;

impl omnia::FromEnv for ConnectOptions {
    fn from_env() -> Result<Self> {
        Ok(Self)
    }
}
