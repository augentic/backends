# omnia-redis

[![crates.io](https://img.shields.io/crates/v/omnia-redis.svg)](https://crates.io/crates/omnia-redis)
[![docs.rs](https://docs.rs/omnia-redis/badge.svg)](https://docs.rs/omnia-redis)

Redis key-value backend for the Omnia WASI runtime, implementing the `wasi-keyvalue` interface.

Uses the `redis` crate with a `ConnectionManager` for automatic reconnection and retry.

MSRV: Rust 1.93

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `REDIS_URL` | no | `redis://localhost:6379` | Redis connection URL |
| `REDIS_MAX_RETRIES` | no | `3` | Maximum reconnection attempts |
| `REDIS_MAX_DELAY` | no | `1000` | Maximum retry delay in milliseconds |

## Usage

```rust,ignore
use omnia::Backend;
use omnia_redis::Client;

let options = omnia_redis::ConnectOptions::from_env()?;
let client = Client::connect_with(options).await?;
```

## License

MIT OR Apache-2.0
