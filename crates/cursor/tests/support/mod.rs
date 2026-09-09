//! Shared helpers for cursor backend integration tests.

pub mod local_path_tool_host;
pub mod mcp_server;

pub use local_path_tool_host::{
    CHECK_WORD, TOOL_SENTINEL, checking_tool_host, local_path_tool_host, no_tool_host,
};
pub use mcp_server::{SENTINEL, serve};
