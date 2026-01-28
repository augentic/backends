# Azure Table Storage Resource for WASI

This crate implements `qwasr::Backend` to provide an Azure Table Storage client resource for wasi-sql services. Note that Azure Table Storage is not a full-featured relational database so consumers of this crate should understand that it represents a basic object store with a SQL-like API.

## IMPORTANT: Unofficial SDK

This crate relies on the `azure_data_table` crate which is no longer an official Microsoft SDK release. There is no official SDK for Azure Table Storage or any documentation that promises one will be developed. The repository for the crate is on a [legacy branch](https://github.com/Azure/azure-sdk-for-rust/tree/legacy).
