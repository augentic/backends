# omnia-mongodb

[![crates.io](https://img.shields.io/crates/v/omnia-mongodb.svg)](https://crates.io/crates/omnia-mongodb)
[![docs.rs](https://docs.rs/omnia-mongodb/badge.svg)](https://docs.rs/omnia-mongodb)

MongoDB blobstore backend for the Omnia WASI runtime, implementing the `wasi-blobstore` interface.

Maps blobstore containers to MongoDB collections using the official `mongodb` driver.

MSRV: Rust 1.93

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `MONGODB_URL` | yes | | MongoDB connection URI (must include a default database) |

## Usage

```rust,ignore
use omnia::Backend;
use omnia_mongodb::Client;

let options = omnia_mongodb::ConnectOptions::from_env()?;
let client = Client::connect_with(options).await?;
```

## License

MIT OR Apache-2.0
