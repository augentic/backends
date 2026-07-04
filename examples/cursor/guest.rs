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

use std::sync::Arc;

use omnia_guest::mcp::{
    self, CallToolResult, Implementation, McpError, McpServer, Resource, ResourceContents,
    Tool as McpTool,
};
use omnia_wasi_model::completion::{self, Format, Grants, Mcp, Tool};
use omnia_wasi_model::prompt::Sections;
use serde_json::{Value, json};
use tracing::Level;
use wasip3::filesystem::preopens;
use wasip3::http::types as http;

struct CliGuest;
wasip3::cli::command::export!(CliGuest);

impl wasip3::exports::cli::run::Guest for CliGuest {
    #[omnia_wasi_otel::instrument(name = "cursor_example_run", level = Level::DEBUG)]
    async fn run() -> Result<(), ()> {
        // Read the preopen table the host populated from `[[mount]]`.
        let directories = preopens::get_directories();
        let workspace = directories.iter().find_map(|(dir, name)| (name == ".").then_some(dir));

        tracing::info!(workspace = workspace.is_some(), mcp = "docs", "cursor example completion");

        let (system, messages) = Sections {
            role: Some("a terse technical writer".to_string()),
            task: "Using the docs MCP server, state the lifecycle stages a widget moves through, \
                   in order."
                .to_string(),
            ..Sections::default()
        }
        .channels(Some(
            "You answer strictly from the read-only `docs` MCP documentation tools. Do not guess.",
        ));

        let request = completion::Request {
            model: None,
            system,
            messages,
            generation: None,
            format: Format::Text,
            tools: vec![Tool::Mcp(Mcp {
                name: "docs".to_string(),
                tools: vec![],
                url: "http://localhost:8080/mcp".to_string(),
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

struct HttpGuest;
wasip3::http::service::export!(HttpGuest);

impl wasip3::exports::http::handler::Guest for HttpGuest {
    #[omnia_wasi_otel::instrument(name = "http_mcp_handle", level = Level::DEBUG)]
    async fn handle(request: http::Request) -> Result<http::Response, http::ErrorCode> {
        tracing::debug!("cursor example mcp request");
        let mcp_router = mcp::router(Arc::new(References));
        omnia_wasi_http::serve(mcp_router, request).await
    }
}

struct References;

impl McpServer for References {
    fn info(&self) -> Implementation {
        Implementation::new("omnia-docs", env!("CARGO_PKG_VERSION"))
    }

    fn tools(&self) -> Vec<McpTool> {
        vec![
            McpTool::new(
                "list_docs",
                "List the name and title of every available document.",
                json!({ "type": "object", "properties": {} }),
            ),
            McpTool::new(
                "read_doc",
                "Read one document in full by its `name` (as returned by `list_docs`).",
                json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "the document name, e.g. `overview`",
                        }
                    },
                    "required": ["name"],
                }),
            ),
        ]
    }

    fn call_tool(&self, name: &str, arguments: &Value) -> Result<CallToolResult, McpError> {
        tracing::debug!(tool = name, "mcp tool call");
        match name {
            "list_docs" => {
                let listing = REFERENCES
                    .iter()
                    .map(|(doc_name, title, _)| format!("- {doc_name}: {title}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(CallToolResult::text(listing))
            }
            "read_doc" => {
                let Some(name) = arguments.get("name").and_then(Value::as_str) else {
                    return Err(McpError::invalid_params("missing `name`"));
                };
                REFERENCES.iter().find(|(ref_name, ..)| *ref_name == name).map_or_else(
                    || Ok(CallToolResult::error(format!("no reference named `{name}`"))),
                    |(.., body)| Ok(CallToolResult::text(*body)),
                )
            }
            other => Err(McpError::method_not_found(format!("unknown tool `{other}`"))),
        }
    }

    fn resources(&self) -> Vec<Resource> {
        REFERENCES
            .iter()
            .map(|(name, title, _)| {
                Resource::new(
                    format!("doc://{name}"),
                    *title,
                    format!("The {title} document."),
                    "text/markdown",
                )
            })
            .collect()
    }

    fn read_resource(&self, uri: &str) -> Result<ResourceContents, McpError> {
        tracing::debug!(uri, "mcp resource read");
        let name = uri.strip_prefix("doc://").unwrap_or(uri);
        REFERENCES.iter().find(|(ref_name, ..)| *ref_name == name).map_or_else(
            || Err(McpError::invalid_params(format!("unknown resource `{uri}`"))),
            |(.., body)| Ok(ResourceContents::text(uri, "text/markdown", *body)),
        )
    }
}

/// The compiled-in prose corpus as `(name, title, body)` triples.
const REFERENCES: &[(&str, &str, &str)] = &[
    (
        "overview",
        "Widget Service Overview",
        "# Widget Service Overview\n\n\
         Widgets move through `draft`, `assembled`, and `shipped` in order. They \
         never move backwards.\n",
    ),
    (
        "api-reference",
        "Widget Service API Reference",
        "# Widget Service API Reference\n\n\
         `POST /widgets` creates a draft widget. `POST /widgets/{id}/assemble` \
         advances it to `assembled`.\n",
    ),
    (
        "style-guide",
        "Widget Service Style Guide",
        "# Widget Service Style Guide\n\n\
         Labels are kebab-case. IDs are ULIDs.\n",
    ),
];
