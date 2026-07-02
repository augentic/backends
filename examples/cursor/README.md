# Cursor End-to-End Example

Binds `WasiModel` to the spawned-`cursor-agent` backend and serves two wasm
guests under one HTTP server:

- `/ask` — a guest that calls `complete` once (asks for the widget lifecycle).
- `/mcp/docs` — omnia's `mcp` example guest, a read-only MCP documentation
  server. With `CURSOR_MCP_URL` set, the spawned `cursor-agent` answers the
  `/ask` question by calling this server's `list_docs`/`read_doc` tools.

Assumes an `omnia` checkout beside this repo (`../omnia`), since the `docs`
guest reuses omnia's prebuilt `mcp-wasm` artifact.

## Build

```bash
# docs guest, in the omnia checkout
(cd ../omnia && cargo build --example mcp-wasm --target wasm32-wasip2)

# ask guest, here
cargo build -p examples --example cursor-ask-wasm --target wasm32-wasip2
```

## Run

`cursor-agent` must be on `PATH` and authenticated (`cursor-agent login`).

```bash
export CURSOR_MCP_URL=http://localhost:8080/mcp/docs
export OMNIA_WORKSPACE=$(mktemp -d)   # scratch tree cursor-agent runs in
cargo run --example cursor -- run --config examples/cursor/omnia.toml
```

## Test

```bash
curl -s http://localhost:8080/ask
```

The host writes a scratch `.cursor/mcp.json` advertising `CURSOR_MCP_URL` and
spawns `cursor-agent` with `--approve-mcps`; the agent reads the docs over MCP
and returns the widget stages (`draft`, `assembled`, `shipped`).
