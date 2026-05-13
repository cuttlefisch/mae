//! MCP client — connects to external MCP servers via stdio transport.
//!
//! Each client manages a child process, performs the JSON-RPC initialize
//! handshake, discovers tools, and forwards tool calls.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info, warn};

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Configuration for an external MCP server.
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub enabled: bool,
    pub auto_start: bool,
}

/// Discovered tool from an external MCP server.
#[derive(Debug, Clone)]
pub struct ExternalToolInfo {
    pub server_name: String,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Connection state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientState {
    Disconnected,
    Connecting,
    Ready,
    Failed(String),
}

impl std::fmt::Display for ClientState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientState::Disconnected => write!(f, "disconnected"),
            ClientState::Connecting => write!(f, "connecting"),
            ClientState::Ready => write!(f, "ready"),
            ClientState::Failed(msg) => write!(f, "failed: {}", msg),
        }
    }
}

type PendingRequests = Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>;

/// An MCP client connected to a single external server process.
pub struct McpClient {
    config: McpServerConfig,
    child: Option<Child>,
    tools: Vec<ExternalToolInfo>,
    writer_tx: Option<mpsc::Sender<String>>,
    pending: PendingRequests,
    next_id: AtomicU64,
    state: ClientState,
    failure_count: u32,
}

impl McpClient {
    pub fn new(config: McpServerConfig) -> Self {
        McpClient {
            config,
            child: None,
            tools: Vec::new(),
            writer_tx: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            state: ClientState::Disconnected,
            failure_count: 0,
        }
    }

    pub fn state(&self) -> &ClientState {
        &self.state
    }

    pub fn tools(&self) -> &[ExternalToolInfo] {
        &self.tools
    }

    pub fn server_name(&self) -> &str {
        &self.config.name
    }

    /// Connect to the MCP server: spawn process, start reader/writer tasks.
    pub async fn connect(&mut self) -> Result<(), String> {
        self.state = ClientState::Connecting;

        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args);
        for (k, v) in &self.config.env {
            // Support env values that are commands to execute (like api_key_command)
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());

