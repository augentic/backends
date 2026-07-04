# omnia-azure-id

[![crates.io](https://img.shields.io/crates/v/omnia-azure-id.svg)](https://crates.io/crates/omnia-azure-id)
[![docs.rs](https://docs.rs/omnia-azure-id/badge.svg)](https://docs.rs/omnia-azure-id)

Azure Identity backend for the Omnia WASI runtime, implementing the `wasi-identity` interface.

Acquires Azure AD access tokens via Managed Identity credentials using the official `azure_identity` SDK.

MSRV: Rust 1.95

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `CREDENTIAL_TYPE` | no | `ManagedIdentity` | Credential type to use (only `ManagedIdentity` is implemented today) |

## Usage

```rust,ignore
use omnia::Backend;
use omnia_azure_id::Client;

let options = omnia_azure_id::ConnectOptions::from_env()?;
let client = Client::connect_with(options).await?;
```

## Live tests

[`tests/live.rs`](tests/live.rs) exercises the `wasi-identity` boundary against
real Azure AD. It is `#[ignore]`d so it never runs in CI; run it explicitly in an
environment with an ambient managed-identity credential (e.g. an Azure VM,
App Service, or AKS workload identity):

```bash
cargo nextest run -p omnia-azure-id --run-ignored all
```

## License

MIT OR Apache-2.0
