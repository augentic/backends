//! wasi-sql implementation for Azure Table storage

use std::sync::Arc;

use anyhow::{anyhow, bail};
use base64ct::{Base64, Encoding};
use futures::future::FutureExt;
use hmac::{Hmac, Mac};
use reqwest::Client as HttpClient;
use qwasr_wasi_sql::{Connection, DataType, FutureResult, Row, WasiSqlCtx};
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
        async move {
            Ok(Arc::new(connection) as Arc<dyn Connection>)
        }.boxed()
    }
}

#[derive(Debug)]
pub struct AzTableConnection{
    pub http_client: HttpClient,
    pub config: ConnectOptions,
    pub table: String,
}

impl Connection for AzTableConnection {
    fn query(&self, query: String, params: Vec<DataType>) -> FutureResult<Vec<Row>> {
        tracing::debug!("query: {query}, params: {params:?}");
            let uri = format!("https://{}.table.core.windows.net/{}()", 
                self.config.name, 
                self.table);
            let now = chrono::Utc::now().to_rfc2822();
            let auth = format!("SharedKey {}:{}", self.config.name, self.config.key);
        async move {
            let odata_query = QueryPhrases::from_query(&query, &params)?.to_odata();

            todo!()
        }.boxed()
    }

    fn exec(&self, query: String, params: Vec<DataType>) -> FutureResult<u32> {
        tracing::debug!("exec: {query}, params: {params:?}");
        todo!()
    }
}

