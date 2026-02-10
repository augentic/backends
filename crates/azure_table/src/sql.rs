//! wasi-sql implementation for Azure Table storage

use std::sync::Arc;

use anyhow::{anyhow, bail};
use base64ct::{Base64, Encoding};
use futures::future::FutureExt;
use hmac::{Hmac, Mac};
use qwasr_wasi_sql::{Connection, DataType, Field, FutureResult, Row, WasiSqlCtx};
use reqwest::Client as HttpClient;
use serde_json::Value;
use sha2::Sha256;

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
                bail!("Azure Table query failed: {}", response.error_for_status()
                    .err().map_or_else(|| "unknown error".to_string(), |e| e.to_string()));
            }
            let body: Value =
                response.json().await.map_err(|e| anyhow!("Failed to parse response JSON: {e}"))?;
            parse(&body)
        }
        .boxed()
    }

    fn exec(&self, query: String, params: Vec<DataType>) -> FutureResult<u32> {
        tracing::debug!("exec: {query}, params: {params:?}");
        todo!()
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

fn infer_data_type(value: &Value) -> &'static str {
    match value {
        Value::Bool(_) => "Edm.Boolean",
        Value::Number(n) => {
            if n.is_f64() {
                "Edm.Double"
            } else {
                "Edm.Int32"
            }
        }
        _ => "Edm.String", // fallback for null, array, object, etc.
    }
}

fn parse(val: &Value) -> anyhow::Result<Vec<Row>> {
    let mut rows = Vec::new();
    if let Some(entries) = val.get("value").and_then(|v| v.as_array()) {
        for entry in entries {
            let mut index = String::new();
            let mut fields = Vec::new();
            if let Some(obj) = entry.as_object() {
                for (k, v) in obj {
                    // If the key is an odata property, skip it.
                    if k.starts_with("odata.") {
                        continue;
                    }
                    // If the key is `RowKey`, use it as the row index.
                    if k == "RowKey" {
                        if let Some(s) = v.as_str() {
                            index = s.to_string();
                        }
                        continue;
                    }
                    // If the key is another system one, skip it.
                    if k == "PartitionKey" || k == "Timestamp" {
                        continue;
                    }
                    // If the key contains "@odata.type", skip it (but we will
                    // use it to determine the data type of the corresponding
                    // value).
                    if k.ends_with("@odata.type") {
                        continue;
                    }
                    // Find the corresponding "@odata.type" key if it exists.
                    let type_key = format!("{k}@odata.type");
                    let data_type = obj
                        .get(&type_key)
                        .and_then(|t| t.as_str())
                        .unwrap_or_else(|| infer_data_type(v));
                    let value = convert(v, data_type)?;
                    fields.push(Field {
                        name: k.clone(),
                        value,
                    });
                }
            }
            rows.push(Row { index, fields });
        }
    }
    Ok(rows)
}

fn convert(value: &Value, data_type: &str) -> anyhow::Result<DataType> {
    match data_type {
        "Edm.Binary" => {
            if let Some(s) = value.as_str() {
                let decoded = Base64::decode_vec(s)?;
                Ok(DataType::Binary(Some(decoded)))
            } else {
                Ok(DataType::Binary(None))
            }
        }
        "Edm.Boolean" => value
            .as_bool()
            .map_or_else(|| Ok(DataType::Boolean(None)), |b| Ok(DataType::Boolean(Some(b)))),
        "Edm.DateTime" => value.as_str().map_or_else(
            || Ok(DataType::Timestamp(None)),
            |s| Ok(DataType::Timestamp(Some(s.to_string()))),
        ),
        "Edm.Double" => value
            .as_f64()
            .map_or_else(|| Ok(DataType::Double(None)), |f| Ok(DataType::Double(Some(f)))),
        "Edm.String" | "Edm.Guid" => value
            .as_str()
            .map_or_else(|| Ok(DataType::Str(None)), |s| Ok(DataType::Str(Some(s.to_string())))),
        "Edm.Int32" => value.as_i64().map_or_else(
            || Ok(DataType::Int32(None)),
            |n| {
                i32::try_from(n)
                    .map(|v| DataType::Int32(Some(v)))
                    .map_err(|_e| anyhow!("Value {n} out of range for Int32"))
            },
        ),
        "Edm.Int64" => value
            .as_i64()
            .map_or_else(|| Ok(DataType::Int64(None)), |n| Ok(DataType::Int64(Some(n)))),
        _ => Err(anyhow!("unsupported data type: {data_type}")),
    }
}

