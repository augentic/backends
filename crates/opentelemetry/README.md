# omnia-opentelemetry

[![crates.io](https://img.shields.io/crates/v/omnia-opentelemetry.svg)](https://crates.io/crates/omnia-opentelemetry)
[![docs.rs](https://docs.rs/omnia-opentelemetry/badge.svg)](https://docs.rs/omnia-opentelemetry)

OpenTelemetry gRPC backend for the Omnia WASI runtime, implementing the `wasi-otel` interface.

Exports traces and metrics to an OpenTelemetry Collector via gRPC using the OTLP protocol. Telemetry failures are logged but never propagated to application logic.

MSRV: Rust 1.93

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `OTEL_GRPC_URL` | no | `http://localhost:4317` | Collector gRPC endpoint |

## Usage

```rust,ignore
use omnia::Backend;
use omnia_opentelemetry::Client;

let options = omnia_opentelemetry::ConnectOptions::from_env()?;
let client = Client::connect_with(options).await?;
```

## License

MIT OR Apache-2.0
