# omnia-postgres

[![crates.io](https://img.shields.io/crates/v/omnia-postgres.svg)](https://crates.io/crates/omnia-postgres)
[![docs.rs](https://docs.rs/omnia-postgres/badge.svg)](https://docs.rs/omnia-postgres)

`PostgreSQL` backend for the Omnia WASI runtime, implementing the `wasi-sql` interface.

Uses `deadpool-postgres` connection pooling with optional TLS via `rustls`. Supports multiple named pools for connecting to several databases from a single runtime.

MSRV: Rust 1.93

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `POSTGRES_URL` | yes | | Default pool connection URI |
| `POSTGRES_POOL_SIZE` | no | `10` | Default pool size |
| `POSTGRES_POOLS` | no | | Comma-separated extra pool names |
| `POSTGRES_URL__<NAME>` | per pool | | URI for named pool |
| `POSTGRES_POOL_SIZE__<NAME>` | no | inherited | Pool size for named pool |

## Usage

```rust,no_run
use omnia::Backend;
use omnia_postgres::Client;

let options = omnia_postgres::ConnectOptions::from_env()?;
let client = Client::connect_with(options).await?;
```

## License

MIT OR Apache-2.0
