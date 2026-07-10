//! Live blob round-trip for the Azure Blob Storage backend, driven through the
//! `omnia:blobstore` host boundary (`WasiBlobstoreCtx`).
//!
//! `#[ignore]`d so it never touches the network in CI. Run against a real
//! storage account (`AZURE_BLOB_ENDPOINT` plus credentials, or Azurite):
//! `cargo nextest run -p omnia-azure-blob --run-ignored all`.

use anyhow::Result;
use omnia::Backend;
use omnia_azure_blob::Client;
use omnia_wasi_blobstore::{Container, WasiBlobstoreCtx};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: needs an Azure Blob endpoint (AZURE_BLOB_ENDPOINT); run with --run-ignored"]
async fn write_read_delete() -> Result<()> {
    let client = <Client as Backend>::connect().await?;

    // Container names are lowercase alphanumeric + dashes, 3-63 chars.
    let container = format!("omnia-live-{}", std::process::id());
    let store: std::sync::Arc<dyn Container> = client.create_container(container.clone()).await?;

    let object = "greeting".to_owned();
    store.write_data(object.clone(), b"payload".to_vec().into()).await?;

    // (0, 0) is a full read (see `range_options_full_read_zero_zero`).
    assert_eq!(store.get_data(object.clone(), 0, 0).await?.as_deref(), Some(b"payload".as_slice()));
    assert!(store.has_object(object.clone()).await?, "object exists after write");

    // Exercise the real list/metadata mappings against the service (these
    // replace the deleted unit tests that asserted against reimplemented helpers).
    let names = store.list_objects().await?;
    assert!(names.contains(&object), "written object appears in listing: {names:?}");
    let info = store.object_info(object.clone()).await?;
    assert_eq!(info.name, object, "object name maps through get-properties");
    assert_eq!(info.size, b"payload".len() as u64, "content length maps through get-properties");

    store.delete_object(object).await?;
    client.delete_container(container).await?;
    Ok(())
}
