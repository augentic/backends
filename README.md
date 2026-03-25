# Omnia Backends

Infrastructure backend implementations for the [Omnia](https://augentic.io) WASI runtime. Each crate bridges a WASI interface to a concrete service (database, message broker, vault, etc.).

MSRV: Rust 1.93

## Crates

| Crate | WASI Interface | Service |
| ----- | -------------- | ------- |
| [`omnia-azure-blob`](crates/azure-blob) | `wasi-blobstore` | Azure Blob Storage |
| [`omnia-azure-id`](crates/azure_id) | `wasi-identity` | Azure Managed Identity |
| [`omnia-azure-table`](crates/azure_table) | `wasi-sql` | Azure Table Storage |
| [`omnia-azure-vault`](crates/azure_vault) | `wasi-vault` | Azure Key Vault |
| [`omnia-kafka`](crates/kafka) | `wasi-messaging` | Apache Kafka |
| [`omnia-mongodb`](crates/mongodb) | `wasi-blobstore` | MongoDB |
| [`omnia-nats`](crates/nats) | `wasi-messaging`, `wasi-keyvalue`, `wasi-blobstore` | NATS / JetStream |
| [`omnia-opentelemetry`](crates/opentelemetry) | `wasi-otel` | OpenTelemetry Collector (gRPC) |
| [`omnia-postgres`](crates/postgres) | `wasi-sql` | PostgreSQL |
| [`omnia-redis`](crates/redis) | `wasi-keyvalue` | Redis |

## Architecture

All backends implement the `omnia::Backend` trait and are loaded by the Omnia runtime at startup. See [`docs/Architecture.md`](docs/Architecture.md) for details.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).
