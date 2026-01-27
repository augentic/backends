# Architecture

This document describes the architecture of Qwasr (Quick WebAssembly Safe Runtime), a modular WASI component runtime built on [wasmtime](https://github.com/bytecodealliance/wasmtime).

## Overview

Qwasr provides a thin wrapper around wasmtime for ergonomic integration of host-based services for WASI components. It enables WebAssembly guests to interact with external services (databases, message queues, etc.) through standardized WASI interfaces, while allowing hosts to swap backend implementations without changing guest code.

```text
┌─────────────────────────────────────────────────────────────────────┐
│                           Host Runtime                              │
│                                                                     │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌────────────┐    │
│  │  Backend   │  │  Backend   │  │  Backend   │  │  Backend   │    │
│  │  (Redis)   │  │  (Kafka)   │  │  (Azure)   │  │  (NATS)    │    │
│  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘    │
│        │               │               │               │            │
│  ┌─────┴──────┐  ┌─────┴──────┐  ┌─────┴──────┐  ┌─────┴──────┐    │
│  │ wasi-kv    │  │ wasi-msg   │  │ wasi-vault │  │ wasi-blob  │    │
│  │ (WASI API) │  │ (WASI API) │  │ (WASI API) │  │ (WASI API) │    │
│  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘    │
│        │               │               │               │            │
│        └───────────────┴───────┬───────┴───────────────┘            │
│                                │                                    │
│                         ┌──────┴──────┐                             │
│                         │   kernel    │                             │
│                         │ (wasmtime)  │                             │
│                         └──────┬──────┘                             │
│                                │                                    │
│   ┌────────────────────────────┴────────────────────────────────┐   │
│   │                     WebAssembly Guest                       │   │
│   │              (Your application logic - .wasm)               │   │
│   └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

## Core Concepts

### Guest/Host Architecture

Qwasr follows the WebAssembly Component Model's guest/host pattern:

- **Guest**: Application code compiled to WebAssembly (`.wasm`). Uses WASI interfaces to interact with the outside world. The guest is portable and qwasr-agnostic.

- **Host**: The native runtime that loads and executes the WebAssembly guest. Provides concrete implementations of WASI interfaces by connecting to actual backends (Redis, Kafka, Postgres, etc.).

This separation allows the same guest code to run with different backends—swap Redis for NATS without changing application logic.

### Three-Layer Architecture

Qwasr is organized into three distinct layers:

```text
┌─────────────────────────────────────────────────────────────────┐
│  Layer 3: Backends (*) (this repo)                              │
│  Concrete connections to external services                      │
│  Examples: redis, kafka, nats, azure, postgres                  │
├─────────────────────────────────────────────────────────────────┤
│  Layer 2: WASI Interfaces (wasi-*)                              │
│  Abstract service capabilities defined by WIT interfaces        │
│  Examples: wasi-keyvalue, wasi-messaging, wasi-blobstore        │
├─────────────────────────────────────────────────────────────────┤
│  Layer 1: Kernel                                                │
│  Core runtime infrastructure (wasmtime, CLI, traits)            │
└─────────────────────────────────────────────────────────────────┘
```

## Crate Organization

### Kernel (`crates/kernel`)

The foundation of the runtime. Provides:

- **CLI infrastructure**: Command-line interface for running and compiling WebAssembly components
- **Core traits**: `State`, `Host`, `Server`, and `Backend` traits that all components implement
- **Wasmtime integration**: Re-exports and wrappers for wasmtime functionality

Key traits:

```rust
/// Implemented by all WASI hosts to link their dependencies
pub trait Host<T>: Debug + Sync + Send {
    fn add_to_linker(linker: &mut Linker<T>) -> Result<()>;
}

/// Implemented by WASI hosts that are servers
pub trait Server<S: State>: Debug + Sync + Send {
    fn run(&self, state: &S) -> impl Future<Output = Result<()>>;
}

/// Implemented by backend resources for connection management
pub trait Backend: Sized + Sync + Send {
    type ConnectOptions: FromEnv;
    fn connect_with(options: Self::ConnectOptions) -> impl Future<Output = Result<Self>>;
}
```

### Backend Crates (`crates/*`)

Backend crates provide concrete implementations connecting to external services:

| Crate           | Service           | Supports                       |
| --------------- | ----------------- | ------------------------------ |
| `redis`         | Redis             | keyvalue                       |
| `nats`          | NATS              | keyvalue, messaging, blobstore |
| `kafka`         | Apache Kafka      | messaging                      |
| `mongodb`       | MongoDB           | blobstore                      |
| `postgres`      | PostgreSQL        | sql                            |
| `azure_id`      | Azure Identity    | identity.                      |
| `azure_vault`   | Azure Key Vault   | vault                          |
| `azure_table`   | Azure Table Store | sql                            |
| `opentelemetry` | OTEL Collector    | otel                           |

Each backend:

1. Implements the `Backend` trait for connection management
2. Implements the context trait for its supported WASI interfaces (e.g., `WasiKeyValueCtx`)
3. Loads configuration from environment variables via `FromEnv`

Example backend structure:

```rust
#[derive(Clone)]
pub struct Client(ConnectionManager);

impl Backend for Client {
    type ConnectOptions = ConnectOptions;

    async fn connect_with(options: Self::ConnectOptions) -> Result<Self> {
        // Connect to the service...
    }
}

// Implement WASI interface contexts
impl WasiKeyValueCtx for Client {
    fn open_bucket(&self, identifier: String) -> FutureResult<Arc<dyn Bucket>> {
        // Provide keyvalue functionality via Redis...
    }
}
```

## Runtime Execution Flow

1. **CLI Parsing**: The kernel parses command-line arguments (`run` or `compile`)

2. **Backend Connection**: The `runtime!` macro-generated code connects to all configured backends using environment variables

3. **Component Compilation**: The WebAssembly component is compiled (or loaded if pre-compiled)

4. **Linker Setup**: Each WASI interface's `add_to_linker` method is called to register host functions

5. **Instance Pre-instantiation**: The component is pre-instantiated for efficient spawning

6. **Server Start**: Server interfaces (HTTP, messaging, WebSockets) start listening for requests

7. **Request Handling**: Incoming requests spawn new instances, execute guest code, and return responses

```text
CLI → Backend Connect → Compile → Link → Pre-instantiate → Server Loop
                                                              ↓
                                              Request → Instance → Response
```

## Configuration

All backends use environment variables for configuration. The `FromEnv` derive macro (from the `fromenv` crate) provides automatic parsing:

```rust
#[derive(Debug, Clone, FromEnv)]
pub struct ConnectOptions {
    #[env(from = "REDIS_URL", default = "redis://localhost:6379")]
    pub url: String,
    #[env(from = "REDIS_MAX_RETRIES", default = "3")]
    pub max_retries: usize,
}
```

See individual backend READMEs for specific environment variables.

## Adding a New Backend

1. Create `crates/<name>/`
2. Implement the `Backend` trait for connection management
3. Implement context traits for supported WASI interfaces (e.g., `WasiKeyValueCtx`)
4. Add `ConnectOptions` with `FromEnv` derive
5. Create example(s) demonstrating the backend

## Related Documentation

- [wasmtime Component Model](https://docs.wasmtime.dev/api/wasmtime/component/)
- [WASI Proposals](https://github.com/WebAssembly/WASI/blob/main/Proposals.md)
- [WIT Format](https://component-model.bytecodealliance.org/design/wit.html)
- [examples/README.md](./examples/README.md) - Running examples
