//! wasi-sql implementation for Azure Table storage

mod exec;
mod query;
use std::sync::Arc;

use anyhow::{anyhow, bail};
use base64ct::{Base64, Encoding};
use futures::future::FutureExt;
use hmac::{Hmac, Mac};
use query::QueryPhrases;
use qwasr_wasi_sql::{Connection, DataType, FutureResult, Row, WasiSqlCtx};
use reqwest::Client as HttpClient;
use serde_json::Value;
use sha2::Sha256;

use crate::sql::exec::{ExecAction, ExecPhrase};
use crate::sql::query::parse;
use crate::{Client, ConnectOptions};

impl WasiSqlCtx for Client {
    fn open(&self, name: String) -> FutureResult<Arc<dyn Connection>> {
        tracing::debug!("opening connection to azure storage table {name}");

        let connection = AzTableConnection {
            http_client: HttpClient::new(),
            config: self.options.clone(),
            table: name,
        };
        async move { Ok(Arc::new(connection) as Arc<dyn Connection>) }.boxed()
    }
}

#[derive(Debug)]
pub struct AzTableConnection {
    pub http_client: HttpClient,
    pub config: ConnectOptions,
    pub table: String,
}

impl Connection for AzTableConnection {
    fn query(&self, query: String, params: Vec<DataType>) -> FutureResult<Vec<Row>> {
        tracing::debug!("query: {query}, params: {params:?}");
        let uri = format!("https://{}.table.core.windows.net/{}()", self.config.name, self.table);
        let now = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let resource_path = format!("/{}/{}()", self.config.name, self.table);
        let client = self.http_client.clone();
        let account_name = self.config.name.clone();
        let account_key = self.config.key.clone();
        async move {
            let auth = auth_header(&account_name, &account_key, &now, &resource_path)?;
            let odata_query = QueryPhrases::query(&query, &params)?.odata();
            let full_uri =
                if odata_query.is_empty() { uri } else { format!("{uri}?{odata_query}") };
            let response = client
                .get(&full_uri)
                .header("x-ms-date", now)
                .header("x-ms-version", "2026-02-06")
                .header("Authorization", auth)
                .header("Accept", "application/json;odata=fullmetadata")
                .send()
                .await
                .map_err(|e| anyhow!("HTTP request error: {e}"))?;
            if !response.status().is_success() {
                bail!(
                    "Azure Table query failed: {}",
                    response
                        .error_for_status()
                        .err()
                        .map_or_else(|| "unknown error".to_string(), |e| e.to_string())
                );
            }
            let body: Value =
                response.json().await.map_err(|e| anyhow!("Failed to parse response JSON: {e}"))?;
            parse(&body)
        }
        .boxed()
    }

    fn exec(&self, query: String, params: Vec<DataType>) -> FutureResult<u32> {
        tracing::debug!("exec: {query}, params: {params:?}");
        // Copy self fields for use in async block
        let account_name = self.config.name.clone();
        let account_key = self.config.key.clone();
        let table = self.table.clone();
        let client = self.http_client.clone();
        async move {
            let phrase = ExecPhrase::parse(&query, &params)?;

            let (resource_path, uri) = match &phrase.action {
                ExecAction::Insert => (format!("/{account_name}/{table}"),
                    format!("https://{account_name}.table.core.windows.net/{table}")),
                ExecAction::Update | ExecAction::Delete => (format!("/{account_name}/{table}(PartitionKey='{}',RowKey='{}')", phrase.partition, phrase.row),
                    format!("https://{account_name}.table.core.windows.net/{table}(PartitionKey='{}',RowKey='{}')", phrase.partition, phrase.row)),
            };
            tracing::debug!("resource path: {resource_path}");

            let now = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
            let response = match &phrase.action {
                ExecAction::Insert => {
                    client.post(&uri)
                        .header("x-ms-date", &now)
                        .header("x-ms-version", "2026-02-06")
                        .header("Authorization", auth_header(&account_name, &account_key, &now, &resource_path)?)
                        .header("Accept", "application/json;odata=fullmetadata")
                        .json(&phrase.entity_to_json()?)
                        .send()
                        .await
                        .map_err(|e| anyhow!("HTTP request error: {e}"))?
                }
                ExecAction::Update => {
                    client.put(&uri)
                        .header("x-ms-date", &now)
                        .header("x-ms-version", "2026-02-06")
                        .header("Authorization", auth_header(&account_name, &account_key, &now, &resource_path)?)
                        .header("Accept", "application/json;odata=fullmetadata")
                        .json(&phrase.entity_to_json()?)
                        .send()
                        .await
                        .map_err(|e| anyhow!("HTTP request error: {e}"))?
                }
                ExecAction::Delete => {
                    client.delete(&uri)
                        .header("x-ms-date", &now)
                        .header("x-ms-version", "2026-02-06")
                        .header("Authorization", auth_header(&account_name, &account_key, &now, &resource_path)?)
                        .header("Accept", "application/json;odata=fullmetadata")
                        .send()
                        .await
                        .map_err(|e| anyhow!("HTTP request error: {e}"))?
                }
            };
            if !response.status().is_success() {
                bail!(
                    "Azure Table exec failed: {}",
                    response
                        .error_for_status()
                        .err()
                        .map_or_else(|| "unknown error".to_string(), |e| e.to_string())
                );
            }
            // Only single-entity operations are supported, so we return 1 if
            // the query is valid.
            Ok(1)
        }
        .boxed()
    }
}

fn auth_header(
    account_name: &str, account_key: &str, date_time: &str, resource_path: &str,
) -> anyhow::Result<String> {
    // String to sign for SharedKey Lite:
    // Date + "\n" + CanonicalizedResource
    let string_to_sign = format!("{date_time}\n{resource_path}");

    let key_bytes = Base64::decode_vec(account_key)?;
    let mut hmac = Hmac::<Sha256>::new_from_slice(&key_bytes)
        .map_err(|e| anyhow!("HMAC initialization error: {e}"))?;
    hmac.update(string_to_sign.as_bytes());
    let signature = hmac.finalize().into_bytes();
    let encoded = Base64::encode_string(&signature);
    Ok(format!("SharedKeyLite {account_name}:{encoded}"))
}

/// Convert SQL filter operators to `OData` operators.
fn sql_to_odata_filter(sql_filter: &str) -> String {
    let filter = sql_filter
        .replace(" = ", " eq ")
        .replace(" != ", " ne ")
        .replace(" <> ", " ne ")
        .replace(" > ", " gt ")
        .replace(" >= ", " ge ")
        .replace(" < ", " lt ")
        .replace(" <= ", " le ")
        .replace(" AND ", " and ")
        .replace(" OR ", " or ")
        .replace(" NOT ", " not ");
    let encoded = urlencoding::encode(&filter);
    encoded.into_owned()
}
