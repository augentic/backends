# Cursor Example

Live model completion via `[omnia-cursor](../crates/cursor)`: the `ask` guest calls `create` once (command mode) while an HTTP server serves `/ask` and the `[mcp](https://github.com/augentic/omnia/tree/main/examples/mcp)` docs guest at `/mcp/docs` for the spawned `cursor-agent`.

Requires a sibling `[omnia](https://github.com/augentic/omnia)` checkout (for the `mcp` docs guest) and `[cursor-agent](https://cursor.com/docs/cli)` on `PATH` (`cursor-agent login`).

## Build and run

```bash
# build the guest
cargo build -p examples --example cursor-wasm --target wasm32-wasip2

# create the working tree the `[[mount]]` lends
mkdir -p examples/cursor/workspace

# set Cursor API key
export CURSOR_API_KEY=<cursor API key>
export RUST_LOG=info,omnia_cursor=debug,cursor_wasm=debug

# run the host (no config)
cargo run --example cursor -- run ./target/wasm32-wasip2/debug/examples/cursor_wasm.wasm --mount path=examples/cursor/workspace,name=.,writable

# run the host (with config)
cargo run --example cursor -- run --config examples/cursor/config.toml
```

The guest's `docs` MCP grant carries its endpoint URL directly (`http://localhost:8080/mcp/docs` in `guest.rs`), pointing at the sibling docs guest. The `[[mount]]` in `omnia.toml` preopens `examples/cursor/workspace` as the tree named `.`; the guest lends it through `grants.workspace` and the cursor backend resolves it to the working tree the spawned agent edits.

## Test

The run command above includes a test that calls `create` once and prints the answer.

## MCP servers

MCP wiring is opt-in per completion and spans two layers:

1. **Prompt grant** — the guest names a server in `tools` and supplies its endpoint `url` (here, `docs` → `http://localhost:8080/mcp/docs` in `guest.rs`). Only granted servers are wired into the spawned `cursor-agent`.
2. **HTTP route** — `omnia.toml` serves that endpoint as a WASM guest (`/mcp/docs` → the omnia `mcp` docs server).

When a completion runs, `omnia-cursor` reads the grant's `url`, merges the entry into `<workspace>/.cursor/mcp.json` for the spawn, and passes `--approve-mcps`. `cursor-agent` has no `--mcp-config` flag; it discovers servers only from that file (or `~/.cursor/mcp.json`).