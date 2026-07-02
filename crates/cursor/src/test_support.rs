//! Unit-test helpers for the cursor backend crate.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt as _;
use omnia_wasi_model::{DirEntry, FutureResult, Reference, ToolHost, VerifyReport};

use crate::Client;

/// Stub host whose every method errors; cursor ignores the lent capabilities.
#[derive(Debug)]
pub struct NoopToolHost;

impl ToolHost for NoopToolHost {
    fn resolve(&self, _reference: Reference) -> FutureResult<Vec<u8>> {
        async { Err(anyhow::anyhow!("cursor ignores the tool host")) }.boxed()
    }

    fn read(&self, _path: String) -> FutureResult<Vec<u8>> {
        async { Err(anyhow::anyhow!("cursor ignores the tool host")) }.boxed()
    }

    fn list(&self, _path: String) -> FutureResult<Vec<DirEntry>> {
        async { Err(anyhow::anyhow!("cursor ignores the tool host")) }.boxed()
    }

    fn write(&self, _path: String, _bytes: Vec<u8>) -> FutureResult<()> {
        async { Err(anyhow::anyhow!("cursor ignores the tool host")) }.boxed()
    }

    fn verify(&self, _check: String) -> FutureResult<VerifyReport> {
        async { Err(anyhow::anyhow!("cursor ignores the tool host")) }.boxed()
    }
}

/// Build a [`Client`] directly, bypassing `connect_with` (and its `PATH` check).
pub fn client(workspace: Option<&Path>) -> Client {
    Client {
        workspace: workspace.map(|path| Arc::from(path.to_path_buf())),
        timeout: Duration::from_secs(1),
    }
}
