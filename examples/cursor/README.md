# Cursor Example

Live model completion via [`omnia-cursor`](../crates/cursor): the `ask` guest calls `create` once (command mode) while an HTTP server serves `/ask` and the [`mcp`](https://github.com/augentic/omnia/tree/main/examples/mcp) docs guest at `/mcp/docs` for the spawned `cursor-agent`.

Requires a sibling [`omnia`](https://github.com/augentic/omnia) checkout (for the `mcp` docs guest) and [`cursor-agent`](https://cursor.com/docs/cli) on `PATH` (`cursor-agent login`).

## Build and run

```bash
# backends repo — cursor guest + runtime
cargo build -p examples --example cursor-wasm --target wasm32-wasip2

# omnia repo (sibling checkout) — docs MCP guest
cargo build -p examples --example mcp-wasm --target wasm32-wasip2

# backends repo — create the working tree the `[[mount]]` lends
mkdir -p examples/cursor/workspace

# backends repo — run the deployment
cargo run -p examples --example cursor -- run --config examples/cursor/omnia.toml
```

The guest's `docs` MCP grant carries its endpoint URL directly (`http://localhost:8080/mcp/docs` in `guest.rs`), pointing at the sibling docs guest. The `[[mount]]` in `omnia.toml` preopens `examples/cursor/workspace` as the tree named `.`; the guest lends it through `grants.workspace` and the cursor backend resolves it to the working tree the spawned agent edits.

## Test

The run command above includes a test that calls `create` once and prints the answer.

## MCP servers

MCP wiring is opt-in per completion and spans two layers:

1. **Prompt grant** — the guest names a server in `tools` and supplies its endpoint `url` (here, `docs` → `http://localhost:8080/mcp/docs` in `guest.rs`). Only granted servers are wired into the spawned `cursor-agent`.
2. **HTTP route** — `omnia.toml` serves that endpoint as a WASM guest (`/mcp/docs` → the omnia `mcp` docs server).

When a completion runs, `omnia-cursor` reads the grant's `url`, merges the entry into `<workspace>/.cursor/mcp.json` for the spawn, and passes `--approve-mcps`. `cursor-agent` has no `--mcp-config` flag; it discovers servers only from that file (or `~/.cursor/mcp.json`).
