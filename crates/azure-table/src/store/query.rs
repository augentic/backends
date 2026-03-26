//! Client-side query post-processing: offset emulation and Azure Table
//! continuation token encoding.

use omnia_wasi_jsondb::Document;

/// Encode Azure Table continuation headers into a single opaque token.
///
/// Azure Table returns `x-ms-continuation-NextPartitionKey` and
/// `x-ms-continuation-NextRowKey` response headers. We pack both
/// into one string separated by `\n`.
#[must_use]
#[allow(clippy::similar_names)]
pub fn encode_continuation(
    next_partition_key: Option<&str>, next_row_key: Option<&str>,
) -> Option<String> {
    match (next_partition_key, next_row_key) {
        (Some(pk), Some(rk)) => Some(format!("{pk}\n{rk}")),
        (Some(pk), None) => Some(pk.to_string()),
        _ => None,
    }
}

/// Decode our opaque token back into (`NextPartitionKey`, `NextRowKey`).
#[must_use]
pub fn decode_continuation(token: &str) -> (String, Option<String>) {
    match token.split_once('\n') {
        Some((pk, rk)) => (pk.to_owned(), Some(rk.to_owned())),
        None => (token.to_owned(), None),
    }
}

/// Skip `offset` documents from the front.
#[must_use]
pub fn apply_offset(docs: Vec<Document>, offset: u32) -> Vec<Document> {
    let skip = offset as usize;
    if skip >= docs.len() { Vec::new() } else { docs.into_iter().skip(skip).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc(id: &str, json: &serde_json::Value) -> Document {
        Document {
            id: id.to_string(),
            data: serde_json::to_vec(json).unwrap(),
        }
    }

    #[test]
    fn continuation_roundtrip() {
        let token = encode_continuation(Some("pk1"), Some("rk1")).unwrap();
        let (pk, rk) = decode_continuation(&token);
        assert_eq!(pk, "pk1");
        assert_eq!(rk.as_deref(), Some("rk1"));
    }

    #[test]
    fn continuation_pk_only() {
        let token = encode_continuation(Some("pk1"), None).unwrap();
        let (pk, rk) = decode_continuation(&token);
        assert_eq!(pk, "pk1");
        assert!(rk.is_none());
    }

    #[test]
    fn apply_offset_skips() {
        let docs = vec![
            make_doc("1", &serde_json::json!({})),
            make_doc("2", &serde_json::json!({})),
            make_doc("3", &serde_json::json!({})),
        ];
        let result = apply_offset(docs, 2);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "3");
    }
}
