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

struct MCPServer {
    cmd: String,
    args: Vec<String>,
}

///////////
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct StructRequest {
    pub a: i32,
    pub b: i32,
}

// FIXME: maybe this shouldn't be cloneable??
#[derive(Clone)]
pub struct MCPMux {
    // FIXME: remove this
    counter: Arc<Mutex<i32>>,

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
            counter: Arc::new(Mutex::new(0)),
            tool_router: Self::tool_router(),
            tools : tools,
            clients : clients,
        }
    }

    fn _create_resource_text(&self, uri: &str, name: &str) -> Resource {
        RawResource::new(uri, name.to_string()).no_annotation()
    }
}

impl ServerHandler for MCPMux {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_prompts()
                .enable_resources()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("This server provides a counter tool that can increment and decrement values. The counter starts at 0 and can be modified using the 'increment' and 'decrement' tools. Use 'get_value' to check the current count.".to_string()),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![
                self._create_resource_text("str:////Users/to/some/path/", "cwd"),
                self._create_resource_text("memo://insights", "memo-name"),
            ],
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        ReadResourceRequestParam { uri }: ReadResourceRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        match uri.as_str() {
            "str:////Users/to/some/path/" => {
                let cwd = "/Users/to/some/path/";
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(cwd, uri)],
                })
            }
            "memo://insights" => {
                let memo = "Business Intelligence Memo\n\nAnalysis has revealed 5 key insights ...";
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(memo, uri)],
                })
            }
            _ => Err(McpError::resource_not_found(
                "resource_not_found",
                Some(json!({
                    "uri": uri
                })),
            )),
        }
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult {
            next_cursor: None,
            prompts: vec![Prompt::new(
                "example_prompt",
                Some("This is an example prompt that takes one required argument, message"),
                Some(vec![PromptArgument {
                    name: "message".to_string(),
                    description: Some("A message to put in the prompt".to_string()),
                    required: Some(true),
                }]),
            )],
        })
    }

    async fn get_prompt(
        &self,
        GetPromptRequestParam { name, arguments }: GetPromptRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        match name.as_str() {
            "example_prompt" => {
                let message = arguments
                    .and_then(|json| json.get("message")?.as_str().map(|s| s.to_string()))
                    .ok_or_else(|| {
                        McpError::invalid_params("No message provided to example_prompt", None)
                    })?;

                let prompt =
                    format!("This is an example prompt with your message here: '{message}'");
                Ok(GetPromptResult {
                    description: None,
                    messages: vec![PromptMessage {
                        role: PromptMessageRole::User,
                        content: PromptMessageContent::text(prompt),
                    }],
                })
            }
            _ => Err(McpError::invalid_params("prompt not found", None)),
        }
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            next_cursor: None,
            resource_templates: Vec::new(),
        })
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


        //match request.name.as_ref() {
        //}
        //Ok(CallToolResult::success(vec![
                //Content::text(request.name),
        //]))
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
///////////

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the tracing subscriber with file and stdout logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    // name -> server
    let mut servers = HashMap::new();
    // name -> client
    let mut clients = HashMap::new();
    // name -> tools
    let mut tools = HashMap::new();

    servers.insert("mycounter",
        MCPServer {
            cmd: "/Users/tom/workspace/mcp-rust-sdk/target/debug/examples/servers_counter_stdio".to_string(),
            args: vec![],
        });

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

    // Create an instance of our counter router
    let service = MCPMux::new(tools, clients).serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e); 
    })?;

    service.waiting().await?;
    Ok(())
}
