//! A no-op [`ToolHost`] for cursor backend tests — the backend owns its own loop.

use std::sync::Arc;

use futures::FutureExt as _;
use omnia_wasi_model::{DirEntry, FutureResult, Reference, ToolHost, VerifyReport};

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

/// Convenience wrapper for tests that need an `Arc<dyn ToolHost>`.
pub fn noop_tool_host() -> Arc<dyn ToolHost> {
    Arc::new(NoopToolHost)
}