fn auth_header(account_name: &str, account_key: &str, date_time: &str, resource_path: &str) -> anyhow::Result<String> {
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

#[derive(Debug)]
pub struct QueryPhrases {
    select: Option<String>,
    filter: Option<String>,
    top: Option<u32>,
}

impl QueryPhrases {
    pub fn from_query(query: &str, params: &[DataType]) -> anyhow::Result<Self> {
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

        Ok(Self {
            select,
            filter,
            top,
        })
    }

    pub fn to_odata(&self) -> String {
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
                DataType::Int32(v) => v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string()),
                DataType::Int64(v) => v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string()),
                DataType::Uint32(v) => v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string()),
                DataType::Uint64(v) => v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string()),
                DataType::Float(v) => v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string()),
                DataType::Double(v) => v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".to_string()),
                DataType::Str(v) => v.as_ref().map_or_else(|| "NULL".to_string(), |s| format!("'{}'", s.replace('\'', "''"))),
                DataType::Boolean(v) => v.map(|b| b.to_string()).unwrap_or_else(|| "NULL".to_string()),
                DataType::Date(v) => v.as_ref().map_or_else(|| "NULL".to_string(), |s| format!("'{s}'")),
                DataType::Time(v) => v.as_ref().map_or_else(|| "NULL".to_string(), |s| format!("'{s}'")),
                DataType::Timestamp(v) => v.as_ref().map_or_else(|| "NULL".to_string(), |s| format!("'{s}'")),
                DataType::Binary(_) => bail!("Binary parameters are not supported in query strings"),
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
        let result = QueryPhrases::from_query(query, &[]).unwrap();
        
        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, None);
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_select_with_columns() {
        let query = "SELECT id, name, email FROM users";
        let result = QueryPhrases::from_query(query, &[]).unwrap();
        
        assert_eq!(result.select, Some("id, name, email".to_string()));
        assert_eq!(result.filter, None);
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_select_with_where() {
        let query = "SELECT * FROM users WHERE age > 18";
        let result = QueryPhrases::from_query(query, &[]).unwrap();
        
        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("age > 18".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_select_with_top() {
        let query = "SELECT TOP 10 * FROM users";
        let result = QueryPhrases::from_query(query, &[]).unwrap();
        
        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, None);
        assert_eq!(result.top, Some(10));
    }

    #[test]
    fn test_select_with_top_and_where() {
        let query = "SELECT TOP 5 name, email FROM users WHERE active = true";
        let result = QueryPhrases::from_query(query, &[]).unwrap();
        
        assert_eq!(result.select, Some("name, email".to_string()));
        assert_eq!(result.filter, Some("active = true".to_string()));
        assert_eq!(result.top, Some(5));
    }

    #[test]
    fn test_parameterized_query_with_int() {
        let query = "SELECT * FROM users WHERE id = $1";
        let params = vec![DataType::Int32(Some(42))];
        let result = QueryPhrases::from_query(query, &params).unwrap();
        
        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("id = 42".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_with_string() {
        let query = "SELECT * FROM users WHERE name = $1";
        let params = vec![DataType::Str(Some("John O'Brien".to_string()))];
        let result = QueryPhrases::from_query(query, &params).unwrap();
        
        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("name = 'John O''Brien'".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_multiple_params() {
        let query = "SELECT * FROM users WHERE age > $1 AND name = $2";
        let params = vec![
            DataType::Int32(Some(18)),
            DataType::Str(Some("Alice".to_string())),
        ];
        let result = QueryPhrases::from_query(query, &params).unwrap();
        
        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("age > 18 AND name = 'Alice'".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_with_null() {
        let query = "SELECT * FROM users WHERE email = $1";
        let params = vec![DataType::Str(None)];
        let result = QueryPhrases::from_query(query, &params).unwrap();
        
        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("email = NULL".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_with_boolean() {
        let query = "SELECT * FROM users WHERE active = $1";
        let params = vec![DataType::Boolean(Some(true))];
        let result = QueryPhrases::from_query(query, &params).unwrap();
        
        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("active = true".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_with_float() {
        let query = "SELECT * FROM products WHERE price > $1";
        let params = vec![DataType::Double(Some(99.99))];
        let result = QueryPhrases::from_query(query, &params).unwrap();
        
        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("price > 99.99".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_parameterized_query_with_date() {
        let query = "SELECT * FROM events WHERE created_at > $1";
        let params = vec![DataType::Date(Some("2026-01-29".to_string()))];
        let result = QueryPhrases::from_query(query, &params).unwrap();
        
        assert_eq!(result.select, Some("*".to_string()));
        assert_eq!(result.filter, Some("created_at > '2026-01-29'".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_join_returns_error() {
        let query = "SELECT * FROM users JOIN orders ON users.id = orders.user_id";
        let result = QueryPhrases::from_query(query, &[]);
        
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "JOIN clauses are not supported");
    }

    #[test]
    fn test_order_by_returns_error() {
        let query = "SELECT * FROM users ORDER BY name";
        let result = QueryPhrases::from_query(query, &[]);
        
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "ORDER BY clauses are not supported");
    }

    #[test]
    fn test_binary_parameter_returns_error() {
        let query = "SELECT * FROM files WHERE data = $1";
        let params = vec![DataType::Binary(Some(vec![1, 2, 3]))];
        let result = QueryPhrases::from_query(query, &params);
        
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Binary parameters are not supported in query strings");
    }

    #[test]
    fn test_complex_where_clause() {
        let query = "SELECT name, age FROM users WHERE age >= $1 AND (status = $2 OR role = $3)";
        let params = vec![
            DataType::Int32(Some(21)),
            DataType::Str(Some("active".to_string())),
            DataType::Str(Some("admin".to_string())),
        ];
        let result = QueryPhrases::from_query(query, &params).unwrap();
        
        assert_eq!(result.select, Some("name, age".to_string()));
        assert_eq!(result.filter, Some("age >= 21 AND (status = 'active' OR role = 'admin')".to_string()));
        assert_eq!(result.top, None);
    }

    #[test]
    fn test_case_insensitive_keywords() {
        let query = "select * from users where id = $1";
        let params = vec![DataType::Int32(Some(1))];
        let result = QueryPhrases::from_query(query, &params).unwrap();
        
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
        let result = QueryPhrases::from_query(query, &params).unwrap();
        
        assert_eq!(result.filter, Some("i32 = 100 AND i64 = 1000 AND u32 = 200 AND u64 = 2000 AND f32 = 1.5 AND f64 = 99.99".to_string()));
    }

    #[test]
    fn test_to_odata_simple() {
        let phrases = QueryPhrases {
            select: Some("*".to_string()),
            filter: Some("age > 18".to_string()),
            top: Some(10),
        };
        
        let odata = phrases.to_odata();
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
        
        let odata = phrases.to_odata();
        assert_eq!(odata, "$select=name%2C%20email%2C%20age");
    }

    #[test]
    fn test_to_odata_complex_filter() {
        let phrases = QueryPhrases {
            select: Some("name, age".to_string()),
            filter: Some("age >= 21 AND (status = 'active' OR role = 'admin')".to_string()),
            top: Some(5),
        };
        
        let odata = phrases.to_odata();
        assert_eq!(odata, "$select=name%2C%20age&$filter=age%20ge%2021%20and%20%28status%20eq%20%27active%27%20or%20role%20eq%20%27admin%27%29&$top=5");
    }

    #[test]
    fn test_to_odata_all_operators() {
        let phrases = QueryPhrases {
            select: None,
            filter: Some("a = 1 AND b != 2 AND c > 3 AND d >= 4 AND e < 5 AND f <= 6".to_string()),
            top: None,
        };
        
        let odata = phrases.to_odata();
        assert_eq!(odata, "$filter=a%20eq%201%20and%20b%20ne%202%20and%20c%20gt%203%20and%20d%20ge%204%20and%20e%20lt%205%20and%20f%20le%206");
    }

    #[test]
    fn test_to_odata_url_encoding() {
        let phrases = QueryPhrases {
            select: None,
            filter: Some("name = 'John O''Brien'".to_string()),
            top: None,
        };
        
        let odata = phrases.to_odata();
        assert_eq!(odata, "$filter=name%20eq%20%27John%20O%27%27Brien%27");
    }
}

