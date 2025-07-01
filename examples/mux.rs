use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{self, EnvFilter};
use tokio::process::Command;
use std::collections::HashMap;
use std::borrow::Cow;
use std::sync::Arc;
use serde_json::json;
use serde::Deserialize;
use rmcp::{
    Error as McpError, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::Parameters},
    model::*,
    schemars,
    service::{RequestContext, RunningService},
    RoleClient,
    tool, tool_handler, tool_router,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::fs;
use std::env;

use mcp_mux::{MCPMux, MCPTransport, build_mux};

#[tokio::main]
async fn main() -> Result<()> {
    //let filename = env::args().nth(1).expect("Usage: mux <servers.json>");
    //// Example servers.json:
    //// ```
    //// {
    ////   "counter": {
    ////     "cmd": "/Users/tom/workspace/mcp-rust-sdk/target/debug/examples/servers_counter_stdio",
    ////     "args": []
    ////   },
    ////   "another": {
    ////     "cmd": "/Users/tom/workspace/mcp-rust-sdk/target/debug/examples/servers_counter_stdio",
    ////     "args": []
    ////   }
    //// }
    //// ```
    //let content = fs::read_to_string(filename)?;
    //let servers: HashMap<String, MCPTransport> = serde_json::from_str(&content)?;
    let mut servers = HashMap::new();
    servers.insert("counter1".to_string(), MCPTransport::Stdio{
        cmd: "/Users/tom/workspace/mcp-rust-sdk/target/debug/examples/servers_counter_stdio".to_string(),
        args: vec![]
    });
    servers.insert("sse-counter".to_string(), MCPTransport::SSE("http://localhost:8000/sse".to_string()));

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
