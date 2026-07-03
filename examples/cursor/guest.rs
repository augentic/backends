//! # Cursor example — `ask` guest
//!
//! A `wasi:cli/command` reactor that **imports** `omnia:model/completion` and
//! calls `create` once when the host drives `wasi:cli/run`. The prompt carries
//! a `docs` MCP grant; when the runtime binds `WasiModel` to the cursor backend,
//! the backend resolves that logical name to a configured endpoint and wires the
//! spawned `cursor-agent` to the read-only MCP documentation server served in the
//! background by the sibling `docs` guest.
//!
//! It reads `wasi:filesystem/preopens` and lends the `.` mount (the `[[mount]]`
//! in `omnia.toml`) through `grants.workspace`; the host resolves it to the
//! node-local working tree the spawned agent edits.
//!
//! It also exports `wasi:http` on `/ask` so the same completion can be triggered
//! over HTTP. `omnia.toml` routes `/ask` here.

#![cfg(target_arch = "wasm32")]

use axum::Router;
use axum::routing::get;
use omnia_wasi_model::completion::{self, Format, Grants, Mcp, Sections, Tool};
use tracing::Level;
use wasip3::filesystem::preopens;
use wasip3::http::types as http;

struct CliGuest;
wasip3::cli::command::export!(CliGuest);

impl wasip3::exports::cli::run::Guest for CliGuest {
    #[omnia_wasi_otel::instrument(name = "cursor_example_run", level = Level::DEBUG)]
    async fn run() -> Result<(), ()> {
        // Read the preopen table the host populated from `[[mount]]` and lend the
        // tree named `.` as the working tree. `directories` must outlive the
        // `create` call — the lent `workspace` borrows one of its descriptors.
        let directories = preopens::get_directories();
        let workspace = directories.iter().find_map(|(dir, name)| (name == ".").then_some(dir));

        tracing::info!(workspace = workspace.is_some(), mcp = "docs", "cursor example completion");

        let request = completion::Request {
            model: None,
            system: Some(
                "You answer strictly from the read-only `docs` MCP documentation tools. Do not guess."
                    .to_string(),
            ),
            messages: vec![],
            sections: Some(Sections {
                role: Some("a terse technical writer".to_string()),
                task: "Using the docs MCP server, state the lifecycle stages a widget moves \
                    through, in order."
                    .to_string(),
                context: None,
                constraints: vec![],
                examples: vec![],
                variables: vec![],
            }),
            generation: None,
            format: Format::Json,
            tools: vec![Tool::Mcp(Mcp {
                name: "docs".to_string(),
                tools: vec![],
                url: Some("http://localhost:8080/mcp".to_string()),
            })],
            grants: Grants {
                references: None,
                workspace,
                verify: vec![],
            },
        };

        let answer = match completion::create(request).await {
            Ok(reply) => {
                tracing::info!("cursor example answered");
                reply.answer
            }
            Err(error) => {
                tracing::warn!(?error, "cursor example completion failed");
                format!("error: {error:?}")
            }
        };

        println!("{answer}");
        Ok(())
    }
}

struct HttpMcp;
wasip3::http::service::export!(HttpMcp);

impl wasip3::exports::http::handler::Guest for HttpMcp {
    #[omnia_wasi_otel::instrument(name = "http_mcp_handle", level = Level::DEBUG)]
    async fn handle(request: http::Request) -> Result<http::Response, http::ErrorCode> {
        let router = Router::new().route("/mcp", get(mcp));
        omnia_wasi_http::serve(router, request).await
    }
}

// Trigger the same completion over HTTP and return its validated answer.
async fn mcp() -> String {
    tracing::debug!("cursor example /mcp request");
    "mcp response".to_string()
}
