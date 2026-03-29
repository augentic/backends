# omnia-azure-table

[![crates.io](https://img.shields.io/crates/v/omnia-azure-table.svg)](https://crates.io/crates/omnia-azure-table)
[![docs.rs](https://docs.rs/omnia-azure-table/badge.svg)](https://docs.rs/omnia-azure-table)

Azure Table Storage backend for the Omnia WASI runtime, implementing the `wasi-jsondb` interface.

Azure Table Storage is a `NoSQL` key-value store. This crate maps the jsondb document model onto Azure Table entities: top-level JSON fields are flattened into entity properties so that server-side `OData` `$filter` queries work, while nested objects are serialized as JSON string properties.

MSRV: Rust 1.93

## Key Mapping

The `collection` string encodes `{table}/{partitionKey}` (split on the first `/`). The document `id` maps to the Azure Table `RowKey`.

| jsondb concept | Azure Table equivalent |
|----------------|------------------------|
| `collection` | `{table}/{PartitionKey}` |
| `id` | `RowKey` |
| `document.data` | Flattened entity properties |

Example: `get("users/tenant-a", "user-123")` → table=`users`, PK=`tenant-a`, RK=`user-123`.

A table-only collection (`"users"` without a `/`) is allowed for `query()` (cross-partition scan) but rejected for point operations (`get`, `insert`, `put`, `delete`).

## `OData` Type Mapping

Top-level JSON fields are flattened into typed entity properties. The crate
automatically adds `@odata.type` annotations where Azure Table cannot infer
the type from the JSON representation alone.

| JSON value | Azure `OData` type | Annotation added? |
|------------|-------------------|-------------------|
| `true` / `false` | `Edm.Boolean` | No (inferred) |
| integer ≤ `i32` range | `Edm.Int32` | No (inferred) |
| integer > `i32` range | `Edm.Int64` | Yes |
| floating point | `Edm.Double` | Yes |
| string | `Edm.String` | No (inferred) |
| `null` | (skipped) | N/A |
| array / object | `Edm.String` (JSON-serialized) | No |

`Edm.DateTime`, `Edm.Guid`, and `Edm.Binary` require the document to include
explicit `@odata.type` annotations — the crate does not attempt to guess these
from string values.

## Supported Operations

| Operation | Description |
|-----------|-------------|
| `get` | Point read by `PartitionKey` + `RowKey` |
| `insert` | Insert new entity (fails if exists) |
| `put` | Upsert entity |
| `delete` | Delete entity (returns whether it existed) |
| `query` | Filtered listing with `OData` `$filter` and pagination |
| `ensure_table` | Creates a table if it does not exist (admin helper) |

## Filter Support

| Filter | Supported | Notes |
|--------|-----------|-------|
| `Compare` (eq/ne/gt/gte/lt/lte) | Yes | Translated to `OData` operators |
| `InList` / `NotInList` | Yes | Expanded to OR chains |
| `And` / `Or` / `Not` | Yes | Supported when all children are supported |
| `Contains` / `StartsWith` / `EndsWith` | **No** | Rejected with error |
| `IsNull` / `IsNotNull` | **No** | Rejected with error |
| `offset` | **No** | Rejected with error — use continuation tokens |
| `continuation` | Yes | Native Azure Table continuation tokens |

Unsupported filters and query options return an error rather than silently
falling back to client-side evaluation, which could pull unbounded data from
the table service. Azure Table's `OData` `$filter` does not support string
functions or null checks, and there is no `$skip` — see
[Querying tables and entities](https://learn.microsoft.com/en-us/rest/api/storageservices/querying-tables-and-entities#supported-query-options).

> **Note:** `order_by` is ignored. Azure Table returns results in
> `PartitionKey` / `RowKey` order; there is no server-side `$orderby`.
> Callers that need a different sort order should sort after retrieval.

## Why not `azure_data_tables`?

The `azure_data_tables` crate is on a [legacy branch](https://github.com/Azure/azure-sdk-for-rust/tree/legacy) with no official replacement planned. This crate calls the REST API directly and leaves business object mapping to the WebAssembly guest.

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `AZURE_STORAGE_ACCOUNT` | yes | | Storage account name |
| `AZURE_STORAGE_KEY` | yes | | Storage account access key |
| `AZURE_TABLE_ENDPOINT` | no | `https://{account}.table.core.windows.net` | Table service endpoint URL |

Set `AZURE_TABLE_ENDPOINT` to override the default public-cloud URL. Common
values:

- **Azurite**: `http://127.0.0.1:10002/{account}`
- **Azure sovereign cloud**: the appropriate `table.core.*` URL for your region

## Usage

```rust,ignore
use omnia::Backend;
use omnia_azure_table::Client;

let options = omnia_azure_table::ConnectOptions::from_env()?;
let client = Client::connect_with(options).await?;

// Create the table if needed (admin helper, not part of wasi-jsondb)
client.ensure_table("my_table").await?;
```

## License

MIT OR Apache-2.0
