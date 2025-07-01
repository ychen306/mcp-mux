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

pub struct MCPServer {
    pub cmd: String,
    pub args: Vec<String>,
}

// FIXME: maybe this shouldn't be cloneable??
#[derive(Clone)]
pub struct MCPMux {
    // TODO: maybe merge the fields into a single MCPService struct??
    tools : HashMap<String, Vec<Tool>>,
    clients : HashMap<String, Arc<RunningService<RoleClient, ()>>>,
    tool_router: ToolRouter<MCPMux>,
}

#[tool_router]
impl MCPMux {
    #[allow(dead_code)]
    pub fn new(tools : HashMap<String, Vec<Tool>>,
        clients : HashMap<String, Arc<RunningService<RoleClient, ()>>>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            tools : tools,
            clients : clients,
        }
    }
}

impl ServerHandler for MCPMux {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("this is a mux".to_string()),
        }
    }


    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut aggregated = Vec::new();
        for (server_name, server_tools) in &self.tools {
            for tool in server_tools {
                let mut new_tool = tool.clone();
                new_tool.name = Cow::Owned(format!("{server_name}::{}", tool.name));
                aggregated.push(new_tool);
            }
        }
        Ok(ListToolsResult { tools : aggregated, next_cursor: None, })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let mut parts = request.name.splitn(2, "::");
        let maybe_server = parts.next();
        let maybe_tool = parts.next();

        // FIXME: should we return ErrorData instead?
        if maybe_server.is_none() || maybe_tool.is_none() {
            return Ok(CallToolResult::error(vec![Content::text("error parsing tool identifier")]));
        }

        let server_name = maybe_server.unwrap();
        let tool_name = maybe_tool.unwrap();

        // Check that the tool exists
        if let Some(server_tools) = self.tools.get(server_name) {
            if !server_tools.iter().any(|t| t.name == tool_name) {
                return Ok(CallToolResult::error(vec![Content::text(format!("unknown tool {}", tool_name))]));
            }
        } else {
            return Ok(CallToolResult::error(vec![Content::text("unknown server")]));
        }

        assert!(self.clients.contains_key(server_name));
        let client = self.clients.get(server_name).unwrap();

        let mut new_request = request.clone();
        new_request.name = tool_name.to_string().into();
        match client.call_tool(new_request).await {
            Ok(result) => Ok(result),
            Err(e) => Err(ErrorData::new(
                            ErrorCode::INTERNAL_ERROR,
                            format!("failed to use tool {}", e),
                            None)),
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if let Some(http_request_part) = context.extensions.get::<axum::http::request::Parts>() {
            let initialize_headers = &http_request_part.headers;
            let initialize_uri = &http_request_part.uri;
            tracing::info!(?initialize_headers, %initialize_uri, "initialize from http server");
        }
        Ok(self.get_info())
    }
}

pub async fn build_mux(servers : &HashMap<String, MCPServer>) -> Result<MCPMux> {
    // name -> client
    let mut clients = HashMap::new();
    // name -> tools
    let mut tools = HashMap::new();

    for (name, server) in servers {
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

    Ok(MCPMux::new(tools, clients))
}
