# omnia-nats

[![crates.io](https://img.shields.io/crates/v/omnia-nats.svg)](https://crates.io/crates/omnia-nats)
[![docs.rs](https://docs.rs/omnia-nats/badge.svg)](https://docs.rs/omnia-nats)

NATS backend for the Omnia WASI runtime, implementing the `wasi-messaging`, `wasi-keyvalue`, and `wasi-blobstore` interfaces.

Uses `async-nats` with JetStream for key-value and object store capabilities. Supports JWT/NKey authentication.

MSRV: Rust 1.93

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `NATS_ADDR` | no | `demo.nats.io` | NATS server address |
| `NATS_TOPICS` | no | | Comma-separated subscription topics |
| `NATS_JWT` | no | | JWT for authentication |
| `NATS_SEED` | no | | `NKey` seed for signing |

## Usage

```rust,no_run
use omnia::Backend;
use omnia_nats::Client;

let options = omnia_nats::ConnectOptions::from_env()?;
let client = Client::connect_with(options).await?;
```

## License

MIT OR Apache-2.0
