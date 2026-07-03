# omnia-azure-id

[![crates.io](https://img.shields.io/crates/v/omnia-azure-id.svg)](https://crates.io/crates/omnia-azure-id)
[![docs.rs](https://docs.rs/omnia-azure-id/badge.svg)](https://docs.rs/omnia-azure-id)

Azure Identity backend for the Omnia WASI runtime, implementing the `wasi-identity` interface.

Acquires Azure AD access tokens via Managed Identity credentials using the official `azure_identity` SDK.

MSRV: Rust 1.93

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `CREDENTIAL_TYPE` | no | `ManagedIdentity` | Credential type to use |

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
environment with an ambient credential (managed identity, or a service
principal):

```bash
CREDENTIAL_TYPE=ClientSecret \
AZURE_TENANT_ID=... AZURE_CLIENT_ID=... AZURE_CLIENT_SECRET=... \
  cargo nextest run -p omnia-azure-id --run-ignored all
```

## License

MIT OR Apache-2.0
