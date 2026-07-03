//! Live publish for the Kafka backend, driven through the `omnia:messaging`
//! host boundary (`WasiMessagingCtx` + the `Client` producer proxy).
//!
//! `#[ignore]`d so it never touches the network in CI. Run against a reachable
//! broker (`KAFKA_BROKERS`, `COMPONENT`, and any SASL env):
//! `cargo nextest run -p omnia-kafka --run-ignored all`.

use std::sync::Arc;

use anyhow::Result;
use omnia::Backend;
use omnia_kafka::Client;
use omnia_wasi_messaging::{Client as MessagingClient, MessageProxy, WasiMessagingCtx};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: needs a reachable Kafka broker (KAFKA_BROKERS); run with --run-ignored"]
async fn publishes_message() -> Result<()> {
    let backend = <Client as Backend>::connect().await?;
    let producer: Arc<dyn MessagingClient> = WasiMessagingCtx::connect(&backend).await?;

    let message = backend.new_message(b"omnia-live".to_vec())?;
    producer.send("omnia.live".to_owned(), MessageProxy(message)).await?;
    Ok(())
}
