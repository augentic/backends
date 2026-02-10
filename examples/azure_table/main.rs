use std::env;

use dotenvy::dotenv;
use qwasr::Backend;
use qwasr_azure_table::{Client, ConnectOptions};
use qwasr_wasi_sql::{DataType, WasiSqlCtx};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
pub async fn main() {
    dotenv().expect("Failed to load .env file");
    tracing_subscriber::registry().with(fmt::layer()).with(EnvFilter::from_default_env()).init();

    tracing::debug!("Sample Azure Table Storage backend.");

    let account_name =
        env::var("AZURE_STORAGE_ACCOUNT").expect("Set AZURE_STORAGE_ACCOUNT env variable");
    let access_key = env::var("AZURE_STORAGE_KEY").expect("Set AZURE_STORAGE_KEY env variable");

    let cnn_opts = ConnectOptions {
        name: account_name,
        key: access_key,
    };

    let client = Client::connect_with(cnn_opts).await.expect("Failed to set connection options");

    let cnn = client.open("testAugenticBE".to_string()).await.expect("Failed configure connection");
    let query = "SELECT * from testAugenticBE".to_string();
    let rows = cnn.query(query, Vec::new()).await.expect("Query execution failed");
    tracing::debug!("All rows:");
    for row in rows {
        tracing::debug!("{row:?}");
    }

    let query = "SELECT TOP 1 * FROM testAugenticBE WHERE IsActive = $1".to_string();
    let params = vec![DataType::Boolean(Some(true))];
    let rows = cnn.query(query, params).await.expect("Query execution failed");
    tracing::debug!("First active row:");
    for row in rows {
        tracing::debug!("{row:?}");
    }
}
