//! Live publish for the NATS backend, driven through the `omnia:messaging` host
//! boundary (`WasiMessagingCtx` + the `Client` producer proxy). NATS also serves
//! `wasi:keyvalue` and `wasi:blobstore` from the same client; a dedicated
//! ignored test per surface can be added here as those live envs are set up.
//!
//! `#[ignore]`d so it never touches the network in CI. Run against a reachable
//! server (`NATS_ADDR`, default `demo.nats.io`):
//! `cargo nextest run -p omnia-nats --run-ignored all`.

use std::sync::Arc;

use anyhow::Result;
use omnia::Backend;
use omnia_nats::Client;
use omnia_wasi_messaging::{Client as MessagingClient, Message, WasiMessagingCtx};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: needs a reachable NATS server (NATS_ADDR); run with --run-ignored"]
async fn publishes_message() -> Result<()> {
    let backend = <Client as Backend>::connect().await?;
    let producer: Arc<dyn MessagingClient> = WasiMessagingCtx::connect(&backend).await?;

    producer.send("omnia.live".to_owned(), Message::new(b"omnia-live".to_vec())).await?;
    Ok(())
}
