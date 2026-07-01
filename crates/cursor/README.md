# omnia-cursor

[![crates.io](https://img.shields.io/crates/v/omnia-cursor.svg)](https://crates.io/crates/omnia-cursor)
[![docs.rs](https://docs.rs/omnia-cursor/badge.svg)](https://docs.rs/omnia-cursor)

Spawned-agent model backend for the Omnia WASI runtime, implementing the
`augentic:model/completion` boundary (`wasi-model`).

Each completion launches a fresh, context-free [`cursor-agent`](https://cursor.com/docs/cli/headless)
session that owns its own tool loop and edits the lent working tree directly,
then returns a validated answer through the same boundary as `omnia-genai`.
string; the model id, the API key, and the agent protocol stay inside this
crate.

MSRV: Rust 1.95

## Requirements

The [`cursor-agent`](https://cursor.com/docs/cli) CLI must be installed and on
`PATH` (validated at `connect`), and authenticated via `CURSOR_API_KEY` or a
prior `cursor-agent login`. The key is read by the spawned process and is never
captured, logged, or recorded into fixtures.

## Configuration

| Variable              | Required | Default                | Description                                                                                         |
| --------------------- | -------- | ---------------------- | --------------------------------------------------------------------------------------------------- |
| `CURSOR_MODEL`        | no       | _cursor-agent default_ | Model id forwarded to `cursor-agent --model`; unset lets the agent choose                           |
| `OMNIA_WORKSPACE`     | no       | _none_                 | Node-local working-tree path lent via `--workspace`; unset is the "no local tree" capability signal |
| `CURSOR_TIMEOUT_SECS` | no       | `120`                  | Wall-clock bound on one `cursor-agent` spawn                                                        |

`OMNIA_WORKSPACE` is a stopgap for the RFC-55 working-tree host's `local-path`
face: until that host lands, the workspace is sourced from config rather than
resolved from the lent `grants.working-tree` descriptor. An absent workspace
yields `error::backend("no local tree on this node")`, preserving the
capability signal.

## Usage

```rust,ignore
use omnia::Backend;
use omnia_cursor::Client;

let options = omnia_cursor::ConnectOptions::from_env()?;
let client = Client::connect_with(options).await?;
```

## License

MIT OR Apache-2.0
