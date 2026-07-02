//! Cursor end-to-end example runtime.
//!
//! Binds `WasiModel` to the spawned-`cursor-agent` backend and serves two wasm
//! guests over one HTTP trigger: `/ask` (calls `complete`) and `/mcp/docs` (the
//! read-only MCP documentation server the agent reads). See `README.md`.

#[cfg(not(target_arch = "wasm32"))]
use omnia_cursor::Client;
#[cfg(not(target_arch = "wasm32"))]
use omnia_wasi_http::{HttpDefault, WasiHttp};
#[cfg(not(target_arch = "wasm32"))]
use omnia_wasi_model::WasiModel;
#[cfg(not(target_arch = "wasm32"))]
use omnia_wasi_otel::{OtelDefault, WasiOtel};

#[cfg(not(target_arch = "wasm32"))]
omnia::runtime!({
    hosts: {
        WasiHttp: HttpDefault,
        WasiOtel: OtelDefault,
        WasiModel: Client,
    }
});

#[cfg(target_arch = "wasm32")]
fn main() {}
