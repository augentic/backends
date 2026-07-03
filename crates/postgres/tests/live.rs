//! Live query round-trip for the Postgres backend, driven through the
//! `omnia:sql` host boundary (`WasiSqlCtx`). This is the acceptance gate for
//! `into_wasi_row`, which cannot be unit-tested (its input `tokio_postgres::Row`
//! is unconstructable without a real server).
//!
//! `#[ignore]`d so it never touches the network in CI. Run against a reachable
//! database (`POSTGRES_URL`):
//! `cargo nextest run -p omnia-postgres --run-ignored all`.

use anyhow::Result;
use omnia::Backend;
use omnia_postgres::Client;
use omnia_wasi_sql::{Connection, DataType, WasiSqlCtx};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: needs a reachable Postgres (POSTGRES_URL); run with --run-ignored"]
async fn query() -> Result<()> {
    let client = <Client as Backend>::connect().await?;
    let conn: std::sync::Arc<dyn Connection> = client.open("default".to_owned()).await?;

    let rows =
        conn.query("SELECT $1::int4 AS n".to_owned(), vec![DataType::Int32(Some(42))]).await?;

    assert_eq!(rows.len(), 1, "one row returned");
    let field = &rows[0].fields[0];
    assert_eq!(field.name, "n", "column alias round-trips");
    assert!(
        matches!(field.value, DataType::Int32(Some(42))),
        "int4 maps back through into_wasi_row: {:?}",
        field.value
    );
    Ok(())
}
