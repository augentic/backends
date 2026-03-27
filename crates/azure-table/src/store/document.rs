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
/// Type annotations (`@odata.type`) are used to restore fidelity for
/// `Edm.Int64` (string → i64 number) and `Edm.Double` (ensure f64
/// representation). Nested objects and arrays that were serialized as JSON
/// strings during [`flatten`] are **not** automatically restored — they
/// remain as string values. See the crate README for details.
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
        let restored = restore_typed_value(obj, key, value);
        data_map.insert(key.clone(), restored);
    }

    let data = serde_json::to_vec(&data_map).context("serializing document body")?;
    Ok(Document { id, data })
}

/// Use `@odata.type` annotations to restore type fidelity where Azure Table
/// serialization loses the original JSON type.
fn restore_typed_value(obj: &Map<String, Value>, key: &str, value: &Value) -> Value {
    let type_key = format!("{key}@odata.type");
    let Some(edm_type) = obj.get(&type_key).and_then(Value::as_str) else {
        return value.clone();
    };

    match edm_type {
        "Edm.Int64" => value
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .map_or_else(|| value.clone(), |n| Value::Number(n.into())),
        "Edm.Double" => match value {
            Value::Number(n) => n
                .as_f64()
                .map_or_else(|| value.clone(), |f| json_f64(f).unwrap_or_else(|| value.clone())),
            Value::String(s) => {
                s.parse::<f64>().ok().and_then(json_f64).unwrap_or_else(|| value.clone())
            }
            _ => value.clone(),
        },
        _ => value.clone(),
    }
}

/// Create a `serde_json::Value::Number` from an f64, returning `None` for
/// `NaN` / infinity which JSON cannot represent.
fn json_f64(f: f64) -> Option<Value> {
    serde_json::Number::from_f64(f).map(Value::Number)
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
    fn unflatten_restores_edm_int64() {
        let entity = json!({
            "PartitionKey": "pk1",
            "RowKey": "r1",
            "LargeId": "9007199254740993",
            "LargeId@odata.type": "Edm.Int64",
        });
        let doc = unflatten(&entity).unwrap();
        let body: Value = serde_json::from_slice(&doc.data).unwrap();
        assert_eq!(body["LargeId"], 9_007_199_254_740_993_i64);
    }

    #[test]
    fn unflatten_restores_edm_double() {
        let entity = json!({
            "PartitionKey": "pk1",
            "RowKey": "r1",
            "Rating": 3,
            "Rating@odata.type": "Edm.Double",
        });
        let doc = unflatten(&entity).unwrap();
        let body: Value = serde_json::from_slice(&doc.data).unwrap();
        assert!(body["Rating"].is_f64(), "expected f64, got {:?}", body["Rating"]);
        assert!((body["Rating"].as_f64().unwrap() - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn nested_objects_remain_strings_after_roundtrip() {
        let doc = Document {
            id: "r1".into(),
            data: serde_json::to_vec(&json!({"tags": ["a", "b"], "meta": {"x": 1}})).unwrap(),
        };
        let entity = flatten(&doc, "pk1").unwrap();
        let roundtripped = unflatten(&entity).unwrap();
        let body: Value = serde_json::from_slice(&roundtripped.data).unwrap();
        assert_eq!(body["tags"].as_str().unwrap(), r#"["a","b"]"#);
        assert_eq!(body["meta"].as_str().unwrap(), r#"{"x":1}"#);
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
