# Azure Table Storage Resource for WASI

This crate implements `qwasr::Backend` to provide an Azure Table Storage client resource for wasi-sql services. Note that Azure Table Storage is not a full-featured relational database so consumers of this crate should understand that it represents a basic object store with a SQL-like API.

The crate could be extended to implement wasi-blobstore.
