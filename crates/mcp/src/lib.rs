//! MCP (Model Context Protocol) bridge for MAE.
//!
//! @stability: stable
//! @since: 0.6.0
//!
//! Exposes the editor's tools via JSON-RPC over a Unix domain socket.
//! Claude Code (or any MCP client) connects via the mae-mcp-shim binary
//! which bridges stdio <-> the socket.
//!
//! ## Multi-client support (v0.11.0+)
//!
//! The server accepts multiple concurrent clients, each in its own tokio
//! task with a `ClientSession`. Messages use Content-Length framing
//! (LSP-compatible) with automatic fallback to line-based framing for
//! backward compatibility with existing `mae-mcp-shim` clients.

pub mod broadcast;
pub mod client;
pub mod client_mgr;
pub mod protocol;
pub mod session;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use protocol::{
    ContentItem, InitializeResult, JsonRpcRequest, JsonRpcResponse, McpError, ServerCapabilities,
    ToolCallResult, ToolInfo,
};
use session::{ClientInfo, ClientSession};
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

impl std::fmt::Debug for McpToolRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpToolRequest")
            .field("tool_name", &self.tool_name)
            .finish_non_exhaustive()
    }
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
    ///
    /// Supports multiple concurrent clients. Each client gets its own
    /// session and tokio task. Content-Length framing is used for responses;
    /// reads auto-detect Content-Length vs line-based framing.
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

        info!(path = %self.socket_path.display(), "MCP server listening (multi-client)");

        let tool_defs = Arc::new(tool_definitions);

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let session = ClientSession::new();
                    let session_id = session.id;
                    info!(session = session_id, "MCP client connected");

                    let tool_tx = self.tool_tx.clone();
                    let tool_defs = Arc::clone(&tool_defs);

                    tokio::spawn(async move {
                        handle_client(stream, tool_tx, &tool_defs, session).await;
                        info!(session = session_id, "MCP client session ended");
                    });
                }
                Err(e) => {
                    error!(error = %e, "MCP accept error");
                }
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

// ---------------------------------------------------------------------------
// Per-client connection handler
// ---------------------------------------------------------------------------

