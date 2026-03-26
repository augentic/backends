# Azure Table Storage Desk Test

Self-seeding desk test for the `omnia-azure-table` crate's `wasi-jsondb`
implementation. It creates the table, seeds five records covering multiple
`OData` types, exercises every filter variant and CRUD operation, then
cleans up after itself.

No manual data pre-seeding is required.

## Covered scenarios

- **CRUD**: insert, get round-trip, put (upsert), delete, double-delete,
  duplicate insert rejected, get non-existent returns None
- **Filters**: `Compare` (Eq/Ne/Gt/Gte/Lte across Boolean, Int32, Float64,
  String), `InList`, `NotInList`, `And`, `Or`, `Not`
- **Unsupported filter rejection**: `Contains`, `StartsWith`, `EndsWith`,
  `IsNull`, `IsNotNull` are rejected with an error (Azure Table `OData`
  does not support string functions or null checks)
- **Query options**: `offset`, `limit`, continuation tokens
- **OData type annotations**: `Edm.Int64` round-trip, `Edm.Guid` and
  `Edm.DateTime` via serde `#[serde(rename = "Field@odata.type")]` pattern
- **Error paths**: table-only collection rejected for point operations

## Running with Azurite

1. Start Azurite (table service on port 10002):

   ```bash
   azurite --tableHost 127.0.0.1 --tablePort 10002
   ```

   Or via Docker:

   ```bash
   docker run -d -p 10002:10002 mcr.microsoft.com/azure-storage/azurite \
     azurite-table --tableHost 0.0.0.0 --skipApiVersionCheck
   ```

2. Create an `.env` file in the workspace root (or export the variables):

   ```bash
   AZURE_STORAGE_ACCOUNT=devstoreaccount1
   AZURE_STORAGE_KEY=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==
   AZURE_TABLE_ENDPOINT=http://127.0.0.1:10002/devstoreaccount1
   ```

   These are the well-known Azurite development credentials.

3. Run the desk test:

   ```bash
   cargo run --example azure_table
   ```

## Running against Azure

Set the storage account name and key. Leave `AZURE_TABLE_ENDPOINT` unset
(defaults to `https://{account}.table.core.windows.net`):

```bash
AZURE_STORAGE_ACCOUNT=myaccount \
AZURE_STORAGE_KEY=<base64-key> \
cargo run --example azure_table
```

## Expected output

```
Azure Table JSONDB desk test
============================

Table 'testJsondb': created
Seeded 5 records

  PASS  get round-trip (all fields verified)
  PASS  query all (5 records)
  PASS  Compare: IsActive eq true (3)
  ...
  PASS  table-only collection rejected for get

Cleaned up 4 rows. All tests passed!
```
