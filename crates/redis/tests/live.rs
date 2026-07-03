//! Live key-value round-trip for the Redis backend, driven through the
//! `omnia:keyvalue` host boundary (`WasiKeyValueCtx`).
//!
//! `#[ignore]`d so it never touches the network in CI. Run against a reachable
//! Redis (`REDIS_URL`, default `redis://localhost:6379`):
//! `cargo nextest run -p omnia-redis --run-ignored all`.

use anyhow::Result;
use omnia::Backend;
use omnia_redis::Client;
use omnia_wasi_keyvalue::{Bucket, WasiKeyValueCtx};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: needs a reachable Redis; run with --run-ignored"]
async fn set_get_delete() -> Result<()> {
    let client = <Client as Backend>::connect().await?;
    let bucket: std::sync::Arc<dyn Bucket> = client.open_bucket("omnia-live".to_owned()).await?;

    let key = unique("k");
    bucket.set(key.clone(), b"payload".to_vec()).await?;
    assert_eq!(bucket.get(key.clone()).await?.as_deref(), Some(b"payload".as_slice()));
    assert!(bucket.exists(key.clone()).await?, "key exists after set");

    bucket.delete(key.clone()).await?;
    assert!(!bucket.exists(key).await?, "key gone after delete");
    Ok(())
}

/// A collision-resistant suffix so parallel runs never share a live key.
fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{prefix}-{}-{nanos}", std::process::id())
}
