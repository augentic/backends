//! `wasi-jsondb` implementation for Azure Table Storage.

pub mod document;
pub mod filter;
pub mod query;

use anyhow::{anyhow, bail};
use base64ct::{Base64, Encoding};
use futures::future::FutureExt;
use hmac::{Hmac, KeyInit, Mac};
use omnia_wasi_jsondb::{
    Document, FilterTree, FutureResult, QueryOpts, QueryResult, WasiJsonDbCtx,
};
use reqwest::Client as HttpClient;
use serde_json::Value;
use sha2::Sha256;

use crate::Client;

/// `wasi-jsondb` implementation backed by Azure Table Storage REST API.
impl WasiJsonDbCtx for Client {
    fn get(&self, collection: String, id: String) -> FutureResult<Option<Document>> {
        let opts = self.options.clone();
        let http = HttpClient::new();
        async move {
            let (table, pk) = parse_collection(&collection)?;
            let pk = require_pk(&collection, pk.as_ref())?;
            let base = opts.base_url();
            let uri = format!("{base}/{table}(PartitionKey='{pk}',RowKey='{id}')");

            let now = now_rfc1123();
            let auth = auth_header(&opts.name, &opts.key, "GET", "", &now, &uri)?;

            let response = http
                .get(&uri)
                .headers(azure_headers(&now, &auth))
                .send()
                .await
                .map_err(|e| anyhow!("HTTP request error: {e}"))?;

            if response.status().as_u16() == 404 {
                return Ok(None);
            }
            if !response.status().is_success() {
                bail!(
                    "Azure Table get failed ({}): {}",
                    response.status(),
                    response.text().await.unwrap_or_default()
                );
            }

            let entity: Value =
                response.json().await.map_err(|e| anyhow!("failed to parse response JSON: {e}"))?;

            Ok(Some(document::unflatten(&entity)?))
        }
        .boxed()
    }

    fn insert(&self, collection: String, doc: Document) -> FutureResult<()> {
        let opts = self.options.clone();
        let http = HttpClient::new();
        async move {
            let (table, pk) = parse_collection(&collection)?;
            let pk = require_pk(&collection, pk.as_ref())?;
            let base = opts.base_url();
            let uri = format!("{base}/{table}");
            let body = document::flatten(&doc, pk)?;

            let now = now_rfc1123();
            let auth = auth_header(&opts.name, &opts.key, "POST", "application/json", &now, &uri)?;

            let response = http
                .post(&uri)
                .headers(azure_headers(&now, &auth))
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow!("HTTP request error: {e}"))?;

            if !response.status().is_success() {
                bail!(
                    "Azure Table insert failed ({}): {}",
                    response.status(),
                    response.text().await.unwrap_or_default()
                );
            }
            Ok(())
        }
        .boxed()
    }

    fn put(&self, collection: String, doc: Document) -> FutureResult<()> {
        let opts = self.options.clone();
        let http = HttpClient::new();
        async move {
            let (table, pk) = parse_collection(&collection)?;
            let pk = require_pk(&collection, pk.as_ref())?;
            let base = opts.base_url();
            let uri = format!("{base}/{table}(PartitionKey='{pk}',RowKey='{}')", doc.id);
            let body = document::flatten(&doc, pk)?;

            let now = now_rfc1123();
            let auth = auth_header(&opts.name, &opts.key, "PUT", "application/json", &now, &uri)?;

            let response = http
                .put(&uri)
                .headers(azure_headers(&now, &auth))
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow!("HTTP request error: {e}"))?;

            if !response.status().is_success() {
                bail!(
                    "Azure Table put failed ({}): {}",
                    response.status(),
                    response.text().await.unwrap_or_default()
                );
            }
            Ok(())
        }
        .boxed()
    }

    fn delete(&self, collection: String, id: String) -> FutureResult<bool> {
        let opts = self.options.clone();
        let http = HttpClient::new();
        async move {
            let (table, pk) = parse_collection(&collection)?;
            let pk = require_pk(&collection, pk.as_ref())?;
            let base = opts.base_url();
            let uri = format!("{base}/{table}(PartitionKey='{pk}',RowKey='{id}')");

            let now = now_rfc1123();
            let auth = auth_header(&opts.name, &opts.key, "DELETE", "", &now, &uri)?;

            let mut headers = azure_headers(&now, &auth);
            headers.insert("If-Match", "*".parse().expect("valid header value"));

            let response = http
                .delete(&uri)
                .headers(headers)
                .send()
                .await
                .map_err(|e| anyhow!("HTTP request error: {e}"))?;

            if response.status().as_u16() == 404 {
                return Ok(false);
            }
            if !response.status().is_success() {
                bail!(
                    "Azure Table delete failed ({}): {}",
                    response.status(),
                    response.text().await.unwrap_or_default()
                );
            }
            Ok(true)
        }
        .boxed()
    }

    fn query(
        &self, collection: String, filter: Option<FilterTree>, options: QueryOpts,
    ) -> FutureResult<QueryResult> {
        let opts = self.options.clone();
        let http = HttpClient::new();
        async move {
            let (table, pk) = parse_collection(&collection)?;

            let user_filter = filter.as_ref().map(filter::to_odata).transpose()?;
            let odata_filter = build_odata_filter(pk.as_deref(), user_filter.as_deref());

            let mut all_documents: Vec<Document> = Vec::new();
            let mut next_continuation = options.continuation.clone();
            let fetch_limit = options.limit.map(|l| l as usize);
            let server_limit = match (fetch_limit, options.offset.map(|o| o as usize)) {
                (Some(l), Some(o)) => Some(l.saturating_add(o)),
                (lim, _) => lim,
            };

            loop {
                let (body, continuation) = fetch_page(
                    &http,
                    &opts,
                    &table,
                    odata_filter.as_deref(),
                    server_limit,
                    next_continuation.as_deref(),
                )
                .await?;

                if let Some(entries) = body.get("value").and_then(Value::as_array) {
                    for entity in entries {
                        all_documents.push(document::unflatten(entity)?);
                    }
                }

                let has_more_pages = continuation.is_some();
                next_continuation = continuation;

                let reached_limit = server_limit.is_some_and(|lim| all_documents.len() >= lim);

                if !has_more_pages || reached_limit {
                    break;
                }
            }

            if let Some(offset) = options.offset {
                all_documents = query::apply_offset(all_documents, offset);
            }

            if let Some(lim) = fetch_limit {
                all_documents.truncate(lim);
            }

            Ok(QueryResult {
                documents: all_documents,
                continuation: next_continuation,
            })
        }
        .boxed()
    }
}

