//! Shared helpers for cursor backend integration tests.

pub mod mcp_server;
pub mod noop_tool_host;

pub use mcp_server::{SENTINEL, serve};
pub use noop_tool_host::noop_tool_host;
