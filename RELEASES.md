## 0.26.0

Released 2026-06-10

### Changed

- The Azure Table backend now encodes the partition key in the document id
  (`{PartitionKey}\0{RowKey}`), so collections no longer need the
  `{table}/{partitionKey}` format.
- Migrated `azure-blob` to the `azure_storage_blob` 1.0 download-range API.
- Upgraded Omnia SDK 0.31.0 → 0.33.0 and the Azure SDK suite to 1.0
  (`azure_core`, `azure_identity`, `azure_storage_blob`,
  `azure_security_keyvault_secrets`).
- Raised the minimum supported Rust version to 1.95.
- Refreshed backend dependencies, including `mongodb` 3.7, `async-nats` 0.49,
  `tokio-postgres-rustls` 0.14, and `opentelemetry-proto` 0.32.

<!-- Release notes generated using configuration in .github/release.yaml at main -->

## What's Changed
* Fix azure table to support partition key in document id by @karthik-phl in https://github.com/augentic/backends/pull/25


**Full Changelog**: https://github.com/augentic/backends/compare/v0.25.0...v0.26.0

---

Release notes for previous releases can be found on the respective release
branches of the repository.

<!-- ARCHIVE_START -->
* [0.26.x](https://github.com/augentic/backends/blob/release-0.26.0/RELEASES.md)

- [0.25.x](https://github.com/augentic/backends/blob/release-0.25.0/RELEASES.md)
- [0.24.x](https://github.com/augentic/backends/blob/release-0.24.0/RELEASES.md)
- [0.23.x](https://github.com/augentic/backends/blob/release-0.23.0/RELEASES.md)
