use anyhow::Result;
use rmcp::{
    Error as McpError, RoleClient, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::Parameters},
    model::*,
    schemars,
    service::{RequestContext, RunningService},
    tool, tool_handler, tool_router,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use rmcp::{ServiceExt, transport::stdio};
use serde::Deserialize;
use serde_json::json;
use std::borrow::Cow;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::sync::Arc;
use tokio::process::Command;
use tracing_subscriber::{self, EnvFilter};

use mcp_mux::{MCPMux, MCPTransport, build_mux};

#[tokio::main]
async fn main() -> Result<()> {
    let filename = env::args().nth(1).expect("Usage: mux <servers.json>");
    // Example servers.json:
    // ```
    // {
    //   "counter": {
    //     "Stdio" : {
    //       "cmd": "/Users/tom/workspace/mcp-rust-sdk/target/debug/examples/servers_counter_stdio",
    //       "args": []
    //     }
    //   },
    //   "counter2": {"SSE": "http://localhost:8000/sse"}
    // }
    // ```
    let content = fs::read_to_string(filename)?;
    let servers: HashMap<String, MCPTransport> = serde_json::from_str(&content)?;

    // Initialize the tracing subscriber with file and stdout logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting MCP server");
    let mux = build_mux(&servers).await?;

    // Create an instance of our counter router
    let service = mux.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;

    service.waiting().await?;
    Ok(())
}
