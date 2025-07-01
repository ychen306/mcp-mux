use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{self, EnvFilter};
use tokio::process::Command;
use std::collections::HashMap;
use std::borrow::Cow;

///// Counter
use std::sync::Arc;
use serde_json::json;
use tokio::sync::Mutex;

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

use mcp_mux::{MCPMux, MCPServer, build_mux};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the tracing subscriber with file and stdout logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let mut servers = HashMap::new();

    servers.insert("mycounter".to_string(),
        MCPServer {
            cmd: "/Users/tom/workspace/mcp-rust-sdk/target/debug/examples/servers_counter_stdio".to_string(),
            args: vec![],
        });
    servers.insert("another-counter".to_string(),
        MCPServer {
            cmd: "/Users/tom/workspace/mcp-rust-sdk/target/debug/examples/servers_counter_stdio".to_string(),
            args: vec![],
        });

    /*
    for (name, server) in &servers {
        let client = ()
        .serve(TokioChildProcess::new(Command::new(server.cmd.clone()).configure(
            |cmd| { cmd.args(server.args.clone()); },
        ))?)
        .await?;
        let server_info = client.peer_info();
        tracing::info!("Connected to {name:#?}: {server_info:#?}");
        let list_result = client.list_tools(Default::default()).await?;
        tools.insert(name.to_string(), list_result.tools);
        clients.insert(name.to_string(), Arc::new(client));
    }

    tracing::info!("Starting MCP server");
    */
    let mux = build_mux(&servers).await?;

    // Create an instance of our counter router
    let service = mux.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e); 
    })?;

    service.waiting().await?;
    Ok(())
}
