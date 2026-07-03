//! Live document round-trip for the Azure Table Storage backend, driven through
//! the `omnia:docstore` host boundary (`WasiDocStoreCtx`).
//!
//! `#[ignore]`d so it never touches the network in CI. Run against a real
//! storage account (`AZURE_STORAGE_ACCOUNT` + `AZURE_STORAGE_KEY`, or Azurite):
//! `cargo nextest run -p omnia-azure-table --run-ignored all`.

use anyhow::Result;
use omnia::Backend;
use omnia_azure_table::Client;
use omnia_wasi_docstore::{Document, WasiDocStoreCtx};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: needs Azure Table Storage (AZURE_STORAGE_ACCOUNT/KEY); run with --run-ignored"]
async fn insert_get_delete() -> Result<()> {
    let client = <Client as Backend>::connect().await?;

    let collection = "omnialive".to_owned();
    // Composite id is `{PartitionKey}\0{RowKey}` (see `document::encode_id`).
    let id = format!("live\u{0}row-{}", std::process::id());
    let doc = Document { id: id.clone(), data: br#"{"hello":"world"}"#.to_vec() };

    client.insert(collection.clone(), doc).await?;
    let got = client.get(collection.clone(), id.clone()).await?.expect("document present");
    assert_eq!(got.id, id, "id round-trips through the boundary");

    client.delete(collection, id).await?;
    Ok(())
}
