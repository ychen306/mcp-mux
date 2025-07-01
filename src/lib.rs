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
    service::{RequestContext, RunningService, ServiceError},
    RoleClient,
    tool, tool_handler, tool_router,
    transport::{ConfigureCommandExt, TokioChildProcess, SseClientTransport},
};

#[derive(Deserialize)]
pub enum MCPTransport {
    // TODO: maybe renamed to SubProcess or something
    Stdio { cmd: String, args: Vec<String> },
    SSE(String),
}

enum MuxClient {
    Stdio(RunningService<RoleClient, ()>),
    SSE(RunningService<RoleClient, rmcp::model::InitializeRequestParam>)
}

async fn call_tool(mc : &MuxClient, request : CallToolRequestParam) -> Result<CallToolResult, ServiceError> {
    match mc {
        MuxClient::Stdio(client) => client.call_tool(request).await,
        MuxClient::SSE(client) => client.call_tool(request).await,
    }
}

fn peer_info(mc : &MuxClient) -> Option<&ServerInfo> {
    match mc {
        MuxClient::Stdio(client) => client.peer_info(),
        MuxClient::SSE(client) => client.peer_info(),
    }

}

async fn list_tools(mc : &MuxClient) -> Result<ListToolsResult, ServiceError> {
    match mc {
        MuxClient::Stdio(client) => client.list_tools(Default::default()).await,
        MuxClient::SSE(client) => client.list_tools(Default::default()).await,
    }
}

// FIXME: maybe this shouldn't be cloneable??
#[derive(Clone)]
pub struct MCPMux {
    // TODO: maybe merge the fields into a single MCPService struct??
    tools : HashMap<String, Vec<Tool>>,
    clients : HashMap<String, Arc<MuxClient>>,
    tool_router: ToolRouter<MCPMux>,
}

#[tool_router]
impl MCPMux {
    #[allow(dead_code)]
    pub fn new(tools : HashMap<String, Vec<Tool>>,
        clients : HashMap<String, Arc<MuxClient>>) -> Self {
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
        match call_tool(client, new_request).await {
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

async fn build_client(transport : &MCPTransport) -> Result<MuxClient> {
    match transport {
        MCPTransport::Stdio{cmd, args} => {
            let client = ()
                .serve(TokioChildProcess::new(Command::new(cmd.clone()).configure(
                            |cmd| { cmd.args(args.clone()); },
                ))?)
                .await?;
            return Ok(MuxClient::Stdio(client));
        }
        MCPTransport::SSE(uri) => {
            let transport = SseClientTransport::start(Arc::<str>::from(uri.to_string())).await?;
            // TODO: make this an argument chosen by the user
            let client_info = ClientInfo {
                protocol_version: Default::default(),
                capabilities: ClientCapabilities::default(),
                client_info: Implementation {
                    name: "test sse client".to_string(),
                    version: "0.0.1".to_string(),
                },
            };
            let client = client_info.serve(transport).await.inspect_err(|e| {
                tracing::error!("client error: {:?}", e);
            })?;
            return Ok(MuxClient::SSE(client));
        }
    }
}

pub async fn build_mux(servers : &HashMap<String, MCPTransport>) -> Result<MCPMux> {
    // name -> client
    let mut clients = HashMap::new();
    // name -> tools
    let mut tools = HashMap::new();

    for (name, transport) in servers {
        let client = build_client(transport).await?;
        let server_info = peer_info(&client);
        tracing::info!("Connected to {name:#?}: {server_info:#?}");
        let list_result = list_tools(&client).await?;
        tools.insert(name.to_string(), list_result.tools);
        clients.insert(name.to_string(), Arc::new(client));
    }

    Ok(MCPMux::new(tools, clients))
}
