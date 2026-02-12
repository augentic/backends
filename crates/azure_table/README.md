# Azure Table Storage Resource for WASI

This crate implements `qwasr::Backend` to provide an Azure Table Storage client resource for wasi-sql services. Note that Azure Table Storage is not a full-featured relational database so consumers of this crate should understand that it represents a basic object store with a SQL-like API.

Very few queries are supported since we are really overloading an object store with a query-like API.

For the `query` (reading records) implementation, simple SELECT and WHERE clauses are possible. So is TOP. But no ORDER BY or JOIN clauses are permitted. Objects returned will have the built-in fields

* `PartitionKey`
* `RowKey`
* `Timestamp`

For the `exec` implementation, only INSERT, DELETE and UPDATE are supported on single objects. The query's WHERE clause must only include the `PartitionKey` and `RowKey` fields (parameterised). No other filter is supported.

## IMPORTANT: Azure REST API

This crate does not use the `azure_data_table` crate which is no longer an official Microsoft SDK release. There is no official SDK for Azure Table Storage or any documentation that promises one will be developed. The repository for the crate is on a [legacy branch](https://github.com/Azure/azure-sdk-for-rust/tree/legacy).

Instead this crate uses the REST API.

Further, the legacy driver includes an inferred business object mapping that does not conform to `wasi-sql`. For our purposes, business object mapping is the concern of the WebAssembly guest.
