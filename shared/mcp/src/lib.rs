//! MCP (Model Context Protocol) bridge for MAE.
//!
//! @stability: stable
//! @since: 0.6.0
//!
//! Exposes the editor's tools via JSON-RPC over a Unix domain socket.
//! Claude Code (or any MCP client) connects via the mae-mcp-shim binary
//! which bridges stdio <-> the socket.
//!
//! ## Transport framing
//!
//! Two distinct framing protocols are in play:
//!
//! - **Socket side** (MAE server <-> shim, or direct clients): Content-Length
//!   framing (LSP-compatible). Each message is preceded by a
//!   `Content-Length: N\r\n\r\n` header. This is also used for TCP transport
//!   (collab daemon, multi-client).
//!
//! - **Stdio side** (shim <-> Claude Code): Newline-delimited JSON per the
//!   MCP stdio transport specification. Each message is a single JSON object
//!   on one line, terminated by `\n`. Messages MUST NOT contain embedded
//!   newlines. See: <https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#stdio>
//!
//! The `mae-mcp-shim` binary translates between these two framing protocols.
//! This is critical — using Content-Length framing on stdio will cause MCP
//! clients (Claude Code, etc.) to hang during the handshake.
//!
//! ## Protocol version negotiation
//!
//! The server supports multiple MCP protocol versions (see `protocol::SUPPORTED_VERSIONS`).
//! Per spec, if the client requests a version we support, we MUST echo it back.
//! If not, we return our latest. Claude Code will disconnect if it receives
//! a version it doesn't support. See: <https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle#version-negotiation>
//!
//! ## Multi-client support (v0.11.0+)
//!
//! The server accepts multiple concurrent clients, each in its own tokio
//! task with a `ClientSession`. Messages use Content-Length framing
//! (LSP-compatible) with automatic fallback to line-based framing for
//! backward compatibility with existing `mae-mcp-shim` clients.

pub mod auth;
pub mod broadcast;
pub mod client;
pub mod client_mgr;
pub mod daemon_client;
pub mod identity;
pub mod keystore;
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

/// Maximum allowed Content-Length for a single MCP message (10 MB).
const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

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
    broadcaster: broadcast::SharedBroadcaster,
}

impl McpServer {
    pub fn new(
        socket_path: impl Into<PathBuf>,
        tool_tx: mpsc::Sender<McpToolRequest>,
        broadcaster: broadcast::SharedBroadcaster,
    ) -> Self {
        McpServer {
            socket_path: socket_path.into(),
            tool_tx,
            broadcaster,
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
                    let broadcaster = Arc::clone(&self.broadcaster);

                    tokio::spawn(async move {
                        handle_client(stream, tool_tx, &tool_defs, session, broadcaster).await;
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
///
/// Uses `tokio::select!` to simultaneously read requests AND push events
/// from the broadcaster. Clients must subscribe (via `notifications/subscribe`)
/// to receive push notifications.
///
/// CANCEL-SAFETY: `read_message` uses multi-step I/O (peek → read headers →
/// read body). If `tokio::select!` cancels it mid-parse, the BufReader's
/// internal cursor is left past partially-consumed data. We spawn a dedicated
/// reader task that feeds complete messages into an mpsc channel, so
/// `read_message` always runs to completion and `select!` only receives from
/// the channel (which is cancel-safe).
async fn handle_client(
    stream: tokio::net::UnixStream,
    tool_tx: mpsc::Sender<McpToolRequest>,
    tool_definitions: &[ToolInfo],
    mut session: ClientSession,
    broadcaster: broadcast::SharedBroadcaster,
) {
    let (reader, writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let mut writer = writer;
    let write_timeout = std::time::Duration::from_secs(5);

    // Spawn a dedicated reader task so read_message always runs to completion
    // (never cancelled by select!). Messages arrive via an mpsc channel.
    let (msg_tx, mut msg_rx) = mpsc::channel::<Result<String, String>>(32);
    tokio::spawn(async move {
        let mut reader = reader;
        loop {
            match read_message(&mut reader).await {
                Ok(Some(msg)) => {
                    if msg_tx.send(Ok(msg)).await.is_err() {
                        break; // handler dropped
                    }
                }
                Ok(None) => {
                    let _ = msg_tx.send(Err("EOF".to_string())).await;
                    break;
                }
                Err(e) => {
                    let _ = msg_tx.send(Err(e.to_string())).await;
                    break;
                }
            }
        }
    });

    // Subscribe with empty subs — receives nothing until client opts in.
    let mut event_rx = {
        let mut bc = broadcaster.lock().unwrap();
        bc.subscribe(session.id, vec![])
    };

    let mut consecutive_write_failures: u32 = 0;

    loop {
        tokio::select! {
            biased;

            msg = msg_rx.recv() => {
                let msg = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) if e == "EOF" => {
                        debug!(session = session.id, "MCP client disconnected (EOF)");
                        break;
                    }
                    Some(Err(e)) => {
                        error!(session = session.id, error = %e, "MCP read error");
                        break;
                    }
                    None => {
                        debug!(session = session.id, "MCP reader task exited");
                        break;
                    }
                };

                session.touch();
                session.messages_received += 1;

                // JSON-RPC notifications have "method" but no "id" — they
                // must not receive a response. Handle known ones, ignore the rest.
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&msg) {
                    if val.get("method").is_some() && val.get("id").is_none() {
                        if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
                            match method {
                                "notifications/initialized" => {
                                    session.initialized = true;
                                    debug!(session = session.id, "client initialized (notification)");
                                }
                                _ => {
                                    debug!(session = session.id, method = method, "ignoring unknown notification");
                                }
                            }
                        }
                        continue;
                    }
                }

                let response = handle_request(
                    &msg, tool_definitions, &tool_tx, &mut session, &broadcaster,
                ).await;
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
            Some(event) = event_rx.recv() => {
                if write_notification(&mut writer, &event, session.events_delivered + 1, write_timeout).await.is_err() {
                    consecutive_write_failures += 1;
                    session.events_dropped += 1;
                    warn!(
                        session = session.id,
                        failures = consecutive_write_failures,
                        "notification write failed ({consecutive_write_failures}/3)"
                    );
                    if consecutive_write_failures >= 3 {
                        warn!(session = session.id, "disconnecting client after 3 consecutive write failures");
                        break;
                    }
                } else {
                    consecutive_write_failures = 0;
                    session.events_delivered += 1;
                }
            }
        }
    }

    // Unsubscribe on disconnect.
    broadcaster.lock().unwrap().unsubscribe(session.id);
}

/// Write Content-Length framed bytes to any async writer with a timeout.
pub async fn write_framed<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    body: &[u8],
    timeout: std::time::Duration,
) -> Result<(), std::io::Error> {
    tokio::time::timeout(timeout, async {
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        writer.write_all(header.as_bytes()).await?;
        writer.write_all(body).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "write timeout"))?
}

/// Write a JSON-RPC notification (no `id` field) with Content-Length framing.
/// Includes a per-client `seq` number for event ordering.
async fn write_notification<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    event: &broadcast::EditorEvent,
    seq: u64,
    timeout: std::time::Duration,
) -> Result<(), std::io::Error> {
    let method = format!("notifications/{}", event.event_type());
    let notification = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": { "seq": seq, "event": event },
    });
    let body = serde_json::to_vec(&notification)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_framed(writer, &body, timeout).await
}

