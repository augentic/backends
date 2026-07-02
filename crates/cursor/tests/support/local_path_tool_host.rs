//! A [`ToolHost`] that lends a fixed node-local workspace path.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::FutureExt as _;
use omnia_wasi_model::{DirEntry, FutureResult, Reference, ToolHost, VerifyReport};

/// Tool host that resolves the lent workspace to a fixed path; cursor ignores
/// every other capability.
#[derive(Debug)]
pub struct LocalPathToolHost {
    path: PathBuf,
}

impl ToolHost for LocalPathToolHost {
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

    fn local_path(&self) -> Option<&Path> {
        Some(&self.path)
    }
}

/// A tool host that lends `path` as the completion's node-local workspace.
pub fn local_path_tool_host(path: PathBuf) -> Arc<dyn ToolHost> {
    Arc::new(LocalPathToolHost { path })
}
