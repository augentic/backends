use std::env;

use dotenvy::dotenv;
use omnia::Backend;
use omnia_azure_blob::{Client, ConnectOptions, CredentialOptions};
use omnia_wasi_blobstore::WasiBlobstoreCtx;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const CONTAINER: &str = "testaugenticblob";

#[tokio::main]
pub async fn main() {
    dotenv().expect("Failed to load .env file");
    tracing_subscriber::registry().with(fmt::layer()).with(EnvFilter::from_default_env()).init();

    tracing::info!("Azure Blob Storage backend desk-test");

    let endpoint =
        env::var("AZURE_BLOB_ENDPOINT").expect("Set AZURE_BLOB_ENDPOINT env variable");

    let credential = match (
        env::var("AZURE_TENANT_ID").ok(),
        env::var("AZURE_CLIENT_ID").ok(),
        env::var("AZURE_CLIENT_SECRET").ok(),
    ) {
        (Some(tenant_id), Some(client_id), Some(client_secret)) => Some(CredentialOptions {
            tenant_id,
            client_id,
            client_secret,
        }),
        _ => None,
    };

    let cnn_opts = ConnectOptions {
        endpoint,
        credential,
    };

    let client = Client::connect_with(cnn_opts).await.expect("Failed to connect");

    // --- create container ---
    tracing::info!("creating container: {CONTAINER}");
    let container = client
        .create_container(CONTAINER.to_string())
        .await
        .expect("Failed to create container");

    // --- container_exists ---
    let exists = client
        .container_exists(CONTAINER.to_string())
        .await
        .expect("Failed to check container existence");
    tracing::info!("container exists: {exists}");

    // --- write blobs ---
    tracing::info!("writing greeting.txt");
    container
        .write_data("greeting.txt".to_string(), b"hello world".to_vec())
        .await
        .expect("Failed to write greeting.txt");

    tracing::info!("writing data.json");
    container
        .write_data(
            "data.json".to_string(),
            br#"{"name":"Alice","age":30}"#.to_vec(),
        )
        .await
        .expect("Failed to write data.json");

    // --- get_data ---
    let data = container
        .get_data("greeting.txt".to_string(), 0, 0)
        .await
        .expect("Failed to get greeting.txt");
    tracing::info!(
        "greeting.txt content: {:?}",
        data.map(|d| String::from_utf8_lossy(&d).to_string())
    );

    // --- list_objects ---
    let names = container.list_objects().await.expect("Failed to list objects");
    tracing::info!("blobs in container: {names:?}");

    // --- has_object ---
    let has = container
        .has_object("greeting.txt".to_string())
        .await
        .expect("Failed to check object existence");
    tracing::info!("has greeting.txt: {has}");

    // --- object_info ---
    let info = container
        .object_info("greeting.txt".to_string())
        .await
        .expect("Failed to get object info");
    tracing::info!(
        "greeting.txt info: name={}, size={}, created_at={}",
        info.name,
        info.size,
        info.created_at
    );

    // --- delete_object ---
    tracing::info!("deleting greeting.txt");
    container
        .delete_object("greeting.txt".to_string())
        .await
        .expect("Failed to delete greeting.txt");

    let has = container
        .has_object("greeting.txt".to_string())
        .await
        .expect("Failed to check object existence");
    tracing::info!("has greeting.txt after delete: {has}");

    // --- cleanup: delete remaining blob and container ---
    tracing::info!("cleaning up data.json");
    container
        .delete_object("data.json".to_string())
        .await
        .expect("Failed to delete data.json");

    tracing::info!("deleting container: {CONTAINER}");
    client
        .delete_container(CONTAINER.to_string())
        .await
        .expect("Failed to delete container");

    tracing::info!("done");
}