#[derive(Debug)]
struct QueryPhrases {
    select: Option<String>,
    filter: Option<String>,
    top: Option<u32>,
}

impl QueryPhrases {
    pub fn query(query: &str, params: &[DataType]) -> anyhow::Result<Self> {
        // Check for unsupported features
        let query_upper = query.to_uppercase();
        if query_upper.contains("JOIN") {
            bail!("JOIN clauses are not supported");
        }
        if query_upper.contains("ORDER BY") {
            bail!("ORDER BY clauses are not supported");
        }

        let mut select = None;
        let mut filter = None;
        let mut top = None;

        // Replace parameters in the query
        let processed_query = Self::substitute_params(query, params)?;

        // Parse the query into sections
        let parts: Vec<&str> = processed_query.split_whitespace().collect();
        let mut i = 0;

        while i < parts.len() {
            let part_upper = parts[i].to_uppercase();

            match part_upper.as_str() {
                "SELECT" => {
                    i += 1;
                    let mut select_items = Vec::new();

                    // Check for TOP clause
                    if i < parts.len() && parts[i].to_uppercase() == "TOP" {
                        i += 1;
                        if i < parts.len() {
                            top = Some(parts[i].parse()?);
                            i += 1;
                        }
                    }

                    // Collect select items until we hit FROM or end
                    while i < parts.len() && parts[i].to_uppercase() != "FROM" {
                        select_items.push(parts[i]);
                        i += 1;
                    }

                    if !select_items.is_empty() {
                        select = Some(select_items.join(" ").trim_end_matches(',').to_string());
                    }
                }
                "FROM" => {
                    // Skip the FROM keyword and table name
                    i += 1;
                    if i < parts.len() {
                        i += 1; // skip table name
                    }
                }
                "WHERE" => {
                    i += 1;
                    let mut filter_parts = Vec::new();

                    // Collect all parts until end of query
                    while i < parts.len() {
                        let part_upper_check = parts[i].to_uppercase();
                        if part_upper_check == "ORDER" || part_upper_check == "JOIN" {
                            break;
                        }
                        filter_parts.push(parts[i]);
                        i += 1;
                    }

                    if !filter_parts.is_empty() {
                        filter = Some(filter_parts.join(" "));
                    }
                }
                _ => {
                    i += 1;
                }
            }
        }

        Ok(Self { select, filter, top })
    }

    pub fn odata(&self) -> String {
        let mut clauses = Vec::new();

        if let Some(select) = &self.select {
            // Don't include $select for SELECT *
            if select != "*" {
                let encoded = urlencoding::encode(select);
                clauses.push(format!("$select={encoded}"));
            }
        }

        if let Some(filter) = &self.filter {
            // Convert SQL operators to OData operators
            let odata_filter = filter
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

            let encoded = urlencoding::encode(&odata_filter);
            clauses.push(format!("$filter={encoded}"));
        }

        if let Some(top) = self.top {
            clauses.push(format!("$top={top}"));
        }

        clauses.join("&")
    }

