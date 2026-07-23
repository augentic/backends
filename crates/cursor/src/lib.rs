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

/// Environment variable read by [`omnia::FromEnv::from_env`]: seconds for the
/// per-spawn wall-clock bound (unset → [`DEFAULT_TIMEOUT_SECS`]).
const TIMEOUT_SECS_ENV: &str = "CURSOR_TIMEOUT_SECS";

/// Spawned, filesystem-capable `cursor-agent` model backend.
#[derive(Clone, Debug)]
pub struct Client {
    timeout: Duration,
}

impl Backend for Client {
    type ConnectOptions = ConnectOptions;

    #[instrument(name = "Cursor::connect_with", skip_all, fields(timeout_secs = options.timeout.as_secs()))]
    async fn connect_with(options: Self::ConnectOptions) -> Result<Self> {
        assert_cursor().await?;

        Ok(Self {
            timeout: options.timeout,
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
/// The working tree is lent per completion through the guest's
/// `grants.workspace`, which the host resolves to a node-local path on the tool
/// host. Optionally override the per-spawn wall-clock bound via
/// [`Self::timeout`] (default 120s) or `CURSOR_TIMEOUT_SECS` when loading
/// through [`omnia::FromEnv`].
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Wall-clock bound on one `cursor-agent` spawn.
    pub timeout: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }
}

impl omnia::FromEnv for ConnectOptions {
    fn from_env() -> Result<Self> {
        let timeout = match std::env::var(TIMEOUT_SECS_ENV) {
            Ok(raw) if !raw.trim().is_empty() => {
                let secs = raw.trim().parse::<u64>().with_context(|| {
                    format!("{TIMEOUT_SECS_ENV} must be an unsigned integer (seconds), got {raw:?}")
                })?;
                if secs == 0 {
                    bail!("{TIMEOUT_SECS_ENV} must be greater than 0");
                }
                Duration::from_secs(secs)
            }
            _ => Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        };
        Ok(Self { timeout })
    }
}
