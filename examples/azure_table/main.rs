use std::env;

use dotenvy::dotenv;
use omnia::Backend;
use omnia_azure_table::{Client, ConnectOptions};
use omnia_wasi_jsondb::{
    ComparisonOp, Document, FilterTree, QueryOpts, QueryResult, ScalarValue, WasiJsonDbCtx,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Demonstrates the serde pattern for explicit `@odata.type` annotations.
///
/// Azure Table cannot infer `Edm.Guid`, `Edm.DateTime`, or `Edm.Binary` from
/// raw JSON strings.  Include a sibling `{field}@odata.type` key in the
/// serialized output so that [`document::flatten`] passes it through verbatim.
#[derive(Debug, Serialize, Deserialize)]
struct TypedRecord {
    #[serde(rename = "Name")]
    name: String,

    #[serde(rename = "SessionId")]
    session_id: String,
    #[serde(rename = "SessionId@odata.type")]
    session_id_type: String,

    #[serde(rename = "ExpiresAt")]
    expires_at: String,
    #[serde(rename = "ExpiresAt@odata.type")]
    expires_at_type: String,
}

const TABLE: &str = "testJsondb";
const COLLECTION: &str = "testJsondb/desk";

fn doc(id: &str, data: Value) -> Document {
    Document {
        id: id.into(),
        data: serde_json::to_vec(&data).expect("serialize"),
    }
}

fn body(d: &Document) -> Value {
    serde_json::from_slice(&d.data).expect("deserialize")
}

fn ids(r: &QueryResult) -> Vec<&str> {
    r.documents.iter().map(|d| d.id.as_str()).collect()
}

fn cmp(field: &str, op: ComparisonOp, value: ScalarValue) -> FilterTree {
    FilterTree::Compare {
        field: field.into(),
        op,
        value,
    }
}

async fn q(client: &Client, filter: Option<FilterTree>, opts: QueryOpts) -> QueryResult {
    client.query(COLLECTION.into(), filter, opts).await.expect("query")
}

async fn q_err(client: &Client, filter: FilterTree) -> String {
    client
        .query(COLLECTION.into(), Some(filter), QueryOpts::default())
        .await
        .unwrap_err()
        .to_string()
}

fn pass(label: &str) {
    println!("  PASS  {label}");
}

fn seed() -> Vec<(&'static str, Value)> {
    vec![
        (
            "alice",
            json!({
                "Name": "Alice Montgomery",
                "IsActive": true,
                "Points": 950,
                "Rating": 4.8,
                "Created": "2026-01-15T10:30:00Z",
                "Region": "US-West"
            }),
        ),
        (
            "bob",
            json!({
                "Name": "Bob Burns",
                "IsActive": false,
                "Points": 200,
                "Rating": 3.2,
                "Created": "2026-02-20T14:00:00Z",
                "Region": "EU-North"
            }),
        ),
        (
            "charlie",
            json!({
                "Name": "Charlie Delta",
                "IsActive": true,
                "Points": 3000,
                "LargeId": 9_007_199_254_740_993_i64,
                "Rating": 4.95,
                "Created": "2025-12-01T08:00:00Z",
                "Region": "US-West"
            }),
        ),
        (
            "dana",
            json!({
                "Name": "Dana Null",
                "IsActive": true,
                "Points": 500,
                "Rating": 3.9,
                "Created": "2026-03-10T16:45:00Z"
            }),
        ),
        (
            "eve",
            json!({
                "Name": "Eve Eavesdrop",
                "IsActive": false,
                "Points": 150,
                "Rating": 2.1,
                "Created": "2026-01-01T00:00:00Z",
                "Region": "AP-South"
            }),
        ),
    ]
}

#[tokio::main]
pub async fn main() {
    dotenv().ok();
    tracing_subscriber::registry().with(fmt::layer()).with(EnvFilter::from_default_env()).init();

    let opts = ConnectOptions {
        name: env::var("AZURE_STORAGE_ACCOUNT").expect("Set AZURE_STORAGE_ACCOUNT"),
        key: env::var("AZURE_STORAGE_KEY").expect("Set AZURE_STORAGE_KEY"),
        endpoint: env::var("AZURE_TABLE_ENDPOINT").unwrap_or_default(),
    };
    let client = Client::connect_with(opts).await.expect("connect");

    println!("Azure Table JSONDB desk test");
    println!("============================\n");

    // ── Setup ────────────────────────────────────────────────────────
    let created = client.ensure_table(TABLE).await.expect("ensure_table");
    println!("Table '{TABLE}': {}", if created { "created" } else { "already exists" });

    let existing = q(&client, None, QueryOpts::default()).await;
    for d in &existing.documents {
        client.delete(COLLECTION.into(), d.id.clone()).await.expect("cleanup delete");
    }
    if !existing.documents.is_empty() {
        println!("Cleaned {} leftover rows", existing.documents.len());
    }

    let records = seed();
    for (id, data) in &records {
        client
            .insert(COLLECTION.into(), doc(id, data.clone()))
            .await
            .unwrap_or_else(|e| panic!("insert {id}: {e}"));
    }
    println!("Seeded {} records\n", records.len());

    // ── 1. Get round-trip ────────────────────────────────────────────
    let alice = client
        .get(COLLECTION.into(), "alice".into())
        .await
        .expect("get alice")
        .expect("alice missing");
    let ab = body(&alice);
    assert_eq!(ab["Name"], "Alice Montgomery");
    assert_eq!(ab["IsActive"], true);
    assert_eq!(ab["Points"], 950);
    assert_eq!(ab["Rating"], 4.8);
    assert_eq!(ab["Created"], "2026-01-15T10:30:00Z");
    assert_eq!(ab["Region"], "US-West");
    pass("get round-trip (all fields verified)");

    // ── 2. Query all ─────────────────────────────────────────────────
    let r = q(&client, None, QueryOpts::default()).await;
    assert_eq!(r.documents.len(), 5, "expected 5, got {}", r.documents.len());
    pass("query all (5 records)");

    // ── 3. Compare: Eq Boolean ───────────────────────────────────────
    let r = q(
        &client,
        Some(cmp("IsActive", ComparisonOp::Eq, ScalarValue::Boolean(true))),
        QueryOpts::default(),
    )
    .await;
    assert_eq!(r.documents.len(), 3);
    pass("Compare: IsActive eq true (3)");

    // ── 4. Compare: Gt Int32 ─────────────────────────────────────────
    let r = q(
        &client,
        Some(cmp("Points", ComparisonOp::Gt, ScalarValue::Int32(500))),
        QueryOpts::default(),
    )
    .await;
    assert_eq!(r.documents.len(), 2);
    assert!(ids(&r).contains(&"alice"));
    assert!(ids(&r).contains(&"charlie"));
    pass("Compare: Points gt 500 (2)");

    // ── 5. Compare: Lte Float64 ──────────────────────────────────────
    let r = q(
        &client,
        Some(cmp("Rating", ComparisonOp::Lte, ScalarValue::Float64(3.9))),
        QueryOpts::default(),
    )
    .await;
    assert_eq!(r.documents.len(), 3);
    pass("Compare: Rating le 3.9 (3)");

    // ── 6. Compare: Gte String (timestamp) ───────────────────────────
    let r = q(
        &client,
        Some(cmp("Created", ComparisonOp::Gte, ScalarValue::Str("2026-02-01T00:00:00Z".into()))),
        QueryOpts::default(),
    )
    .await;
    assert_eq!(r.documents.len(), 2);
    assert!(ids(&r).contains(&"bob"));
    assert!(ids(&r).contains(&"dana"));
    pass("Compare: Created ge '2026-02-01' (2)");

    // ── 7. InList ────────────────────────────────────────────────────
    let r = q(
        &client,
        Some(FilterTree::InList {
            field: "Points".into(),
            values: vec![ScalarValue::Int32(200), ScalarValue::Int32(500)],
        }),
        QueryOpts::default(),
    )
    .await;
    assert_eq!(r.documents.len(), 2);
    assert!(ids(&r).contains(&"bob"));
    assert!(ids(&r).contains(&"dana"));
    pass("InList: Points in [200, 500] (2)");

    // ── 8. NotInList ─────────────────────────────────────────────────
    let r = q(
        &client,
        Some(FilterTree::NotInList {
            field: "Name".into(),
            values: vec![
                ScalarValue::Str("Alice Montgomery".into()),
                ScalarValue::Str("Bob Burns".into()),
            ],
        }),
        QueryOpts::default(),
    )
    .await;
    assert_eq!(r.documents.len(), 3);
    pass("NotInList: Name not in [Alice, Bob] (3)");

    // ── 9. Compare: Ne ──────────────────────────────────────────────
    let r = q(
        &client,
        Some(cmp("Region", ComparisonOp::Ne, ScalarValue::Str("US-West".into()))),
        QueryOpts::default(),
    )
    .await;
    // Azure Table excludes entities where the property is absent (dana has no
    // Region), so only bob and eve match — not 3.
    assert_eq!(r.documents.len(), 2);
    assert!(ids(&r).contains(&"bob"));
    assert!(ids(&r).contains(&"eve"));
    pass("Compare: Region ne 'US-West' (2, absent fields excluded)");

    // ── 10. And (all server-side) ───────────────────────────────────
    let r = q(
        &client,
        Some(FilterTree::And(vec![
            cmp("IsActive", ComparisonOp::Eq, ScalarValue::Boolean(true)),
            cmp("Points", ComparisonOp::Gt, ScalarValue::Int32(500)),
        ])),
        QueryOpts::default(),
    )
    .await;
    assert_eq!(r.documents.len(), 2);
    assert!(ids(&r).contains(&"alice"));
    assert!(ids(&r).contains(&"charlie"));
    pass("And: IsActive=true AND Points gt 500 (2)");

    // ── 11. Edm.Int64 round-trip ────────────────────────────────────
    let charlie = client
        .get(COLLECTION.into(), "charlie".into())
        .await
        .expect("get charlie")
        .expect("charlie missing");
    let cb = body(&charlie);
    assert_eq!(cb["LargeId"], 9_007_199_254_740_993_i64, "Edm.Int64 should round-trip as number");
    pass("Edm.Int64 round-trip (LargeId)");

    // ── 12. Duplicate insert fails ──────────────────────────────────
    let dup = client.insert(COLLECTION.into(), doc("alice", json!({"Name": "Duplicate"}))).await;
    assert!(dup.is_err(), "inserting duplicate id should fail");
    pass("duplicate insert rejected");

    // ── 13. Get non-existent returns None ────────────────────────────
    let missing = client
        .get(COLLECTION.into(), "no-such-id".into())
        .await
        .expect("get non-existent should not error");
    assert!(missing.is_none(), "non-existent id should return None");
    pass("get non-existent returns None");

    // ── 14–18. Unsupported filters are rejected ─────────────────────
    // Azure Table OData does not support string functions or null checks.
    // See: https://learn.microsoft.com/en-us/rest/api/storageservices/querying-tables-and-entities
    let err = q_err(
        &client,
        FilterTree::Contains {
            field: "Name".into(),
            pattern: "Delta".into(),
        },
    )
    .await;
    assert!(err.contains("not supported"), "Contains should be rejected: {err}");
    pass("Contains rejected");

    let err = q_err(
        &client,
        FilterTree::StartsWith {
            field: "Name".into(),
            pattern: "Eve".into(),
        },
    )
    .await;
    assert!(err.contains("not supported"), "StartsWith should be rejected: {err}");
    pass("StartsWith rejected");

    let err = q_err(
        &client,
        FilterTree::EndsWith {
            field: "Name".into(),
            pattern: "Burns".into(),
        },
    )
    .await;
    assert!(err.contains("not supported"), "EndsWith should be rejected: {err}");
    pass("EndsWith rejected");

    let err = q_err(&client, FilterTree::IsNull("Region".into())).await;
    assert!(err.contains("not supported"), "IsNull should be rejected: {err}");
    pass("IsNull rejected");

    let err = q_err(&client, FilterTree::IsNotNull("Region".into())).await;
    assert!(err.contains("not supported"), "IsNotNull should be rejected: {err}");
    pass("IsNotNull rejected");

    // ── 14. Not (server-side) ────────────────────────────────────────
    let r = q(
        &client,
        Some(FilterTree::Not(Box::new(cmp(
            "IsActive",
            ComparisonOp::Eq,
            ScalarValue::Boolean(true),
        )))),
        QueryOpts::default(),
    )
    .await;
    assert_eq!(r.documents.len(), 2);
    assert!(ids(&r).contains(&"bob"));
    assert!(ids(&r).contains(&"eve"));
    pass("Not: NOT(IsActive eq true) (2)");

    // ── 15. Or (server-side) ─────────────────────────────────────────
    let r = q(
        &client,
        Some(FilterTree::Or(vec![
            cmp("Points", ComparisonOp::Eq, ScalarValue::Int32(200)),
            cmp("Points", ComparisonOp::Eq, ScalarValue::Int32(150)),
        ])),
        QueryOpts::default(),
    )
    .await;
    assert_eq!(r.documents.len(), 2);
    assert!(ids(&r).contains(&"bob"));
    assert!(ids(&r).contains(&"eve"));
    pass("Or: Points=200 OR Points=150 (2)");

    // ── 16. Limit ────────────────────────────────────────────────────
    let r = q(
        &client,
        None,
        QueryOpts {
            limit: Some(2),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(r.documents.len(), 2);
    pass("limit=2");

    // ── 17. Offset rejected ─────────────────────────────────────────
    let err = client
        .query(
            COLLECTION.into(),
            None,
            QueryOpts {
                offset: Some(1),
                limit: Some(2),
                ..Default::default()
            },
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("offset is not supported"), "offset should be rejected: {err}");
    pass("offset rejected");

    // ── 18. Edm.Guid + Edm.DateTime via serde annotations ───────────
    let typed = TypedRecord {
        name: "Frank Typed".into(),
        session_id: "a1b2c3d4-e5f6-7890-abcd-ef1234567890".into(),
        session_id_type: "Edm.Guid".into(),
        expires_at: "2026-12-31T23:59:59Z".into(),
        expires_at_type: "Edm.DateTime".into(),
    };
    let typed_json = serde_json::to_vec(&typed).expect("serialize TypedRecord");
    client
        .insert(
            COLLECTION.into(),
            Document {
                id: "frank".into(),
                data: typed_json,
            },
        )
        .await
        .expect("insert frank");

    let frank = client
        .get(COLLECTION.into(), "frank".into())
        .await
        .expect("get frank")
        .expect("frank missing");
    let fb = body(&frank);
    assert_eq!(fb["Name"], "Frank Typed");
    assert_eq!(fb["SessionId"], "a1b2c3d4-e5f6-7890-abcd-ef1234567890");
    assert_eq!(fb["ExpiresAt"], "2026-12-31T23:59:59Z");
    assert!(
        fb.get("SessionId@odata.type").is_none(),
        "odata annotations should be stripped on read"
    );
    pass("Edm.Guid + Edm.DateTime via serde annotations (round-trip)");

    client.delete(COLLECTION.into(), "frank".into()).await.expect("delete frank");

    // ── 19. Put + get (verify change) ────────────────────────────────
    client
        .put(
            COLLECTION.into(),
            doc(
                "alice",
                json!({
                    "Name": "Alice Montgomery",
                    "IsActive": true,
                    "Points": 999,
                    "Rating": 4.8,
                    "Created": "2026-01-15T10:30:00Z",
                    "Region": "US-West"
                }),
            ),
        )
        .await
        .expect("put alice");
    let updated = client
        .get(COLLECTION.into(), "alice".into())
        .await
        .expect("get alice after put")
        .expect("alice missing after put");
    assert_eq!(body(&updated)["Points"], 999);
    pass("put + get (verify Points changed to 999)");

    // ── 20. Delete + get (verify gone) ───────────────────────────────
    let deleted = client.delete(COLLECTION.into(), "eve".into()).await.expect("delete eve");
    assert!(deleted, "eve should have existed");
    let gone = client.get(COLLECTION.into(), "eve".into()).await.expect("get eve after delete");
    assert!(gone.is_none(), "eve should be gone");
    pass("delete + get (verify gone)");

    // ── 21. Double-delete returns false ──────────────────────────────
    let again = client.delete(COLLECTION.into(), "eve".into()).await.expect("double-delete eve");
    assert!(!again, "double-delete should return false");
    pass("double-delete returns false");

    // ── 22. Table-only collection rejected for get ───────────────────
    let err = client.get(TABLE.into(), "alice".into()).await;
    assert!(err.is_err(), "get on table-only collection should fail");
    pass("table-only collection rejected for get");

    // ── Teardown ─────────────────────────────────────────────────────
    let remaining = q(&client, None, QueryOpts::default()).await;
    for d in &remaining.documents {
        client.delete(COLLECTION.into(), d.id.clone()).await.expect("teardown delete");
    }
    println!("\nCleaned up {} rows. All tests passed!", remaining.documents.len());
}