    fn substitute_params(query: &str, params: &[DataType]) -> anyhow::Result<String> {
        let mut processed_query = query.to_string();
        for (i, param) in params.iter().enumerate() {
            let placeholder = format!("${}", i + 1);
            let value = match param {
                DataType::Int32(v) => {
                    v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string())
                }
                DataType::Int64(v) => {
                    v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string())
                }
                DataType::Uint32(v) => {
                    v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string())
                }
                DataType::Uint64(v) => {
                    v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string())
                }
                DataType::Float(v) => {
                    v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string())
                }
                DataType::Double(v) => {
                    v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string())
                }
                DataType::Str(v) => v
                    .as_ref()
                    .map_or_else(|| "NULL".to_string(), |s| format!("'{}'", s.replace('\'', "''"))),
                DataType::Boolean(v) => {
                    v.map(|b| b.to_string()).unwrap_or_else(|| "NULL".to_string())
                }
                DataType::Date(v) => {
                    v.as_ref().map_or_else(|| "NULL".to_string(), |s| format!("'{s}'"))
                }
                DataType::Time(v) => {
                    v.as_ref().map_or_else(|| "NULL".to_string(), |s| format!("'{s}'"))
                }
                DataType::Timestamp(v) => {
                    v.as_ref().map_or_else(|| "NULL".to_string(), |s| format!("'{s}'"))
                }
                DataType::Binary(_) => {
                    bail!("Binary parameters are not supported in query strings")
                }
            };
            processed_query = processed_query.replace(&placeholder, &value);
        }
        Ok(processed_query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_select() {
        let query = "SELECT * FROM users";
        let result = QueryPhrases::query(query, &[]).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, None);
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_select_with_columns() {
        let query = "SELECT id, name, email FROM users";
        let result = QueryPhrases::query(query, &[]).unwrap();

        assert_eq!(result.select, Some("id, name, email".to_string()));
        assert_eq!(result.filter, None);
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_select_with_where() {
        let query = "SELECT * FROM users WHERE age > 18";
        let result = QueryPhrases::query(query, &[]).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("age > 18".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_select_with_top() {
        let query = "SELECT TOP 10 * FROM users";
        let result = QueryPhrases::query(query, &[]).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, None);
        assert_eq!(result.top, Some(10));
    }

    #[test]
    fn test_select_with_top_and_where() {
        let query = "SELECT TOP 5 name, email FROM users WHERE active = true";
        let result = QueryPhrases::query(query, &[]).unwrap();

        assert_eq!(result.select, Some("name, email".to_string()));
        assert_eq!(result.filter, Some("active = true".to_string()));
        assert_eq!(result.top, Some(5));
    }

    #[test]
    fn test_parameterized_query_with_int() {
        let query = "SELECT * FROM users WHERE id = $1";
        let params = vec![DataType::Int32(Some(42))];
        let result = QueryPhrases::query(query, &params).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("id = 42".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_with_string() {
        let query = "SELECT * FROM users WHERE name = $1";
        let params = vec![DataType::Str(Some("John O'Brien".to_string()))];
        let result = QueryPhrases::query(query, &params).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("name = 'John O''Brien'".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_multiple_params() {
        let query = "SELECT * FROM users WHERE age > $1 AND name = $2";
        let params = vec![DataType::Int32(Some(18)), DataType::Str(Some("Alice".to_string()))];
        let result = QueryPhrases::query(query, &params).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("age > 18 AND name = 'Alice'".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_with_null() {
        let query = "SELECT * FROM users WHERE email = $1";
        let params = vec![DataType::Str(None)];
        let result = QueryPhrases::query(query, &params).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("email = NULL".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_with_boolean() {
        let query = "SELECT * FROM users WHERE active = $1";
        let params = vec![DataType::Boolean(Some(true))];
        let result = QueryPhrases::query(query, &params).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("active = true".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_with_float() {
        let query = "SELECT * FROM products WHERE price > $1";
        let params = vec![DataType::Double(Some(99.99))];
        let result = QueryPhrases::query(query, &params).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("price > 99.99".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_with_date() {
        let query = "SELECT * FROM events WHERE created_at > $1";
        let params = vec![DataType::Date(Some("2026-01-29".to_string()))];
        let result = QueryPhrases::query(query, &params).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("created_at > '2026-01-29'".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_join_returns_error() {
        let query = "SELECT * FROM users JOIN orders ON users.id = orders.user_id";
        let result = QueryPhrases::query(query, &[]);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "JOIN clauses are not supported");
    }

    #[test]
    fn test_order_by_returns_error() {
        let query = "SELECT * FROM users ORDER BY name";
        let result = QueryPhrases::query(query, &[]);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "ORDER BY clauses are not supported");
    }

    #[test]
    fn test_binary_parameter_returns_error() {
        let query = "SELECT * FROM files WHERE data = $1";
        let params = vec![DataType::Binary(Some(vec![1, 2, 3]))];
        let result = QueryPhrases::query(query, &params);

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Binary parameters are not supported in query strings"
        );
    }

    #[test]
    fn test_complex_where_clause() {
        let query = "SELECT name, age FROM users WHERE age >= $1 AND (status = $2 OR role = $3)";
        let params = vec![
            DataType::Int32(Some(21)),
            DataType::Str(Some("active".to_string())),
            DataType::Str(Some("admin".to_string())),
        ];
        let result = QueryPhrases::query(query, &params).unwrap();

        assert_eq!(result.select, Some("name, age".to_string()));
        assert_eq!(
            result.filter,
            Some("age >= 21 AND (status = 'active' OR role = 'admin')".to_string())
        );
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_case_insensitive_keywords() {
        let query = "select * from users where id = $1";
        let params = vec![DataType::Int32(Some(1))];
        let result = QueryPhrases::query(query, &params).unwrap();

        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("id = 1".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_all_numeric_types() {
        let query = "SELECT * FROM data WHERE i32 = $1 AND i64 = $2 AND u32 = $3 AND u64 = $4 AND f32 = $5 AND f64 = $6";
        let params = vec![
            DataType::Int32(Some(100)),
            DataType::Int64(Some(1000)),
            DataType::Uint32(Some(200)),
            DataType::Uint64(Some(2000)),
            DataType::Float(Some(1.5)),
            DataType::Double(Some(99.99)),
        ];
        let result = QueryPhrases::query(query, &params).unwrap();

        assert_eq!(result.filter, Some("i32 = 100 AND i64 = 1000 AND u32 = 200 AND u64 = 2000 AND f32 = 1.5 AND f64 = 99.99".to_string()));
    }

    #[test]
    fn test_to_odata_simple() {
        let phrases = QueryPhrases {
            select: Some("*".to_string()),
            filter: Some("age > 18".to_string()),
            top: Some(10),
        };

        let odata = phrases.odata();
        // SELECT * should not be included in OData
        // Operators should be converted and URL-encoded
        assert_eq!(odata, "$filter=age%20gt%2018&$top=10");
    }

    #[test]
    fn test_to_odata_with_select() {
        let phrases = QueryPhrases {
            select: Some("name, email, age".to_string()),
            filter: None,
            top: None,
        };

        let odata = phrases.odata();
        assert_eq!(odata, "$select=name%2C%20email%2C%20age");
    }

    #[test]
    fn test_to_odata_complex_filter() {
        let phrases = QueryPhrases {
            select: Some("name, age".to_string()),
            filter: Some("age >= 21 AND (status = 'active' OR role = 'admin')".to_string()),
            top: Some(5),
        };

        let odata = phrases.odata();
        assert_eq!(
            odata,
            "$select=name%2C%20age&$filter=age%20ge%2021%20and%20%28status%20eq%20%27active%27%20or%20role%20eq%20%27admin%27%29&$top=5"
        );
    }

    #[test]
    fn test_to_odata_all_operators() {
        let phrases = QueryPhrases {
            select: None,
            filter: Some("a = 1 AND b != 2 AND c > 3 AND d >= 4 AND e < 5 AND f <= 6".to_string()),
            top: None,
        };

        let odata = phrases.odata();
        assert_eq!(
            odata,
            "$filter=a%20eq%201%20and%20b%20ne%202%20and%20c%20gt%203%20and%20d%20ge%204%20and%20e%20lt%205%20and%20f%20le%206"
        );
    }

    #[test]
    fn test_to_odata_url_encoding() {
        let phrases = QueryPhrases {
            select: None,
            filter: Some("name = 'John O''Brien'".to_string()),
            top: None,
        };

        let odata = phrases.odata();
        assert_eq!(odata, "$filter=name%20eq%20%27John%20O%27%27Brien%27");
    }

    #[test]
    fn test_parse_empty_response() {
        let json = serde_json::json!({
            "value": []
        });
        let result = parse(&json).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_parse_single_row_with_string() {
        let json = serde_json::json!({
            "value": [
                {
                    "odata.etag": "W/\"0x5B168C7B6E589D2\"",
                    "PartitionKey": "partition1",
                    "RowKey": "row1",
                    "Timestamp": "2026-01-30T12:00:00Z",
                    "Name": "John Doe",
                    "Name@odata.type": "Edm.String"
                }
            ]
        });

        let result = parse(&json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].index, "row1");
        assert_eq!(result[0].fields.len(), 1);
        assert_eq!(result[0].fields[0].name, "Name");
        match &result[0].fields[0].value {
            DataType::Str(Some(s)) => assert_eq!(s, "John Doe"),
            _ => panic!("Expected Str(Some(\"John Doe\"))"),
        }
    }

    #[test]
    fn test_parse_multiple_data_types() {
        let json = serde_json::json!({
            "value": [
                {
                    "odata.etag": "etag1",
                    "PartitionKey": "p1",
                    "RowKey": "r1",
                    "Timestamp": "2026-01-30T12:00:00Z",
                    "stringField": "test",
                    "stringField@odata.type": "Edm.String",
                    "intField": 42,
                    "intField@odata.type": "Edm.Int32",
                    "longField": 9_223_372_036_854_775_807_i64,
                    "longField@odata.type": "Edm.Int64",
                    "doubleField": 42.5,
                    "doubleField@odata.type": "Edm.Double",
                    "boolField": true,
                    "boolField@odata.type": "Edm.Boolean",
                    "dateField": "2026-01-30T12:00:00Z",
                    "dateField@odata.type": "Edm.DateTime",
                    "binaryField": "SGVsbG8gV29ybGQ=",
                    "binaryField@odata.type": "Edm.Binary",
                    "guidField": "123e4567-e89b-12d3-a456-426614174000",
                    "guidField@odata.type": "Edm.Guid"
                }
            ]
        });

        let result = parse(&json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fields.len(), 8);

        // Check each field by finding it by name (order not guaranteed)
        let fields = &result[0].fields;
        
        let string_field = fields.iter().find(|f| f.name == "stringField").unwrap();
        match &string_field.value {
            DataType::Str(Some(s)) => assert_eq!(s, "test"),
            _ => panic!("Expected Str(Some(\"test\"))"),
        }

        let int_field = fields.iter().find(|f| f.name == "intField").unwrap();
        match &int_field.value {
            DataType::Int32(Some(n)) => assert_eq!(*n, 42),
            _ => panic!("Expected Int32(Some(42))"),
        }

        let long_field = fields.iter().find(|f| f.name == "longField").unwrap();
        match &long_field.value {
            DataType::Int64(Some(n)) => assert_eq!(*n, 9_223_372_036_854_775_807),
            _ => panic!("Expected Int64(Some(9223372036854775807))"),
        }

        let double_field = fields.iter().find(|f| f.name == "doubleField").unwrap();
        match &double_field.value {
            DataType::Double(Some(f)) => assert!((*f - 42.5).abs() < f64::EPSILON),
            _ => panic!("Expected Double(Some(42.5))"),
        }

        let bool_field = fields.iter().find(|f| f.name == "boolField").unwrap();
        match &bool_field.value {
            DataType::Boolean(Some(b)) => assert!(*b),
            _ => panic!("Expected Boolean(Some(true))"),
        }

        let date_field = fields.iter().find(|f| f.name == "dateField").unwrap();
        match &date_field.value {
            DataType::Timestamp(Some(s)) => assert_eq!(s, "2026-01-30T12:00:00Z"),
            _ => panic!("Expected Timestamp(Some(\"2026-01-30T12:00:00Z\"))"),
        }

        let binary_field = fields.iter().find(|f| f.name == "binaryField").unwrap();
        match &binary_field.value {
            DataType::Binary(Some(data)) => assert_eq!(data, b"Hello World"),
            _ => panic!("Expected Binary(Some(b\"Hello World\"))"),
        }

        let guid_field = fields.iter().find(|f| f.name == "guidField").unwrap();
        match &guid_field.value {
            DataType::Str(Some(s)) => assert_eq!(s, "123e4567-e89b-12d3-a456-426614174000"),
            _ => panic!("Expected Str(Some(\"123e4567-e89b-12d3-a456-426614174000\"))"),
        }
    }

    #[test]
    fn test_parse_multiple_rows() {
        let json = serde_json::json!({
            "value": [
                {
                    "odata.etag": "etag1",
                    "PartitionKey": "p1",
                    "RowKey": "r1",
                    "Timestamp": "2026-01-30T12:00:00Z",
                    "name": "Alice",
                    "name@odata.type": "Edm.String",
                    "age": 30,
                    "age@odata.type": "Edm.Int32"
                },
                {
                    "odata.etag": "etag2",
                    "PartitionKey": "p1",
                    "RowKey": "r2",
                    "Timestamp": "2026-01-30T12:00:00Z",
                    "name": "Bob",
                    "name@odata.type": "Edm.String",
                    "age": 25,
                    "age@odata.type": "Edm.Int32"
                }
            ]
        });

        let result = parse(&json).unwrap();
        assert_eq!(result.len(), 2);
        
        assert_eq!(result[0].index, "r1");
        assert_eq!(result[0].fields.len(), 2);
        let name_field = result[0].fields.iter().find(|f| f.name == "name").unwrap();
        match &name_field.value {
            DataType::Str(Some(s)) => assert_eq!(s, "Alice"),
            _ => panic!("Expected Str(Some(\"Alice\"))"),
        }
        let age_field = result[0].fields.iter().find(|f| f.name == "age").unwrap();
        match &age_field.value {
            DataType::Int32(Some(n)) => assert_eq!(*n, 30),
            _ => panic!("Expected Int32(Some(30))"),
        }

        assert_eq!(result[1].index, "r2");
        assert_eq!(result[1].fields.len(), 2);
        let name_field = result[1].fields.iter().find(|f| f.name == "name").unwrap();
        match &name_field.value {
            DataType::Str(Some(s)) => assert_eq!(s, "Bob"),
            _ => panic!("Expected Str(Some(\"Bob\"))"),
        }
        let age_field = result[1].fields.iter().find(|f| f.name == "age").unwrap();
        match &age_field.value {
            DataType::Int32(Some(n)) => assert_eq!(*n, 25),
            _ => panic!("Expected Int32(Some(25))"),
        }
    }

    #[test]
    fn test_parse_missing_odata_type() {
        let json = serde_json::json!({
            "value": [
                {
                    "odata.etag": "etag1",
                    "PartitionKey": "p1",
                    "RowKey": "r1",
                    "Timestamp": "2026-01-30T12:00:00Z",
                    "name": "John Doe"
                    // Missing name@odata.type
                }
            ]
        });

        let result = parse(&json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing @odata.type"));
    }

    #[test]
    fn test_parse_skips_system_fields() {
        let json = serde_json::json!({
            "value": [
                {
                    "odata.etag": "etag1",
                    "odata.metadata": "https://example.table.core.windows.net/$metadata#table",
                    "PartitionKey": "partition1",
                    "RowKey": "row1",
                    "Timestamp": "2026-01-30T12:00:00Z",
                    "customField": "value",
                    "customField@odata.type": "Edm.String"
                }
            ]
        });

        let result = parse(&json).unwrap();
        assert_eq!(result.len(), 1);
        // Should only have customField, not PartitionKey, RowKey, Timestamp, or other odata fields
        assert_eq!(result[0].fields.len(), 1);
        assert_eq!(result[0].fields[0].name, "customField");
    }

    #[test]
    fn test_parse_int32_out_of_range() {
        let json = serde_json::json!({
            "value": [
                {
                    "odata.etag": "etag1",
                    "PartitionKey": "p1",
                    "RowKey": "r1",
                    "Timestamp": "2026-01-30T12:00:00Z",
                    "largeInt": 9_223_372_036_854_775_807_i64,
                    "largeInt@odata.type": "Edm.Int32"
                }
            ]
        });

        let result = parse(&json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("out of range"));
    }

    #[test]
    fn test_parse_unsupported_data_type() {
        let json = serde_json::json!({
            "value": [
                {
                    "odata.etag": "etag1",
                    "PartitionKey": "p1",
                    "RowKey": "r1",
                    "Timestamp": "2026-01-30T12:00:00Z",
                    "unknownField": "value",
                    "unknownField@odata.type": "Edm.Unsupported"
                }
            ]
        });

        let result = parse(&json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unsupported data type"));
    }

    #[test]
    fn test_parse_null_values() {
        let json = serde_json::json!({
            "value": [
                {
                    "odata.etag": "etag1",
                    "PartitionKey": "p1",
                    "RowKey": "r1",
                    "Timestamp": "2026-01-30T12:00:00Z",
                    "nullString": null,
                    "nullString@odata.type": "Edm.String",
                    "nullInt": null,
                    "nullInt@odata.type": "Edm.Int32"
                }
            ]
        });

        let result = parse(&json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fields.len(), 2);
        
        let null_string = result[0].fields.iter().find(|f| f.name == "nullString").unwrap();
        match &null_string.value {
            DataType::Str(None) => {},
            _ => panic!("Expected Str(None), got: {:?}", null_string.value),
        }
        
        let null_int = result[0].fields.iter().find(|f| f.name == "nullInt").unwrap();
        match &null_int.value {
            DataType::Int32(None) => {},
            _ => panic!("Expected Int32(None), got: {:?}", null_int.value),
        }
    }
}
