//! MCP (Model Context Protocol) bridge for MAE.
//!
//! Exposes the editor's tools via JSON-RPC over a Unix domain socket.
//! Claude Code (or any MCP client) connects via the mae-mcp-shim binary
//! which bridges stdio <-> the socket.

pub mod protocol;

use std::path::{Path, PathBuf};

use protocol::{
    ContentItem, InitializeResult, JsonRpcRequest, JsonRpcResponse, McpError, ServerCapabilities,
    ToolCallResult, ToolInfo,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};

/// A tool call request sent from the MCP server to the main editor thread.
pub struct McpToolRequest {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub reply: oneshot::Sender<McpToolResult>,
}

/// Result of a tool call, sent back from the editor thread.
pub struct McpToolResult {
    pub success: bool,
    pub output: String,
}

/// MCP server configuration.
pub struct McpServer {
    socket_path: PathBuf,
    tool_tx: mpsc::Sender<McpToolRequest>,
}

impl McpServer {
    pub fn new(socket_path: impl Into<PathBuf>, tool_tx: mpsc::Sender<McpToolRequest>) -> Self {
        McpServer {
            socket_path: socket_path.into(),
            tool_tx,
        }
    }

    /// Run the MCP server, accepting connections on the Unix socket.
    /// This should be spawned as a tokio task.
    pub async fn run(self, tool_definitions: Vec<ToolInfo>) {
        // Clean up stale socket file
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = match UnixListener::bind(&self.socket_path) {
            Ok(l) => l,
            Err(e) => {
                error!(path = %self.socket_path.display(), error = %e, "failed to bind MCP socket");
                return;
            }
        };

        info!(path = %self.socket_path.display(), "MCP server listening");

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    debug!("MCP client connected");
                    let (reader, writer) = stream.into_split();
                    let mut reader = BufReader::new(reader);
                    let mut writer = writer;

                    loop {
                        let mut line = String::new();
                        match reader.read_line(&mut line).await {
                            Ok(0) => {
                                debug!("MCP client disconnected");
                                break;
                            }
                            Ok(_) => {
                                let line = line.trim();
                                if line.is_empty() {
                                    continue;
                                }
                                let response = self.handle_message(line, &tool_definitions).await;
                                let response_json = match serde_json::to_string(&response) {
                                    Ok(j) => j,
                                    Err(e) => {
                                        error!(error = %e, "failed to serialize MCP response");
                                        continue;
                                    }
                                };
                                if let Err(e) = writer.write_all(response_json.as_bytes()).await {
                                    error!(error = %e, "failed to write MCP response");
                                    break;
                                }
                                if let Err(e) = writer.write_all(b"\n").await {
                                    error!(error = %e, "failed to write newline");
                                    break;
                                }
                                let _ = writer.flush().await;
                            }
                            Err(e) => {
                                error!(error = %e, "MCP read error");
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "MCP accept error");
                }
            }
        }
    }

    async fn handle_message(&self, line: &str, tool_definitions: &[ToolInfo]) -> JsonRpcResponse {
        let request: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                return JsonRpcResponse::error(
                    serde_json::Value::Null,
                    McpError::parse_error(format!("Invalid JSON: {}", e)),
                );
            }
        };

        let id = request.id.clone();

        match request.method.as_str() {
            "initialize" => {
                let result = InitializeResult {
                    protocol_version: "2024-11-05".to_string(),
                    capabilities: ServerCapabilities {
                        tools: Some(serde_json::json!({})),
                    },
                    server_info: serde_json::json!({
                        "name": "mae-editor",
                        "version": env!("CARGO_PKG_VERSION"),
                    }),
                };
                JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
            }
            "notifications/initialized" => {
                // Ack, no response needed for notifications -- but we still
                // return a response since the client may expect one.
                JsonRpcResponse::success(id, serde_json::Value::Null)
            }
            "tools/list" => {
                let tools: Vec<serde_json::Value> = tool_definitions
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": t.input_schema,
                        })
                    })
                    .collect();
                JsonRpcResponse::success(id, serde_json::json!({ "tools": tools }))
            }
            "tools/call" => {
                let params = request.params.unwrap_or(serde_json::Value::Null);
                let tool_name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));

                let (reply_tx, reply_rx) = oneshot::channel();
                let req = McpToolRequest {
                    tool_name: tool_name.clone(),
                    arguments,
                    reply: reply_tx,
                };

                if self.tool_tx.send(req).await.is_err() {
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error("Editor channel closed".to_string()),
                    );
                }

                match reply_rx.await {
                    Ok(result) => {
                        let call_result = ToolCallResult {
                            content: vec![ContentItem {
                                content_type: "text".to_string(),
                                text: result.output,
                            }],
                            is_error: Some(!result.success),
                        };
                        JsonRpcResponse::success(id, serde_json::to_value(call_result).unwrap())
                    }
                    Err(_) => JsonRpcResponse::error(
                        id,
                        McpError::internal_error("Tool execution cancelled".to_string()),
                    ),
                }
            }
            other => {
                warn!(method = other, "unknown MCP method");
                JsonRpcResponse::error(
                    id,
                    McpError::method_not_found(format!("Unknown method: {}", other)),
                )
            }
        }
    }

    /// Socket path for this server.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
