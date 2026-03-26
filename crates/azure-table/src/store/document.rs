//! Conversion between [`Document`] and Azure Table entity JSON.
//!
//! Azure Table stores typed entity properties rather than raw JSON blobs.
//! Top-level JSON fields are flattened into entity properties so that
//! server-side `OData` `$filter` queries work. Nested objects and arrays
//! are serialized as JSON string properties.

use anyhow::{Context, anyhow};
use omnia_wasi_jsondb::Document;
use serde_json::{Map, Value};

/// Azure Table system / `OData` metadata properties stripped during unflatten.
const SYSTEM_KEYS: &[&str] = &["PartitionKey", "RowKey", "Timestamp"];

/// Build an Azure Table entity JSON body from a [`Document`] and partition key.
///
/// Top-level JSON fields become entity properties. `PartitionKey` and `RowKey`
/// are injected from the collection string and `doc.id` respectively. `OData`
/// type annotations (`@odata.type`) are added for types that Azure Table
/// cannot infer from the JSON representation alone.
///
/// # Errors
///
/// Returns an error if the document body is not valid JSON or not a JSON object.
pub fn flatten(doc: &Document, partition_key: &str) -> anyhow::Result<Value> {
    let body: Value =
        serde_json::from_slice(&doc.data).context("document body is not valid JSON")?;
    let src = body.as_object().ok_or_else(|| anyhow!("document body must be a JSON object"))?;

    let mut entity = Map::new();
    entity.insert("PartitionKey".into(), Value::String(partition_key.into()));
    entity.insert("RowKey".into(), Value::String(doc.id.clone()));

    for (key, value) in src {
        insert_typed_property(&mut entity, key, value)?;
    }

    Ok(Value::Object(entity))
}

/// Convert an Azure Table entity JSON (from a GET/query response) into a
/// [`Document`], stripping system and `OData` metadata properties.
///
/// # Errors
///
/// Returns an error if the entity is not a JSON object or is missing `RowKey`.
pub fn unflatten(entity: &Value) -> anyhow::Result<Document> {
    let obj = entity.as_object().ok_or_else(|| anyhow!("entity must be a JSON object"))?;

    let id = obj
        .get("RowKey")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("entity missing RowKey"))?
        .to_owned();

    let mut data_map = Map::new();
    for (key, value) in obj {
        if is_metadata_key(key) {
            continue;
        }
        data_map.insert(key.clone(), value.clone());
    }

    let data = serde_json::to_vec(&data_map).context("serializing document body")?;
    Ok(Document { id, data })
}

fn is_metadata_key(key: &str) -> bool {
    SYSTEM_KEYS.contains(&key) || key.starts_with("odata.") || key.ends_with("@odata.type")
}

/// Insert a single user property into the entity map, adding `@odata.type`
/// annotations where Azure Table cannot infer the type from raw JSON.
fn insert_typed_property(
    entity: &mut Map<String, Value>, key: &str, value: &Value,
) -> anyhow::Result<()> {
    match value {
        Value::Null => {}
        Value::Bool(_) | Value::String(_) => {
            entity.insert(key.into(), value.clone());
        }
        Value::Number(n) => {
            if n.is_f64() && !n.is_i64() {
                entity.insert(key.into(), value.clone());
                entity.insert(format!("{key}@odata.type"), Value::String("Edm.Double".into()));
            } else if let Some(v) = n.as_i64()
                && !(i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&v)
            {
                entity.insert(key.into(), Value::String(v.to_string()));
                entity.insert(format!("{key}@odata.type"), Value::String("Edm.Int64".into()));
            } else {
                entity.insert(key.into(), value.clone());
            }
        }
        Value::Array(_) | Value::Object(_) => {
            let serialized =
                serde_json::to_string(value).context("serializing nested value to JSON string")?;
            entity.insert(key.into(), Value::String(serialized));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn flatten_adds_keys_and_preserves_fields() {
        let doc = Document {
            id: "r1".into(),
            data: serde_json::to_vec(&json!({"Name": "Alice", "Points": 42})).unwrap(),
        };
        let entity = flatten(&doc, "pk1").unwrap();
        let obj = entity.as_object().unwrap();
        assert_eq!(obj["PartitionKey"], "pk1");
        assert_eq!(obj["RowKey"], "r1");
        assert_eq!(obj["Name"], "Alice");
        assert_eq!(obj["Points"], 42);
    }

    #[test]
    fn flatten_annotates_large_int() {
        let doc = Document {
            id: "r1".into(),
            data: serde_json::to_vec(&json!({"big": 9_223_372_036_854_775_807_i64})).unwrap(),
        };
        let entity = flatten(&doc, "pk1").unwrap();
        let obj = entity.as_object().unwrap();
        assert_eq!(obj["big@odata.type"], "Edm.Int64");
    }

    #[test]
    fn flatten_skips_null_fields() {
        let doc = Document {
            id: "r1".into(),
            data: serde_json::to_vec(&json!({"a": null, "b": "ok"})).unwrap(),
        };
        let entity = flatten(&doc, "pk1").unwrap();
        let obj = entity.as_object().unwrap();
        assert!(!obj.contains_key("a"));
        assert_eq!(obj["b"], "ok");
    }

    #[test]
    fn flatten_serializes_nested_objects_as_strings() {
        let doc = Document {
            id: "r1".into(),
            data: serde_json::to_vec(&json!({"nested": {"x": 1}})).unwrap(),
        };
        let entity = flatten(&doc, "pk1").unwrap();
        let obj = entity.as_object().unwrap();
        assert_eq!(obj["nested"].as_str().unwrap(), r#"{"x":1}"#);
    }

    #[test]
    fn unflatten_strips_system_keys() {
        let entity = json!({
            "PartitionKey": "pk1",
            "RowKey": "r1",
            "Timestamp": "2026-01-01T00:00:00Z",
            "odata.etag": "W/\"datetime'...'\"",
            "Name": "Alice",
            "Name@odata.type": "Edm.String",
        });
        let doc = unflatten(&entity).unwrap();
        assert_eq!(doc.id, "r1");
        let body: Value = serde_json::from_slice(&doc.data).unwrap();
        let obj = body.as_object().unwrap();
        assert!(!obj.contains_key("PartitionKey"));
        assert!(!obj.contains_key("RowKey"));
        assert!(!obj.contains_key("Timestamp"));
        assert!(!obj.contains_key("odata.etag"));
        assert!(!obj.contains_key("Name@odata.type"));
        assert_eq!(obj["Name"], "Alice");
    }
}