        let mut child = cmd.spawn().map_err(|e| {
            let msg = format!("Failed to spawn '{}': {}", self.config.command, e);
            self.state = ClientState::Failed(msg.clone());
            msg
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "No stdout from child".to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "No stdin for child".to_string())?;

        // Writer task: sends JSON-RPC requests to the child's stdin
        let (writer_tx, mut writer_rx) = mpsc::channel::<String>(32);
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(msg) = writer_rx.recv().await {
                if let Err(e) = stdin.write_all(msg.as_bytes()).await {
                    error!(error = %e, "MCP client write error");
                    break;
                }
                if let Err(e) = stdin.write_all(b"\n").await {
                    error!(error = %e, "MCP client write newline error");
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // Reader task: reads JSON-RPC responses from the child's stdout
        let pending = self.pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        debug!("MCP client: child stdout closed");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                            Ok(resp) => {
                                let id = resp
                                    .id
                                    .as_u64()
                                    .or_else(|| resp.id.as_i64().map(|v| v as u64));
                                if let Some(id) = id {
                                    let mut pending = pending.lock().await;
                                    if let Some(tx) = pending.remove(&id) {
                                        let _ = tx.send(resp);
                                    }
                                }
                            }
                            Err(e) => {
                                debug!(error = %e, line = %trimmed, "MCP client: non-JSON-RPC line");
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "MCP client read error");
                        break;
                    }
                }
            }
        });

        self.child = Some(child);
        self.writer_tx = Some(writer_tx);
        self.state = ClientState::Ready;
        self.failure_count = 0;

        Ok(())
    }

    /// Send a JSON-RPC request and await the response.
    async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, String> {
        let writer = self.writer_tx.as_ref().ok_or("Not connected")?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::Value::Number(id.into()),
            method: method.into(),
            params,
        };

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        let json = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        writer
            .send(json)
            .await
            .map_err(|e| format!("Write channel closed: {}", e))?;

        // 30s timeout
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err("Response channel dropped".into()),
            Err(_) => {
                // Clean up pending
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err("Request timed out (30s)".into())
            }
        }
    }

    /// Perform the MCP initialize handshake.
    pub async fn initialize(&mut self) -> Result<(), String> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "mae-editor",
                "version": env!("CARGO_PKG_VERSION"),
            }
        });

        let resp = self.send_request("initialize", Some(params)).await?;
        if let Some(err) = resp.error {
            return Err(format!("Initialize failed: {}", err.message));
        }

        // Send initialized notification
        let _ = self.send_request("notifications/initialized", None).await;

        info!(server = %self.config.name, "MCP client initialized");
        Ok(())
    }

    /// Discover tools from the server.
    pub async fn discover_tools(&mut self) -> Result<(), String> {
        let resp = self
            .send_request("tools/list", Some(serde_json::json!({})))
            .await?;
        if let Some(err) = resp.error {
            return Err(format!("tools/list failed: {}", err.message));
        }

        let result = resp.result.unwrap_or_default();
        let tools_arr = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        self.tools.clear();
        for tool_val in tools_arr {
            let name = tool_val
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let description = tool_val
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = tool_val
                .get("inputSchema")
                .cloned()
                .unwrap_or(serde_json::json!({"type": "object"}));

            if !name.is_empty() {
                self.tools.push(ExternalToolInfo {
                    server_name: self.config.name.clone(),
                    name,
                    description,
                    input_schema,
                });
            }
        }

        info!(
            server = %self.config.name,
            tool_count = self.tools.len(),
            "MCP client discovered tools"
        );
        Ok(())
    }

    /// Call a tool on the remote server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, String> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        let resp = self.send_request("tools/call", Some(params)).await?;
        if let Some(err) = resp.error {
            return Err(format!("Tool call failed: {}", err.message));
        }

        let result = resp.result.unwrap_or_default();
        // Extract text content from MCP tool result
        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
            let texts: Vec<&str> = content
                .iter()
                .filter_map(|c| c.get("text").and_then(|v| v.as_str()))
                .collect();
            Ok(texts.join("\n"))
        } else {
            Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
        }
    }

    /// Disconnect: kill child process.
    pub async fn disconnect(&mut self) {
        self.writer_tx = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
        }
        self.tools.clear();
        self.state = ClientState::Disconnected;
    }

    /// Full connection sequence: connect, initialize, discover tools.
    pub async fn start(&mut self) -> Result<(), String> {
        self.connect().await?;
        if let Err(e) = self.initialize().await {
            self.disconnect().await;
            self.state = ClientState::Failed(e.clone());
            self.failure_count += 1;
            return Err(e);
        }
        if let Err(e) = self.discover_tools().await {
            warn!(server = %self.config.name, error = %e, "Tool discovery failed (server may not support tools/list)");
            // Don't disconnect — server is alive, just has no tools
        }
        Ok(())
    }

    /// Reconnect with exponential backoff.
    pub async fn reconnect(&mut self) {
        self.disconnect().await;
        let delay = std::cmp::min(1u64 << self.failure_count, 30);
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        if let Err(e) = self.start().await {
            warn!(
                server = %self.config.name,
                failure_count = self.failure_count,
                delay_secs = delay,
                error = %e,
                "MCP reconnect failed"
            );
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Best-effort kill — can't do async in Drop
        if let Some(ref mut child) = self.child {
            let _ = child.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let config = McpServerConfig {
            name: "test".into(),
            command: "echo".into(),
            args: vec!["hello".into()],
            env: HashMap::new(),
            enabled: true,
            auto_start: true,
        };
        assert_eq!(config.name, "test");
        assert!(config.enabled);
    }

    #[test]
    fn client_initial_state() {
        let config = McpServerConfig {
            name: "test".into(),
            command: "echo".into(),
            args: vec![],
            env: HashMap::new(),
            enabled: true,
            auto_start: true,
        };
        let client = McpClient::new(config);
        assert_eq!(*client.state(), ClientState::Disconnected);
        assert!(client.tools().is_empty());
    }

    #[test]
    fn client_state_display() {
        assert_eq!(format!("{}", ClientState::Disconnected), "disconnected");
        assert_eq!(format!("{}", ClientState::Connecting), "connecting");
        assert_eq!(format!("{}", ClientState::Ready), "ready");
        assert_eq!(
            format!("{}", ClientState::Failed("oops".into())),
            "failed: oops"
        );
    }

    #[test]
    fn tool_name_prefixing() {
        let tool = ExternalToolInfo {
            server_name: "filesystem".into(),
            name: "read_file".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({}),
        };
        let prefixed = format!("mcp_{}_{}", tool.server_name, tool.name);
        assert_eq!(prefixed, "mcp_filesystem_read_file");
    }

    #[test]
    fn parse_prefixed_tool_name() {
        let full = "mcp_github_create_issue";
        let rest = full.strip_prefix("mcp_").unwrap();
        let (server, tool) = rest.split_once('_').unwrap();
        assert_eq!(server, "github");
        assert_eq!(tool, "create_issue");
    }
}