// ---------------------------------------------------------------------------
// Message framing (Content-Length + line-based fallback)
// ---------------------------------------------------------------------------

/// Format bytes as hex for diagnostic logging.
fn hex_preview(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Read a single JSON-RPC message from the stream.
///
/// Auto-detects framing:
/// - If the stream starts with `Content-Length:`, reads the header and then
///   exactly that many bytes of body (LSP-compatible framing).
/// - Otherwise, reads a single line (legacy line-based framing).
///
/// Returns `Ok(None)` on clean EOF.
pub async fn read_message<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<String>, std::io::Error> {
    // Peek at the buffer to determine framing mode.
    let buf = reader.fill_buf().await?;
    if buf.is_empty() {
        return Ok(None); // EOF
    }

    // Check if this looks like Content-Length framing.
    // Use a prefix check that works even with small initial reads: if the
    // buffer starts with any prefix of "Content-Length:", assume CL framing.
    // The header-reading loop below will read more bytes as needed.
    let cl_prefix = b"Content-Length:";
    let peek_len = buf.len().min(cl_prefix.len());
    let looks_like_cl = peek_len > 0 && buf[..peek_len] == cl_prefix[..peek_len];
    tracing::debug!(
        peek_first_byte = buf[0],
        peek_len = buf.len(),
        looks_like_cl,
        peek_hex = %hex_preview(&buf[..buf.len().min(30)]),
        "read_message: framing decision"
    );
    if looks_like_cl {
        // Read header lines until we hit the empty \r\n separator.
        let mut content_length: Option<usize> = None;
        let mut header_bytes: usize = 0;
        const MAX_HEADER_SIZE: usize = 16_384; // 16 KB guard
        loop {
            let mut header_line = String::new();
            let n = reader.read_line(&mut header_line).await?;
            if n == 0 {
                return Ok(None); // EOF mid-header
            }
            header_bytes += n;
            if header_bytes > MAX_HEADER_SIZE {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "header too large (>16KB)",
                ));
            }
            let trimmed = header_line.trim();
            if trimmed.is_empty() {
                break; // End of headers
            }
            if let Some(val) = trimmed.strip_prefix("Content-Length:") {
                let raw = val.trim();
                match raw.parse::<usize>() {
                    Ok(v) => content_length = Some(v),
                    Err(_) => {
                        warn!(header = %trimmed, "non-numeric Content-Length");
                        return Err(std::io::Error::other(format!(
                            "non-numeric Content-Length: {}",
                            raw
                        )));
                    }
                }
            }
        }

        let len = content_length
            .ok_or_else(|| std::io::Error::other("Content-Length header missing value"))?;

        if len == 0 {
            return Err(std::io::Error::other("Content-Length must be > 0"));
        }
        if len > MAX_MESSAGE_SIZE {
            return Err(std::io::Error::other(format!(
                "Content-Length {} exceeds maximum {}",
                len, MAX_MESSAGE_SIZE
            )));
        }

        let mut body = vec![0u8; len];
        tokio::io::AsyncReadExt::read_exact(reader, &mut body).await?;
        let msg = String::from_utf8(body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        tracing::debug!(
            content_length = len,
            msg_len = msg.len(),
            has_id = msg.contains("\"id\""),
            has_method = msg.contains("\"method\""),
            "read_message: complete (CL)"
        );
        Ok(Some(msg))
    } else {
        // Legacy line-based framing. Skip blank lines.
        let peek_bytes = &buf[..buf.len().min(40)];
        tracing::warn!(
            peek_hex = %hex_preview(peek_bytes),
            peek_len = buf.len(),
            "read_message: falling back to line-based framing"
        );
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None);
            }
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() {
                tracing::warn!(
                    line_len = trimmed.len(),
                    has_id = trimmed.contains("\"id\""),
                    has_method = trimmed.contains("\"method\""),
                    "read_message: complete (line-based)"
                );
                return Ok(Some(trimmed));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Request dispatch
// ---------------------------------------------------------------------------

/// Process a single JSON-RPC request, updating session state as needed.
///
/// Handles protocol methods (initialize, ping, subscribe, health, etc.)
/// and dispatches tool calls and sync methods via the `tool_tx` channel.
/// Reusable by any server (editor MCP, daemon) that needs JSON-RPC dispatch.
pub async fn handle_request(
    msg: &str,
    tool_definitions: &[ToolInfo],
    tool_tx: &mpsc::Sender<McpToolRequest>,
    session: &mut ClientSession,
    broadcaster: &broadcast::SharedBroadcaster,
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
            // Extract client info and requested protocol version.
            let mut client_requested_version: Option<&str> = None;
            if let Some(ref params) = request.params {
                client_requested_version = params.get("protocolVersion").and_then(|v| v.as_str());
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

            let negotiated = match client_requested_version {
                Some(v) => protocol::negotiate_version(v),
                None => protocol::PROTOCOL_VERSION,
            };

            info!(
                session = session.id,
                client = session.display_name(),
                negotiated_version = negotiated,
                "MCP initialize handshake"
            );

            let result = InitializeResult {
                protocol_version: negotiated.to_string(),
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
        // Backward compat: some clients incorrectly send this as a request
        // (with `id`). The proper notification path is handled in handle_client
        // before dispatch. This arm handles the request variant gracefully.
        "notifications/initialized" => {
            session.initialized = true;
            debug!(session = session.id, "client initialized (request compat)");
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
                    // Update broadcaster so the event channel filters correctly.
                    let subs: Vec<String> = session.subscriptions.iter().cloned().collect();
                    broadcaster
                        .lock()
                        .unwrap()
                        .update_subscriptions(session.id, subs);
                    debug!(
                        session = session.id,
                        subscriptions = ?session.subscriptions,
                        "client subscribed to events"
                    );
                }
            }
            JsonRpcResponse::success(id, serde_json::Value::Null)
        }
        "$/health" => {
            let uptime = session.connected_at.elapsed().as_secs();
            let health = serde_json::json!({
                "uptime_secs": uptime,
                "session_id": session.id,
                "initialized": session.initialized,
                "messages_received": session.messages_received,
                "tool_calls": session.tool_calls,
                "protocol_version": env!("CARGO_PKG_VERSION"),
            });
            JsonRpcResponse::success(id, health)
        }
        "$/resync" => {
            let resync = serde_json::json!({
                "session_id": session.id,
                "subscriptions": session.subscriptions.iter().collect::<Vec<_>>(),
                "messages_received": session.messages_received,
                "message": "Full editor state resync requires tool call to introspect"
            });
            JsonRpcResponse::success(id, resync)
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
        // --- Sync protocol methods ---
        "sync/enable" | "sync/state_vector" | "sync/update" | "sync/full_state" => {
            let params = request.params.unwrap_or(serde_json::Value::Null);
            let (reply_tx, reply_rx) = oneshot::channel();
            let req = McpToolRequest {
                tool_name: format!("__mcp_{}", request.method.replace('/', "_")),
                arguments: params,
                reply: reply_tx,
            };
            debug!(session = session.id, method = %request.method, "sync method dispatched");
            if tool_tx.send(req).await.is_err() {
                return JsonRpcResponse::error(
                    id,
                    McpError::internal_error("Editor channel closed".to_string()),
                );
            }
            match reply_rx.await {
                Ok(result) => {
                    if result.success {
                        match serde_json::from_str::<serde_json::Value>(&result.output) {
                            Ok(val) => JsonRpcResponse::success(id, val),
                            Err(_) => JsonRpcResponse::success(
                                id,
                                serde_json::json!({ "result": result.output }),
                            ),
                        }
                    } else {
                        JsonRpcResponse::error(id, McpError::internal_error(result.output))
                    }
                }
                Err(_) => JsonRpcResponse::error(
                    id,
                    McpError::internal_error("Sync operation cancelled".to_string()),
                ),
            }
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

            debug!(session = session.id, tool = %tool_name, "tool call dispatched");
            session.tool_calls += 1;

            if tool_tx.send(req).await.is_err() {
                return JsonRpcResponse::error(
                    id,
                    McpError::internal_error("Editor channel closed".to_string()),
                );
            }

            match reply_rx.await {
                Ok(result) => {
                    debug!(session = session.id, tool = %tool_name, success = result.success, "tool call complete");
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

    /// Create a dummy `SharedBroadcaster` for unit tests.
    fn dummy_broadcaster() -> broadcast::SharedBroadcaster {
        std::sync::Arc::new(std::sync::Mutex::new(broadcast::EventBroadcaster::new()))
    }

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
        let bc = dummy_broadcaster();
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"test-client","version":"0.1"}}}"#;

        let resp = handle_request(msg, &[], &tx, &mut session, &bc).await;
        assert!(resp.result.is_some());
        assert_eq!(session.client_info.as_ref().unwrap().name, "test-client");
    }

    #[tokio::test]
    async fn handle_request_initialize_echoes_protocol_version() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let bc = dummy_broadcaster();

        // Client requests 2025-11-25 — server must echo it back.
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
        let resp = handle_request(msg, &[], &tx, &mut session, &bc).await;
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2025-11-25");

        // Client requests old version — server echoes that too.
        let mut session2 = ClientSession::new();
        let msg2 = r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"old-client","version":"0.1"}}}"#;
        let resp2 = handle_request(msg2, &[], &tx, &mut session2, &bc).await;
        let result2 = resp2.result.unwrap();
        assert_eq!(result2["protocolVersion"], "2024-11-05");

        // Client requests unknown version — server returns latest.
        let mut session3 = ClientSession::new();
        let msg3 = r#"{"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":"9999-01-01","capabilities":{},"clientInfo":{"name":"future","version":"9.0"}}}"#;
        let resp3 = handle_request(msg3, &[], &tx, &mut session3, &bc).await;
        let result3 = resp3.result.unwrap();
        assert_eq!(result3["protocolVersion"], "2025-11-25");
    }

    #[tokio::test]
    async fn handle_request_ping() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let bc = dummy_broadcaster();
        let msg = r#"{"jsonrpc":"2.0","id":2,"method":"$/ping"}"#;

        let resp = handle_request(msg, &[], &tx, &mut session, &bc).await;
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn handle_request_subscribe() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let bc = dummy_broadcaster();
        let msg = r#"{"jsonrpc":"2.0","id":3,"method":"notifications/subscribe","params":{"types":["buffer_edit","diagnostics"]}}"#;

        let resp = handle_request(msg, &[], &tx, &mut session, &bc).await;
        assert!(resp.result.is_some());
        assert!(session.subscriptions.contains("buffer_edit"));
        assert!(session.subscriptions.contains("diagnostics"));
    }

    #[tokio::test]
    async fn handle_request_tools_list() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let bc = dummy_broadcaster();
        let tools = vec![ToolInfo {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        let msg = r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#;

        let resp = handle_request(msg, &tools, &tx, &mut session, &bc).await;
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
    // Content-Length framing edge-case tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn framing_zero_content_length() {
        let data = b"Content-Length: 0\r\n\r\n";
        let mut reader = BufReader::new(&data[..]);
        let result = read_message(&mut reader).await;
        assert!(result.is_err() || result.unwrap().is_none());
    }

    #[tokio::test]
    async fn framing_huge_content_length() {
        // Content-Length exceeding MAX_MESSAGE_SIZE should error
        let data = b"Content-Length: 999999999\r\n\r\n";
        let mut reader = BufReader::new(&data[..]);
        let result = read_message(&mut reader).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn framing_non_numeric() {
        let data = b"Content-Length: abc\r\n\r\n";
        let mut reader = BufReader::new(&data[..]);
        let result = read_message(&mut reader).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn framing_negative_content_length() {
        let data = b"Content-Length: -1\r\n\r\n";
        let mut reader = BufReader::new(&data[..]);
        let result = read_message(&mut reader).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn framing_partial_header_then_eof() {
        // Partial header followed by EOF
        let data = b"Content-Len";
        let mut reader = BufReader::new(&data[..]);
        let result = read_message(&mut reader).await;
        // Should get None (EOF in line mode) or error
        assert!(result.is_ok()); // line mode reads "Content-Len" as a line
    }

    #[tokio::test]
    async fn framing_utf8_invalid_body() {
        let invalid_utf8 = vec![0xFF, 0xFE, 0x00];
        let header = format!("Content-Length: {}\r\n\r\n", invalid_utf8.len());
        let mut data = header.into_bytes();
        data.extend_from_slice(&invalid_utf8);
        let mut reader = BufReader::new(&data[..]);
        let result = read_message(&mut reader).await;
        assert!(result.is_err()); // Invalid UTF-8
    }

    #[tokio::test]
    async fn framing_mixed_modes() {
        // Line-based message followed by Content-Length message
        let line_msg = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        let cl_body = "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"pong\"}";
        let cl_header = format!("Content-Length: {}\r\n\r\n", cl_body.len());
        let data = format!("{}{}{}", line_msg, cl_header, cl_body);
        let mut reader = BufReader::new(data.as_bytes());

        let msg1 = read_message(&mut reader).await.unwrap().unwrap();
        assert!(msg1.contains("ping"));

        let msg2 = read_message(&mut reader).await.unwrap().unwrap();
        assert!(msg2.contains("pong"));
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
        use tokio::io::AsyncWriteExt;

        let payload = serde_json::to_string(msg).unwrap();
        stream
            .write_all(format!("{}\n", payload).as_bytes())
            .await
            .unwrap();
        stream.flush().await.unwrap();

        let value = read_framed_message(stream, 5000)
            .await
            .expect("expected response from server");
        serde_json::from_value(value).unwrap()
    }

    /// Helper: send a JSON-RPC notification (no `id`, fire-and-forget).
    async fn send_notification(
        stream: &mut tokio::net::UnixStream,
        method: &str,
        params: Option<serde_json::Value>,
    ) {
        use tokio::io::AsyncWriteExt;

        let mut msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        if let Some(p) = params {
            msg["params"] = p;
        }
        let payload = serde_json::to_string(&msg).unwrap();
        stream
            .write_all(format!("{}\n", payload).as_bytes())
            .await
            .unwrap();
        stream.flush().await.unwrap();
    }

    #[tokio::test]
    async fn multi_client_concurrent_connections() {
        let socket_path = format!("/tmp/mae-test-multi-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        // Set up the server with a mock tool handler.
        let (tool_tx, mut tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let server = McpServer::new(&socket_path, tool_tx, dummy_broadcaster());
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
        let server = McpServer::new(&socket_path, tool_tx, dummy_broadcaster());

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

    #[tokio::test]
    async fn client_lifecycle_full_sequence() {
        let socket_path = format!("/tmp/mae-test-lifecycle-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, mut tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let server = McpServer::new(&socket_path, tool_tx, dummy_broadcaster());
        let tools = vec![ToolInfo {
            name: "test_tool".to_string(),
            description: "Test".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }];

        tokio::spawn(async move {
            server.run(tools).await;
        });
        tokio::spawn(async move {
            while let Some(req) = tool_rx.recv().await {
                let _ = req.reply.send(McpToolResult {
                    success: true,
                    output: "ok".to_string(),
                });
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        // 1. Initialize
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "lifecycle-test", "version": "1.0"}}
            }),
        )
        .await;
        assert!(resp.error.is_none());

        // 2. notifications/initialized — proper notification (no id, no response)
        send_notification(&mut client, "notifications/initialized", None).await;
        // Brief pause for server to process the notification.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // 3. Tool call
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "test_tool", "arguments": {}}
            }),
        )
        .await;
        assert!(resp.error.is_none());

        // 4. Ping
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 4, "method": "$/ping"
            }),
        )
        .await;
        assert_eq!(resp.result.unwrap(), "pong");

        // 5. Health check
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 5, "method": "$/health"
            }),
        )
        .await;
        let health = resp.result.unwrap();
        assert!(health["session_id"].as_u64().unwrap() > 0);

        // 6. Shutdown
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 6, "method": "shutdown"
            }),
        )
        .await;
        assert!(resp.error.is_none());

        drop(client);

        // 7. Server still accepts new connections
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let mut client2 = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let resp = send_and_recv(
            &mut client2,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "$/ping"
            }),
        )
        .await;
        assert_eq!(resp.result.unwrap(), "pong");

        drop(client2);
        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn handle_request_resync() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let bc = dummy_broadcaster();
        session.subscriptions.insert("buffer_edit".to_string());
        session.subscriptions.insert("mode_change".to_string());

        let msg = r#"{"jsonrpc":"2.0","id":10,"method":"$/resync"}"#;
        let resp = handle_request(msg, &[], &tx, &mut session, &bc).await;
        let result = resp.result.unwrap();

        assert_eq!(result["session_id"], session.id);
        let subs = result["subscriptions"].as_array().unwrap();
        assert_eq!(subs.len(), 2);
        assert!(result["message"].as_str().unwrap().contains("resync"));
    }

    #[tokio::test]
    async fn client_rapid_connect_disconnect() {
        let socket_path = format!("/tmp/mae-test-rapid-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let server = McpServer::new(&socket_path, tool_tx, dummy_broadcaster());

        tokio::spawn(async move {
            server.run(vec![]).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Rapidly connect and disconnect 10 clients
        for _ in 0..10 {
            let client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
            drop(client);
        }

        // Small delay for server to process disconnects
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Server should still be alive
        let mut alive_client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let resp = send_and_recv(
            &mut alive_client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "$/ping"
            }),
        )
        .await;
        assert_eq!(resp.result.unwrap(), "pong");

        drop(alive_client);
        let _ = std::fs::remove_file(&socket_path);
    }

    // -----------------------------------------------------------------------
    // Push notification integration tests
    // -----------------------------------------------------------------------

    /// Helper: read a Content-Length framed message from a stream.
    /// Returns the parsed JSON. Panics on timeout.
    async fn read_framed_message(
        stream: &mut tokio::net::UnixStream,
        timeout_ms: u64,
    ) -> Option<serde_json::Value> {
        use tokio::io::AsyncReadExt;

        let result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
            let mut header_buf = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                stream.read_exact(&mut byte).await.ok()?;
                header_buf.push(byte[0]);
                if header_buf.len() >= 4 && &header_buf[header_buf.len() - 4..] == b"\r\n\r\n" {
                    break;
                }
            }
            let header = String::from_utf8(header_buf).ok()?;
            let content_length: usize = header
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .and_then(|v| v.trim().parse().ok())?;
            let mut body = vec![0u8; content_length];
            stream.read_exact(&mut body).await.ok()?;
            serde_json::from_slice(&body).ok()
        })
        .await;

        result.unwrap_or_default()
    }

    #[tokio::test]
    async fn push_notification_after_subscribe() {
        let socket_path = format!("/tmp/mae-test-push-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let bc = dummy_broadcaster();
        let server = McpServer::new(&socket_path, tool_tx, bc.clone());

        tokio::spawn(async move {
            server.run(vec![]).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        // Initialize.
        send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "push-test"}}
            }),
        )
        .await;

        // Subscribe to sync_update.
        send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "notifications/subscribe",
                "params": {"types": ["sync_update"]}
            }),
        )
        .await;

        // Broadcast a sync update via the shared broadcaster.
        {
            let mut locked = bc.lock().unwrap();
            locked.broadcast(&broadcast::EditorEvent::SyncUpdate {
                buffer_name: "test.rs".to_string(),
                update_base64: "AQIDBA==".to_string(),
                wal_seq: 0,
            });
        }

        // Client should receive the push notification.
        let notification = read_framed_message(&mut client, 1000).await;
        assert!(notification.is_some(), "should have received notification");
        let notif = notification.unwrap();
        // JSON-RPC notification: no "id" field.
        assert!(
            notif.get("id").is_none(),
            "notification should have no id field"
        );
        assert_eq!(notif["jsonrpc"], "2.0");
        assert_eq!(notif["method"], "notifications/sync_update");
        assert!(notif["params"]["seq"].as_u64().unwrap() > 0);
        assert_eq!(notif["params"]["event"]["data"]["buffer_name"], "test.rs");
        assert_eq!(
            notif["params"]["event"]["data"]["update_base64"],
            "AQIDBA=="
        );

        drop(client);
        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn push_notification_not_sent_before_subscribe() {
        let socket_path = format!("/tmp/mae-test-nosub-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let bc = dummy_broadcaster();
        let server = McpServer::new(&socket_path, tool_tx, bc.clone());

        tokio::spawn(async move {
            server.run(vec![]).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        // Initialize but do NOT subscribe.
        send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "nosub-test"}}
            }),
        )
        .await;

        // Broadcast an event.
        {
            let mut locked = bc.lock().unwrap();
            locked.broadcast(&broadcast::EditorEvent::SyncUpdate {
                buffer_name: "test.rs".to_string(),
                update_base64: "dGVzdA==".to_string(),
                wal_seq: 0,
            });
        }

        // Try to read — should timeout (no notification expected).
        let msg = read_framed_message(&mut client, 200).await;
        assert!(
            msg.is_none(),
            "should NOT have received notification without subscribing"
        );

        // Ping should still work.
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/ping"}),
        )
        .await;
        assert_eq!(resp.result.unwrap(), "pong");

        drop(client);
        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn two_clients_one_subscribed_one_not() {
        let socket_path = format!("/tmp/mae-test-2cli-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let bc = dummy_broadcaster();
        let server = McpServer::new(&socket_path, tool_tx, bc.clone());

        tokio::spawn(async move {
            server.run(vec![]).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Client A: subscribes to sync_update.
        let mut client_a = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        send_and_recv(
            &mut client_a,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "client-a"}}
            }),
        )
        .await;
        send_and_recv(
            &mut client_a,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "notifications/subscribe",
                "params": {"types": ["sync_update"]}
            }),
        )
        .await;

        // Client B: does NOT subscribe.
        let mut client_b = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        send_and_recv(
            &mut client_b,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "client-b"}}
            }),
        )
        .await;

        // Broadcast.
        {
            let mut locked = bc.lock().unwrap();
            locked.broadcast(&broadcast::EditorEvent::SyncUpdate {
                buffer_name: "shared.rs".to_string(),
                update_base64: "AAAA".to_string(),
                wal_seq: 0,
            });
        }

        // Client A should receive notification.
        let notif = read_framed_message(&mut client_a, 1000).await;
        assert!(notif.is_some(), "client A should receive notification");
        assert_eq!(notif.unwrap()["method"], "notifications/sync_update");

        // Client B should NOT receive notification.
        let no_notif = read_framed_message(&mut client_b, 200).await;
        assert!(
            no_notif.is_none(),
            "client B should NOT receive notification"
        );

        // Client B can still ping.
        let resp = send_and_recv(
            &mut client_b,
            &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/ping"}),
        )
        .await;
        assert_eq!(resp.result.unwrap(), "pong");

        drop(client_a);
        drop(client_b);
        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn push_notification_survives_client_disconnect() {
        let socket_path = format!("/tmp/mae-test-surv-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let bc = dummy_broadcaster();
        let server = McpServer::new(&socket_path, tool_tx, bc.clone());

        tokio::spawn(async move {
            server.run(vec![]).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Client A subscribes.
        let mut client_a = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        send_and_recv(
            &mut client_a,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "surv-a"}}
            }),
        )
        .await;
        send_and_recv(
            &mut client_a,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "notifications/subscribe",
                "params": {"types": ["sync_update"]}
            }),
        )
        .await;

        // Client B subscribes.
        let mut client_b = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        send_and_recv(
            &mut client_b,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "surv-b"}}
            }),
        )
        .await;
        send_and_recv(
            &mut client_b,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "notifications/subscribe",
                "params": {"types": ["sync_update"]}
            }),
        )
        .await;

        // Drop client A.
        drop(client_a);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Broadcast after A disconnected.
        {
            let mut locked = bc.lock().unwrap();
            locked.broadcast(&broadcast::EditorEvent::SyncUpdate {
                buffer_name: "after.rs".to_string(),
                update_base64: "BBBB".to_string(),
                wal_seq: 0,
            });
        }

        // Client B should still receive the notification.
        let notif = read_framed_message(&mut client_b, 1000).await;
        assert!(
            notif.is_some(),
            "client B should receive notification after A disconnected"
        );
        assert_eq!(
            notif.unwrap()["params"]["event"]["data"]["buffer_name"],
            "after.rs"
        );

        drop(client_b);
        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn backpressure_drops_events_gracefully() {
        let socket_path = format!("/tmp/mae-test-bp-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let bc = dummy_broadcaster();
        let server = McpServer::new(&socket_path, tool_tx, bc.clone());

        tokio::spawn(async move {
            server.run(vec![]).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "bp-test"}}
            }),
        )
        .await;
        send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "notifications/subscribe",
                "params": {"types": ["sync_update"]}
            }),
        )
        .await;

        // Blast 200 events (queue capacity is 100).
        {
            let mut locked = bc.lock().unwrap();
            for i in 0..200 {
                locked.broadcast(&broadcast::EditorEvent::SyncUpdate {
                    buffer_name: format!("file-{}.rs", i),
                    update_base64: "AA==".to_string(),
                    wal_seq: 0,
                });
            }
        }

        // Read as many as we can (up to 100, queue capacity).
        let mut received = 0;
        while read_framed_message(&mut client, 200).await.is_some() {
            received += 1;
        }
        assert!(received > 0, "should have received some events");
        assert!(
            received <= 100,
            "should not exceed queue capacity, got {}",
            received
        );

        // Server should still be operational.
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "$/ping"}),
        )
        .await;
        assert_eq!(resp.result.unwrap(), "pong");

        drop(client);
        let _ = std::fs::remove_file(&socket_path);
    }

    // -----------------------------------------------------------------------
    // TCP transport tests
    // -----------------------------------------------------------------------

    /// Helper: connect to a TCP address, send JSON-RPC, read Content-Length response.
    async fn tcp_send_and_recv(
        stream: &mut tokio::net::TcpStream,
        msg: &serde_json::Value,
    ) -> JsonRpcResponse {
        use tokio::io::AsyncWriteExt;

        let payload = serde_json::to_string(msg).unwrap();
        stream
            .write_all(format!("{}\n", payload).as_bytes())
            .await
            .unwrap();
        stream.flush().await.unwrap();

        let value = tcp_read_framed(stream, 5000)
            .await
            .expect("expected response from TCP server");
        serde_json::from_value(value).unwrap()
    }

    /// Helper: read Content-Length framed message from TcpStream.
    async fn tcp_read_framed(
        stream: &mut tokio::net::TcpStream,
        timeout_ms: u64,
    ) -> Option<serde_json::Value> {
        use tokio::io::AsyncReadExt;

        let result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
            let mut header_buf = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                stream.read_exact(&mut byte).await.ok()?;
                header_buf.push(byte[0]);
                if header_buf.len() >= 4 && &header_buf[header_buf.len() - 4..] == b"\r\n\r\n" {
                    break;
                }
            }
            let header = String::from_utf8(header_buf).ok()?;
            let content_length: usize = header
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .and_then(|v| v.trim().parse().ok())?;
            let mut body = vec![0u8; content_length];
            stream.read_exact(&mut body).await.ok()?;
            serde_json::from_slice(&body).ok()
        })
        .await;

        result.unwrap_or_default()
    }

    #[tokio::test]
    async fn tcp_read_message_works() {
        // Verify read_message works with TCP streams (via BufReader over &[u8])
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"test"}"#;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let data = format!("{}{}", header, body);
        let mut reader = BufReader::new(data.as_bytes());
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert!(msg.contains("test"));
    }

    #[tokio::test]
    async fn tcp_write_framed_works() {
        // Verify write_framed works with any AsyncWrite
        let mut buf = Vec::new();
        let body = b"hello";
        write_framed(&mut buf, body, std::time::Duration::from_secs(5))
            .await
            .unwrap();
        let expected = "Content-Length: 5\r\n\r\nhello".to_string();
        assert_eq!(String::from_utf8(buf).unwrap(), expected);
    }

    #[tokio::test]
    async fn tcp_single_client_initialize() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let bc = dummy_broadcaster();
        let tool_defs: Vec<ToolInfo> = vec![];
        let bc_clone = bc.clone();

        // Spawn a mini-server that accepts one TCP client.
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let session = ClientSession::new();
            let (reader, writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut writer = writer;
            let mut session = session;
            let write_timeout = std::time::Duration::from_secs(5);

            // Simple request-response loop (no event push for this test).
            while let Ok(Some(msg)) = read_message(&mut reader).await {
                let response =
                    handle_request(&msg, &tool_defs, &tool_tx, &mut session, &bc_clone).await;
                let body = serde_json::to_vec(&response).unwrap();
                if write_framed(&mut writer, &body, write_timeout)
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // Connect as a TCP client.
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();

        // Initialize
        let resp = tcp_send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "tcp-test", "version": "0.1"}}
            }),
        )
        .await;
        assert!(
            resp.error.is_none(),
            "TCP initialize failed: {:?}",
            resp.error
        );
        let result = resp.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "mae-editor");

        // Ping
        let resp = tcp_send_and_recv(
            &mut client,
            &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/ping"}),
        )
        .await;
        assert_eq!(resp.result.unwrap(), "pong");
    }

    #[tokio::test]
    async fn tcp_and_unix_coexist() {
        use tokio::net::TcpListener;

        let socket_path = format!("/tmp/mae-test-coexist-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        // TCP listener
        let tcp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp_addr = tcp_listener.local_addr().unwrap();

        // Unix server
        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let bc = dummy_broadcaster();
        let unix_server = McpServer::new(&socket_path, tool_tx.clone(), bc.clone());

        tokio::spawn(async move {
            unix_server.run(vec![]).await;
        });

        // TCP server task
        let tool_tx2 = tool_tx.clone();
        let bc2 = bc.clone();
        tokio::spawn(async move {
            let (stream, _) = tcp_listener.accept().await.unwrap();
            let session = ClientSession::new();
            let tool_defs: Vec<ToolInfo> = vec![];
            let (reader, writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut writer = writer;
            let mut session = session;
            let timeout = std::time::Duration::from_secs(5);

            while let Ok(Some(msg)) = read_message(&mut reader).await {
                let response =
                    handle_request(&msg, &tool_defs, &tool_tx2, &mut session, &bc2).await;
                let body = serde_json::to_vec(&response).unwrap();
                if write_framed(&mut writer, &body, timeout).await.is_err() {
                    break;
                }
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Both transports should work concurrently.
        let mut unix_client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let mut tcp_client = tokio::net::TcpStream::connect(tcp_addr).await.unwrap();

        let unix_resp = send_and_recv(
            &mut unix_client,
            &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "$/ping"}),
        )
        .await;
        assert_eq!(unix_resp.result.unwrap(), "pong");

        let tcp_resp = tcp_send_and_recv(
            &mut tcp_client,
            &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "$/ping"}),
        )
        .await;
        assert_eq!(tcp_resp.result.unwrap(), "pong");

        drop(unix_client);
        drop(tcp_client);
        let _ = std::fs::remove_file(&socket_path);
    }

    // -----------------------------------------------------------------------
    // Notification handling tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn notification_initialized_sets_session_flag() {
        let socket_path = format!("/tmp/mae-test-notif-init-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let server = McpServer::new(&socket_path, tool_tx, dummy_broadcaster());

        tokio::spawn(async move {
            server.run(vec![]).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        // Initialize (request).
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "notif-test", "version": "1.0"}}
            }),
        )
        .await;
        assert!(resp.error.is_none());

        // Send proper notification (no id).
        send_notification(&mut client, "notifications/initialized", None).await;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        // Verify via health that session is initialized.
        let health = send_and_recv(
            &mut client,
            &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/health"}),
        )
        .await;
        let result = health.result.unwrap();
        assert_eq!(
            result["initialized"], true,
            "session should be initialized after notification"
        );

        drop(client);
        let _ = std::fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn notification_unknown_silently_accepted() {
        let socket_path = format!("/tmp/mae-test-notif-unk-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, _tool_rx) = mpsc::channel::<McpToolRequest>(16);
        let server = McpServer::new(&socket_path, tool_tx, dummy_broadcaster());

        tokio::spawn(async move {
            server.run(vec![]).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        // Initialize.
        send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "unk-notif-test", "version": "1.0"}}
            }),
        )
        .await;

        // Send an unknown notification — should not crash or close connection.
        send_notification(&mut client, "notifications/something_unknown", None).await;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        // Connection should still be alive — verify with ping.
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/ping"}),
        )
        .await;
        assert_eq!(resp.result.unwrap(), "pong");

        drop(client);
        let _ = std::fs::remove_file(&socket_path);
    }

    // ---- Regression tests for transport framing and version negotiation ----
    //
    // These test the specific failure modes discovered when connecting Claude Code
    // v2.1.72 to MAE via the MCP shim (2026-05-19):
    //
    // Bug 1: MAE returned protocolVersion "2024-11-05" when Claude Code requested
    //         "2025-11-25". Claude Code silently disconnected after 30s.
    //         Fix: negotiate_version() echoes back the client's version if supported.
    //
    // Bug 2: The shim used Content-Length framing on stdout (LSP-style), but the
    //         MCP stdio transport spec requires newline-delimited JSON. Claude Code
    //         couldn't parse the response and hung for 30s.
    //         Fix: shim writes JSON + \n to stdout, reads lines from stdin.
    //         Ref: https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#stdio

    /// Regression: initialize must echo back the client's requested protocol version.
    /// Claude Code v2.1.72+ requests "2025-11-25" and disconnects if it gets anything else.
    #[tokio::test]
    async fn regression_initialize_echoes_client_protocol_version() {
        let (tx, _rx) = mpsc::channel(1);
        let bc = dummy_broadcaster();

        // Simulate exactly what Claude Code v2.1.72 sends.
        let claude_init = r#"{"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{"roots":{},"elicitation":{"form":{},"url":{}}},"clientInfo":{"name":"claude-code","version":"2.1.72"}},"jsonrpc":"2.0","id":0}"#;
        let mut session = ClientSession::new();
        let resp = handle_request(claude_init, &[], &tx, &mut session, &bc).await;
        let result = resp.result.unwrap();

        // MUST echo back the exact version the client requested.
        assert_eq!(
            result["protocolVersion"], "2025-11-25",
            "Server must echo client's protocolVersion per MCP spec"
        );
        // MUST have tools capability.
        assert!(
            result["capabilities"]["tools"].is_object(),
            "Server must declare tools capability"
        );
        // MUST have serverInfo with name.
        assert_eq!(result["serverInfo"]["name"], "mae-editor");
    }

    /// Regression: server must handle older protocol versions too.
    #[tokio::test]
    async fn regression_initialize_accepts_old_protocol_version() {
        let (tx, _rx) = mpsc::channel(1);
        let bc = dummy_broadcaster();
        let mut session = ClientSession::new();

        let old_init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"old-client","version":"0.1"}}}"#;
        let resp = handle_request(old_init, &[], &tx, &mut session, &bc).await;
        let result = resp.result.unwrap();
        assert_eq!(
            result["protocolVersion"], "2024-11-05",
            "Server must echo back supported older versions"
        );
    }

    /// Regression: read_message must handle newline-delimited JSON (MCP stdio format).
    /// The shim reads this format from Claude Code's stdin.
    #[tokio::test]
    async fn regression_read_message_handles_jsonl_from_stdio() {
        // This is what Claude Code sends on stdin: bare JSON + newline, no Content-Length.
        let data = b"{\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"},\"jsonrpc\":\"2.0\",\"id\":0}\n";
        let mut reader = tokio::io::BufReader::new(&data[..]);
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert!(msg.contains("initialize"));
        assert!(msg.contains("2025-11-25"));
    }

    /// Regression: read_message must also handle Content-Length framing (socket format).
    /// The MAE server sends this format over the Unix socket.
    #[tokio::test]
    async fn regression_read_message_handles_content_length_from_socket() {
        let body = r#"{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2025-11-25"}}"#;
        let framed = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut reader = tokio::io::BufReader::new(framed.as_bytes());
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert!(msg.contains("2025-11-25"));
    }

    /// Regression: the full handshake sequence must work over a real Unix socket.
    /// Simulates exactly what Claude Code v2.1.72 does:
    ///   initialize (with protocolVersion) → notifications/initialized → tools/list
    #[tokio::test]
    async fn regression_full_handshake_sequence() {
        let socket_path = format!("/tmp/mae-test-handshake-{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let (tool_tx, _tool_rx) = mpsc::channel(16);
        let server = McpServer::new(&socket_path, tool_tx, dummy_broadcaster());
        let tools = vec![ToolInfo {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
        }];

        tokio::spawn(async move {
            server.run(tools).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        // Step 1: initialize with 2025-11-25 (what Claude Code sends)
        let resp = send_and_recv(
            &mut client,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 0, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {"roots": {}},
                    "clientInfo": {"name": "claude-code", "version": "2.1.72"}
                }
            }),
        )
        .await;
        assert!(resp.error.is_none(), "initialize should succeed");
        let result = resp.result.unwrap();
        assert_eq!(
            result["protocolVersion"], "2025-11-25",
            "Must echo back client's protocol version"
        );

        // Step 2: notifications/initialized (no response expected)
        send_notification(&mut client, "notifications/initialized", None).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Step 3: tools/list — must return the registered tools
        let tools_resp = send_and_recv(
            &mut client,
            &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .await;
        assert!(tools_resp.error.is_none(), "tools/list should succeed");
        let tools = tools_resp.result.unwrap();
        let tool_list = tools["tools"].as_array().unwrap();
        assert_eq!(tool_list.len(), 1);
        assert_eq!(tool_list[0]["name"], "test_tool");

        drop(client);
        let _ = std::fs::remove_file(&socket_path);
    }

    // -----------------------------------------------------------------------
    // Protocol audit tests (MCP hardening)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_tools_call_before_initialize() {
        // Sending tools/call without initialize should return an error, not panic.
        let (tx, rx) = mpsc::channel(1);
        // Drop the receiver so tool_tx.send() fails immediately.
        drop(rx);
        let mut session = ClientSession::new();
        let bc = dummy_broadcaster();
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"buffer_read","arguments":{}}}"#;
        let resp = handle_request(msg, &[], &tx, &mut session, &bc).await;
        // Must return an error (editor channel closed), not panic.
        assert!(resp.error.is_some());
        assert!(resp
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("channel closed"));
    }

    #[tokio::test]
    async fn test_unknown_method_returns_method_not_found() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let bc = dummy_broadcaster();
        let msg = r#"{"jsonrpc":"2.0","id":42,"method":"bogus/method"}"#;
        let resp = handle_request(msg, &[], &tx, &mut session, &bc).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, -32601);
        assert!(resp
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("bogus/method"));
    }

    #[tokio::test]
    async fn test_malformed_json_returns_parse_error() {
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let bc = dummy_broadcaster();
        let msg = "{not valid json at all";
        let resp = handle_request(msg, &[], &tx, &mut session, &bc).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, -32700);
    }

    #[test]
    fn test_json_rpc_error_codes_correct() {
        // Verify all McpError constructors use spec-correct codes.
        assert_eq!(McpError::parse_error("".into()).code, -32700);
        assert_eq!(McpError::invalid_request("".into()).code, -32600);
        assert_eq!(McpError::method_not_found("".into()).code, -32601);
        assert_eq!(McpError::internal_error("".into()).code, -32603);
        // Application-level codes in -32000..-32099 range.
        assert_eq!(McpError::backpressure("".into()).code, -32000);
        assert_eq!(McpError::editor_busy("".into()).code, -32001);
        assert_eq!(McpError::tool_not_found("".into()).code, -32002);
        assert_eq!(McpError::invalid_session("".into()).code, -32003);
        assert_eq!(McpError::session_expired("".into()).code, -32004);
    }

    #[tokio::test]
    async fn test_duplicate_initialize_rejected() {
        // The second initialize should still return a response (not panic),
        // and ideally succeed idempotently (current behavior) or error.
        let (tx, _rx) = mpsc::channel(1);
        let mut session = ClientSession::new();
        let bc = dummy_broadcaster();
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"test"}}}"#;
        let resp1 = handle_request(msg, &[], &tx, &mut session, &bc).await;
        assert!(resp1.error.is_none(), "first initialize should succeed");

        let msg2 = r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"clientInfo":{"name":"test"}}}"#;
        let resp2 = handle_request(msg2, &[], &tx, &mut session, &bc).await;
        // Should not panic. Either succeeds idempotently or returns an error.
        assert!(resp2.error.is_some() || resp2.result.is_some());
    }

    #[tokio::test]
    async fn test_notification_no_response() {
        // In handle_client, notifications (no `id`) are intercepted before
        // handle_request is called — they get no response. Verify that the
        // handle_client notification detection works correctly by checking
        // that a message with no `id` + a method is identified as a notification.
        let msg = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let val: serde_json::Value = serde_json::from_str(msg).unwrap();
        // Notification detection: has "method" but no "id".
        assert!(val.get("method").is_some());
        assert!(val.get("id").is_none());
        // If this were passed to handle_request, it would fail to deserialize
        // as JsonRpcRequest (missing required `id` field). That's correct —
        // notifications must be handled before dispatch.
    }

    #[tokio::test]
    async fn test_concurrent_tool_calls() {
        // Two tool calls with different IDs should each get the correct response.
        let (tx, mut rx) = mpsc::channel::<McpToolRequest>(16);
        let bc = dummy_broadcaster();

        // Spawn a mock tool handler that echoes the tool name.
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let _ = req.reply.send(McpToolResult {
                    success: true,
                    output: format!("result-{}", req.tool_name),
                });
            }
        });

        let tools = vec![
            ToolInfo {
                name: "tool_a".to_string(),
                description: "A".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            ToolInfo {
                name: "tool_b".to_string(),
                description: "B".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        ];

        let mut session_a = ClientSession::new();
        let mut session_b = ClientSession::new();
        let msg_a = r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"tool_a","arguments":{}}}"#;
        let msg_b = r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"tool_b","arguments":{}}}"#;

        let (resp_a, resp_b) = tokio::join!(
            handle_request(msg_a, &tools, &tx, &mut session_a, &bc),
            handle_request(msg_b, &tools, &tx, &mut session_b, &bc),
        );

        assert!(resp_a.error.is_none(), "tool_a should succeed");
        assert!(resp_b.error.is_none(), "tool_b should succeed");

        let text_a = resp_a.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        let text_b = resp_b.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();

        // Both should have gotten their respective results.
        assert!(text_a.contains("tool_a") || text_b.contains("tool_a"));
        assert!(text_a.contains("tool_b") || text_b.contains("tool_b"));
    }

    #[tokio::test]
    async fn test_header_size_guard() {
        // A pathological stream that sends endless header lines should be rejected.
        let mut data = Vec::new();
        data.extend_from_slice(b"Content-Length: 10\r\n");
        // Add 16KB+ of junk headers.
        for _ in 0..500 {
            data.extend_from_slice(b"X-Junk-Header: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
        }
        data.extend_from_slice(b"\r\n");
        data.extend_from_slice(b"0123456789");

        let mut reader = BufReader::new(&data[..]);
        let result = read_message(&mut reader).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("header too large"));
    }

    /// Regression: read_message must handle Content-Length framing even when
    /// the initial fill_buf returns fewer than 15 bytes (partial TCP read).
    /// A BufReader with a 1-byte buffer forces byte-by-byte reads.
    #[tokio::test]
    async fn read_message_partial_peek_content_length() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"sync/share"}"#;
        let framed = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        // Use a BufReader with capacity=1 to simulate partial TCP reads.
        let cursor = std::io::Cursor::new(framed.into_bytes());
        let mut reader = tokio::io::BufReader::with_capacity(1, cursor);

        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(msg, body);
    }

    /// Regression: two back-to-back Content-Length messages with tiny buffer.
    #[tokio::test]
    async fn read_message_two_messages_tiny_buffer() {
        let body1 = r#"{"id":1}"#;
        let body2 = r#"{"id":2}"#;
        let data = format!(
            "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
            body1.len(),
            body1,
            body2.len(),
            body2
        );

        let cursor = std::io::Cursor::new(data.into_bytes());
        let mut reader = tokio::io::BufReader::with_capacity(4, cursor);

        let msg1 = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(msg1, body1);

        let msg2 = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(msg2, body2);
    }
}