/// Handle a single client connection in its own task.
async fn handle_client(
    stream: tokio::net::UnixStream,
    tool_tx: mpsc::Sender<McpToolRequest>,
    tool_definitions: &[ToolInfo],
    mut session: ClientSession,
) {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut writer = writer;
    let write_timeout = std::time::Duration::from_secs(5);

    loop {
        let msg = match read_message(&mut reader).await {
            Ok(Some(msg)) => msg,
            Ok(None) => {
                debug!(session = session.id, "MCP client disconnected (EOF)");
                break;
            }
            Err(e) => {
                error!(session = session.id, error = %e, "MCP read error");
                break;
            }
        };

        session.touch();

        let response = handle_request(&msg, tool_definitions, &tool_tx, &mut session).await;
        let body = match serde_json::to_vec(&response) {
            Ok(b) => b,
            Err(e) => {
                error!(session = session.id, error = %e, "failed to serialize response");
                continue;
            }
        };

        // Content-Length framed write with timeout.
        let write_result = tokio::time::timeout(write_timeout, async {
            let header = format!("Content-Length: {}\r\n\r\n", body.len());
            writer.write_all(header.as_bytes()).await?;
            writer.write_all(&body).await?;
            writer.flush().await
        })
        .await;

        match write_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                error!(session = session.id, error = %e, "write error; closing client");
                break;
            }
            Err(_) => {
                warn!(session = session.id, "write timeout; closing slow client");
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Message framing (Content-Length + line-based fallback)
// ---------------------------------------------------------------------------

/// Read a single JSON-RPC message from the stream.
///
/// Auto-detects framing:
/// - If the stream starts with `Content-Length:`, reads the header and then
///   exactly that many bytes of body (LSP-compatible framing).
/// - Otherwise, reads a single line (legacy line-based framing).
///
/// Returns `Ok(None)` on clean EOF.
async fn read_message<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<String>, std::io::Error> {
    // Peek at the buffer to determine framing mode.
    let buf = reader.fill_buf().await?;
    if buf.is_empty() {
        return Ok(None); // EOF
    }

    // Check if this looks like Content-Length framing.
    if buf.len() >= 15 && buf.starts_with(b"Content-Length:") {
        // Read header lines until we hit the empty \r\n separator.
        let mut content_length: Option<usize> = None;
        loop {
            let mut header_line = String::new();
            let n = reader.read_line(&mut header_line).await?;
            if n == 0 {
                return Ok(None); // EOF mid-header
            }
            let trimmed = header_line.trim();
            if trimmed.is_empty() {
                break; // End of headers
            }
            if let Some(val) = trimmed.strip_prefix("Content-Length:") {
                content_length = val.trim().parse().ok();
            }
        }

        let len = content_length
            .ok_or_else(|| std::io::Error::other("Content-Length header missing value"))?;

        let mut body = vec![0u8; len];
        tokio::io::AsyncReadExt::read_exact(reader, &mut body).await?;
        let msg = String::from_utf8(body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Some(msg))
    } else {
        // Legacy line-based framing. Skip blank lines.
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None);
            }
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Request dispatch
// ---------------------------------------------------------------------------

/// Process a single JSON-RPC request, updating session state as needed.
async fn handle_request(
    msg: &str,
    tool_definitions: &[ToolInfo],
    tool_tx: &mpsc::Sender<McpToolRequest>,
    session: &mut ClientSession,
) -> JsonRpcResponse {
    let request: JsonRpcRequest = match serde_json::from_str(msg) {
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
            // Extract client info if provided.
            if let Some(ref params) = request.params {
                if let Some(client_info) = params.get("clientInfo") {
                    session.client_info = Some(ClientInfo {
                        name: client_info
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        version: client_info
                            .get("version")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    });
                }
            }

            info!(
                session = session.id,
                client = session.display_name(),
                "MCP initialize handshake"
            );

            let result = InitializeResult {
                protocol_version: "2024-11-05".to_string(),
                capabilities: ServerCapabilities {
                    tools: Some(serde_json::json!({})),
                },
                server_info: serde_json::json!({
                    "name": "mae-editor",
                    "version": env!("CARGO_PKG_VERSION"),
                    "features": {
                        "multiClient": true,
                        "contentLengthFraming": true,
                        "stateNotifications": true,
                    },
                }),
            };
            JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
        }
        "notifications/initialized" => {
            session.initialized = true;
            debug!(session = session.id, "client initialized");
            JsonRpcResponse::success(id, serde_json::Value::Null)
        }
        "$/ping" => {
            session.touch();
            JsonRpcResponse::success(id, serde_json::json!("pong"))
        }
        "notifications/subscribe" => {
            if let Some(ref params) = request.params {
                if let Some(types) = params.get("types").and_then(|v| v.as_array()) {
                    for t in types {
                        if let Some(s) = t.as_str() {
                            session.subscriptions.insert(s.to_string());
                        }
                    }
                    debug!(
                        session = session.id,
                        subscriptions = ?session.subscriptions,
                        "client subscribed to events"
                    );
                }
            }
            JsonRpcResponse::success(id, serde_json::Value::Null)
        }
        "shutdown" => {
            info!(session = session.id, "client requested shutdown");
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

            if tool_tx.send(req).await.is_err() {
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
            warn!(method = other, session = session.id, "unknown MCP method");
            JsonRpcResponse::error(
                id,
                McpError::method_not_found(format!("Unknown method: {}", other)),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_message_line_based() {
        let data = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"test\"}\n";
        let mut reader = BufReader::new(&data[..]);
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert!(msg.contains("test"));
    }

    #[tokio::test]
    async fn read_message_content_length() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"test"}"#;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let data = format!("{}{}", header, body);
        let mut reader = BufReader::new(data.as_bytes());
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert!(msg.contains("test"));
    }

    #[tokio::test]
    async fn read_message_eof() {
        let data = b"";
        let mut reader = BufReader::new(&data[..]);
        assert!(read_message(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn read_message_skips_blank_lines() {
        let data = b"\n\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"test\"}\n";
        let mut reader = BufReader::new(&data[..]);
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert!(msg.contains("test"));
    }

    #[tokio::test]
    async fn handle_request_initialize_extracts_client_info() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"test-client","version":"0.1"}}}"#;

        let resp = handle_request(msg, &[], &tx, &mut session).await;
        assert!(resp.result.is_some());
        assert_eq!(session.client_info.as_ref().unwrap().name, "test-client");
    }

    #[tokio::test]
    async fn handle_request_ping() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let msg = r#"{"jsonrpc":"2.0","id":2,"method":"$/ping"}"#;

        let resp = handle_request(msg, &[], &tx, &mut session).await;
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn handle_request_subscribe() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let msg = r#"{"jsonrpc":"2.0","id":3,"method":"notifications/subscribe","params":{"types":["buffer_edit","diagnostics"]}}"#;

        let resp = handle_request(msg, &[], &tx, &mut session).await;
        assert!(resp.result.is_some());
        assert!(session.subscriptions.contains("buffer_edit"));
        assert!(session.subscriptions.contains("diagnostics"));
    }

    #[tokio::test]
    async fn handle_request_tools_list() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let tools = vec![ToolInfo {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        let msg = r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#;

        let resp = handle_request(msg, &tools, &tx, &mut session).await;
        let result = resp.result.unwrap();
        let tools_arr = result["tools"].as_array().unwrap();
        assert_eq!(tools_arr.len(), 1);
        assert_eq!(tools_arr[0]["name"], "test_tool");
    }

    #[tokio::test]
    async fn content_length_framing_round_trip() {
        // Simulate writing a Content-Length framed response and reading it back.
        let response =
            JsonRpcResponse::success(serde_json::json!(1), serde_json::json!({"result": "ok"}));
        let body = serde_json::to_vec(&response).unwrap();
        let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        framed.extend_from_slice(&body);

        let mut reader = BufReader::new(&framed[..]);
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        let parsed: JsonRpcResponse = serde_json::from_str(&msg).unwrap();
        assert!(parsed.result.is_some());
    }

    // -----------------------------------------------------------------------
    // Multi-client integration tests
    // -----------------------------------------------------------------------

    /// Helper: send a JSON-RPC message over a Unix socket using line framing
    /// and read back a Content-Length framed response.
    async fn send_and_recv(
        stream: &mut tokio::net::UnixStream,
        msg: &serde_json::Value,
    ) -> JsonRpcResponse {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let payload = serde_json::to_string(msg).unwrap();
        stream
            .write_all(format!("{}\n", payload).as_bytes())
            .await
            .unwrap();
        stream.flush().await.unwrap();

        // Read Content-Length framed response.
        let mut header_buf = Vec::new();
        let mut byte = [0u8; 1];
        // Read until we hit \r\n\r\n.
        loop {
            stream.read_exact(&mut byte).await.unwrap();
            header_buf.push(byte[0]);
            if header_buf.len() >= 4 && &header_buf[header_buf.len() - 4..] == b"\r\n\r\n" {
                break;
            }
        }
        let header = String::from_utf8(header_buf).unwrap();
        let content_length: usize = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        let mut body = vec![0u8; content_length];
        stream.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn multi_client_concurrent_connections() {
        let socket_path = format!("/tmp/mae-test-multi-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        // Set up the server with a mock tool handler.
        let (tool_tx, mut tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let server = McpServer::new(&socket_path, tool_tx);
        let tools = vec![ToolInfo {
            name: "echo".to_string(),
            description: "Echo tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }];

        // Spawn the server.
        tokio::spawn(async move {
            server.run(tools).await;
        });

        // Spawn a mock tool handler that echoes the tool name back.
        tokio::spawn(async move {
            while let Some(req) = tool_rx.recv().await {
                let _ = req.reply.send(McpToolResult {
                    success: true,
                    output: format!("echoed: {}", req.tool_name),
                });
            }
        });

        // Give server time to bind.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // --- Client 1 connects ---
        let mut client1 = tokio::net::UnixStream::connect(&socket_path)
            .await
            .expect("client1 connect");

        // Client 1: initialize
        let resp = send_and_recv(
            &mut client1,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "client-1", "version": "1.0"}}
            }),
        )
        .await;
        assert!(resp.error.is_none(), "client1 initialize failed");
        let result = resp.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "mae-editor");
        // Verify multiClient capability is advertised.
        assert_eq!(result["serverInfo"]["features"]["multiClient"], true);

        // --- Client 2 connects while client 1 is still connected ---
        let mut client2 = tokio::net::UnixStream::connect(&socket_path)
            .await
            .expect("client2 connect");

        // Client 2: initialize
        let resp = send_and_recv(
            &mut client2,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "client-2"}}
            }),
        )
        .await;
        assert!(resp.error.is_none(), "client2 initialize failed");

        // Both clients: tools/list
        let resp1 = send_and_recv(
            &mut client1,
            &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        )
        .await;
        let resp2 = send_and_recv(
            &mut client2,
            &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        )
        .await;
        let tools1 = resp1.result.unwrap()["tools"].as_array().unwrap().len();
        let tools2 = resp2.result.unwrap()["tools"].as_array().unwrap().len();
        assert_eq!(tools1, 1);
        assert_eq!(tools2, 1);

        // Client 1: ping
        let resp = send_and_recv(
            &mut client1,
            &serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "$/ping"}),
        )
        .await;
        assert_eq!(resp.result.unwrap(), "pong");

        // Client 2: tool call
        let resp = send_and_recv(
            &mut client2,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "echo", "arguments": {}}
            }),
        )
        .await;
        let result = resp.result.unwrap();
        assert_eq!(result["content"][0]["text"], "echoed: echo");

        // --- Disconnect client 1, client 2 should still work ---
        drop(client1);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Client 2: still alive — ping works
        let resp = send_and_recv(
            &mut client2,
            &serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "$/ping"}),
        )
        .await;
        assert_eq!(resp.result.unwrap(), "pong");

        // Client 2: tool call still works after client 1 dropped
        let resp = send_and_recv(
            &mut client2,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 5, "method": "tools/call",
                "params": {"name": "echo", "arguments": {}}
            }),
        )
        .await;
        assert_eq!(resp.result.unwrap()["content"][0]["text"], "echoed: echo");

        // Clean up.
        drop(client2);
        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn multi_client_subscribe_events() {
        let socket_path = format!("/tmp/mae-test-subscribe-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let server = McpServer::new(&socket_path, tool_tx);

        tokio::spawn(async move {
            server.run(vec![]).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path)
            .await
            .expect("connect");

        // Initialize.
        send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "sub-test"}}
            }),
        )
        .await;

        // Subscribe to events.
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "notifications/subscribe",
                "params": {"types": ["buffer_edit", "mode_change"]}
            }),
        )
        .await;
        assert!(resp.error.is_none());

        // Shutdown.
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown"}),
        )
        .await;
        assert!(resp.error.is_none());

        drop(client);
        let _ = std::fs::remove_file(&socket_path);
    }
}
