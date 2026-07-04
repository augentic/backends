//! Unit-test helpers for the cursor backend crate.

use std::time::Duration;

use omnia_wasi_model::{DirEntry, FutureResult, Reference, ToolHost, VerifyReport};

use crate::Client;

/// Stub host whose every method errors; cursor ignores the lent capabilities.
#[derive(Debug)]
pub struct NoopToolHost;

impl ToolHost for NoopToolHost {
    fn resolve(&self, _reference: Reference) -> FutureResult<Vec<u8>> {
        Box::pin(async { Err(anyhow::anyhow!("cursor ignores the tool host")) })
    }

    fn read(&self, _path: String) -> FutureResult<Vec<u8>> {
        Box::pin(async { Err(anyhow::anyhow!("cursor ignores the tool host")) })
    }

    fn list(&self, _path: String) -> FutureResult<Vec<DirEntry>> {
        Box::pin(async { Err(anyhow::anyhow!("cursor ignores the tool host")) })
    }

    fn write(&self, _path: String, _bytes: Vec<u8>) -> FutureResult<()> {
        Box::pin(async { Err(anyhow::anyhow!("cursor ignores the tool host")) })
    }

    fn verify(&self, _check: String) -> FutureResult<VerifyReport> {
        Box::pin(async { Err(anyhow::anyhow!("cursor ignores the tool host")) })
    }
}

/// Build a [`Client`] directly, bypassing `connect_with` (and its `PATH` check).
pub fn client() -> Client {
    Client {
        timeout: Duration::from_secs(1),
    }
}
