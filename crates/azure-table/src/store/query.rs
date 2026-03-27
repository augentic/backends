//! Azure Table continuation token encoding.

/// Separator for continuation token encoding. U+0000 is forbidden in Azure
/// Table partition and row keys (control characters U+0000–U+001F are
/// disallowed), so it is unambiguous.
const TOKEN_SEP: char = '\0';

/// Encode Azure Table continuation headers into a single opaque token.
///
/// Azure Table returns `x-ms-continuation-NextPartitionKey` and
/// `x-ms-continuation-NextRowKey` response headers. We pack both
/// into one string separated by a null byte.
#[must_use]
#[allow(clippy::similar_names)]
pub fn encode_continuation(
    next_partition_key: Option<&str>, next_row_key: Option<&str>,
) -> Option<String> {
    match (next_partition_key, next_row_key) {
        (Some(pk), Some(rk)) => Some(format!("{pk}{TOKEN_SEP}{rk}")),
        (Some(pk), None) => Some(pk.to_string()),
        _ => None,
    }
}

/// Decode our opaque token back into (`NextPartitionKey`, `NextRowKey`).
#[must_use]
pub fn decode_continuation(token: &str) -> (String, Option<String>) {
    match token.split_once(TOKEN_SEP) {
        Some((pk, rk)) => (pk.to_owned(), Some(rk.to_owned())),
        None => (token.to_owned(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
