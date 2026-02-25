# omnia-azure-table

[![crates.io](https://img.shields.io/crates/v/omnia-azure-table.svg)](https://crates.io/crates/omnia-azure-table)
[![docs.rs](https://docs.rs/omnia-azure-table/badge.svg)](https://docs.rs/omnia-azure-table)

Azure Table Storage backend for the Omnia WASI runtime, implementing the `wasi-sql` interface.

Azure Table Storage is not a full-featured relational database — this crate exposes a basic object store through a SQL-like API using the Azure REST API directly.

MSRV: Rust 1.93

## Supported Operations

**Query** (`SELECT`): simple `SELECT` and `WHERE` clauses, plus `TOP`. No `ORDER BY` or `JOIN`. Returned rows include the built-in `PartitionKey`, `RowKey`, and `Timestamp` fields.

**Exec** (`INSERT`, `UPDATE`, `DELETE`): single-entity operations only. The `WHERE` clause must filter on `PartitionKey` and `RowKey` exclusively.

## SQL Dialect

Parameters use `$1`, `$2`, ... placeholder syntax. SQL operators are translated to `OData` equivalents:

| SQL | `OData` |
|-----|-------|
| `=` | `eq` |
| `!=`, `<>` | `ne` |
| `>`, `>=`, `<`, `<=` | `gt`, `ge`, `lt`, `le` |
| `AND`, `OR`, `NOT` | `and`, `or`, `not` |

`INSERT`, `UPDATE`, and `DELETE` require `PartitionKey` and `RowKey` in every statement. `SELECT` supports `WHERE` and `TOP` but not `ORDER BY` or `JOIN`.

## Why not `azure_data_tables`?

The `azure_data_tables` crate is on a [legacy branch](https://github.com/Azure/azure-sdk-for-rust/tree/legacy) with no official replacement planned. This crate calls the REST API directly and leaves business object mapping to the WebAssembly guest.

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `AZURE_STORAGE_ACCOUNT` | yes | | Storage account name |
| `AZURE_STORAGE_KEY` | yes | | Storage account access key |

## Usage

```rust,no_run
use omnia::Backend;
use omnia_azure_table::Client;

let options = omnia_azure_table::ConnectOptions::from_env()?;
let client = Client::connect_with(options).await?;
```

## License

MIT OR Apache-2.0