/// Azure Table Storage management operations (outside the `wasi-jsondb` trait).
impl Client {
    /// Creates the named table if it does not already exist.
    ///
    /// Returns `true` when the table was created, `false` when it already
    /// existed.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the server responds with
    /// an unexpected status code.
    pub async fn ensure_table(&self, table: &str) -> anyhow::Result<bool> {
        let base = self.options.base_url();
        let uri = format!("{base}/Tables");
        let now = now_rfc1123();
        let auth = auth_header(
            &self.options.name,
            &self.options.key,
            "POST",
            "application/json",
            &now,
            &uri,
        )?;

        let response = HttpClient::new()
            .post(&uri)
            .headers(azure_headers(&now, &auth))
            .json(&serde_json::json!({"TableName": table}))
            .send()
            .await
            .map_err(|e| anyhow!("create table request: {e}"))?;

        match response.status().as_u16() {
            201 | 204 => Ok(true),
            409 => Ok(false),
            _ => {
                bail!(
                    "create table failed ({}): {}",
                    response.status(),
                    response.text().await.unwrap_or_default()
                );
            }
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────

fn require_pk<'a>(collection: &str, pk: Option<&'a String>) -> anyhow::Result<&'a str> {
    pk.map(String::as_str).ok_or_else(|| {
        anyhow!(
            "operation requires collection format '{{table}}/{{partitionKey}}', got '{collection}'"
        )
    })
}

fn build_odata_filter(pk: Option<&str>, server_filter: Option<&str>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(pk) = pk {
        parts.push(format!("PartitionKey eq '{}'", pk.replace('\'', "''")));
    }
    if let Some(sf) = server_filter {
        parts.push(sf.to_owned());
    }
    if parts.is_empty() { None } else { Some(parts.join(" and ")) }
}

#[allow(clippy::similar_names)]
async fn fetch_page(
    http: &HttpClient, opts: &crate::ConnectOptions, table: &str, odata_filter: Option<&str>,
    fetch_limit: Option<usize>, continuation: Option<&str>,
) -> anyhow::Result<(Value, Option<String>)> {
    let base = opts.base_url();
    let base_uri = format!("{base}/{table}()");

    let mut query_params: Vec<String> = Vec::new();
    if let Some(f) = odata_filter {
        query_params.push(format!("$filter={}", urlencoding::encode(f)));
    }
    if let Some(limit) = fetch_limit {
        query_params.push(format!("$top={limit}"));
    }
    if let Some(cont) = continuation {
        let (next_pk, next_rk) = query::decode_continuation(cont);
        query_params.push(format!("NextPartitionKey={}", urlencoding::encode(&next_pk)));
        if let Some(rk) = next_rk {
            query_params.push(format!("NextRowKey={}", urlencoding::encode(&rk)));
        }
    }

    let uri = if query_params.is_empty() {
        base_uri
    } else {
        format!("{base_uri}?{}", query_params.join("&"))
    };

    let now = now_rfc1123();
    let auth = auth_header(&opts.name, &opts.key, "GET", "", &now, &uri)?;

    let response = http
        .get(&uri)
        .headers(azure_headers(&now, &auth))
        .send()
        .await
        .map_err(|e| anyhow!("HTTP request error: {e}"))?;

    if !response.status().is_success() {
        bail!(
            "Azure Table query failed ({}): {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }

    let continuation_pk = response
        .headers()
        .get("x-ms-continuation-NextPartitionKey")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let continuation_rk = response
        .headers()
        .get("x-ms-continuation-NextRowKey")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let body: Value =
        response.json().await.map_err(|e| anyhow!("failed to parse response JSON: {e}"))?;

    let token = query::encode_continuation(continuation_pk.as_deref(), continuation_rk.as_deref());
    Ok((body, token))
}

/// Split `collection` on the first `/` into `(table, partition_key)`.
fn parse_collection(collection: &str) -> anyhow::Result<(String, Option<String>)> {
    match collection.split_once('/') {
        Some((table, pk)) if !table.is_empty() && !pk.is_empty() => {
            Ok((table.to_owned(), Some(pk.to_owned())))
        }
        Some((table, _)) if !table.is_empty() => {
            bail!("collection '{collection}' has an empty partition key after '/'")
        }
        Some(_) => bail!("collection '{collection}' has an empty table name"),
        None if !collection.is_empty() => Ok((collection.to_owned(), None)),
        _ => bail!("collection must not be empty"),
    }
}

fn now_rfc1123() -> String {
    chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn auth_header(
    account_name: &str, account_key: &str, method: &str, content_type: &str, date_time: &str,
    uri: &str,
) -> anyhow::Result<String> {
    let uri_path = uri
        .split("://")
        .nth(1)
        .and_then(|after_scheme| after_scheme.find('/').map(|i| &after_scheme[i..]))
        .unwrap_or("/");
    let uri_path = uri_path.split('?').next().unwrap_or(uri_path);
    let resource = format!("/{account_name}{uri_path}");
    let string_to_sign = format!("{method}\n\n{content_type}\n{date_time}\n{resource}");
    let key_bytes = Base64::decode_vec(account_key)?;
    let mut hmac = Hmac::<Sha256>::new_from_slice(&key_bytes)
        .map_err(|e| anyhow!("HMAC initialization error: {e}"))?;
    hmac.update(string_to_sign.as_bytes());
    let signature = hmac.finalize().into_bytes();
    let encoded = Base64::encode_string(&signature);
    Ok(format!("SharedKey {account_name}:{encoded}"))
}

fn azure_headers(date: &str, auth: &str) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert("x-ms-date", date.parse().expect("valid header value"));
    h.insert("x-ms-version", "2026-02-06".parse().expect("valid header value"));
    h.insert("Authorization", auth.parse().expect("valid header value"));
    h.insert("Accept", "application/json;odata=fullmetadata".parse().expect("valid header value"));
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_collection_full() {
        let (table, pk) = parse_collection("users/tenant-a").unwrap();
        assert_eq!(table, "users");
        assert_eq!(pk.as_deref(), Some("tenant-a"));
    }

    #[test]
    fn parse_collection_table_only() {
        let (table, pk) = parse_collection("users").unwrap();
        assert_eq!(table, "users");
        assert!(pk.is_none());
    }

    #[test]
    fn parse_collection_empty_errors() {
        parse_collection("").unwrap_err();
    }

    #[test]
    fn parse_collection_empty_pk_errors() {
        parse_collection("users/").unwrap_err();
    }
}
