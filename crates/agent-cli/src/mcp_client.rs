//! Thin MCP client dialing MAE's Unix socket directly (ADR-046) — no
//! `mae-mcp-shim` subprocess hop. Reuses `mae_mcp::{read_message, write_framed}`
//! (the same Content-Length framing the shim itself uses) and, when a PSK file
//! is discovered alongside the socket, performs the ADR-048 handshake so this
//! harness can declare its AI provider and be trusted for `LocalModelsOnly` KBs.
//!
//! Sequential by design: one request in flight at a time, matching the agent
//! loop's own strictly-sequential tool-calling shape (`agent_loop.rs`) — no
//! pending-reply map or background reader task needed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use mae_mcp::auth::{AuthProvider, PskAuth};
use mae_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, ToolInfo};
use tokio::io::BufReader;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of a `tools/call` request.
#[derive(Debug, Clone)]
pub struct ToolCallOutcome {
    pub success: bool,
    pub text: String,
}

pub struct McpClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u64,
}

/// Abstraction over "call a tool by name, get an outcome back" — lets
/// `agent_loop.rs`'s turn loop be unit-tested against a stub instead of a real
/// socket connection.
#[async_trait::async_trait]
pub trait ToolExecutor: Send {
    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallOutcome>;
}

#[async_trait::async_trait]
impl ToolExecutor for McpClient {
    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallOutcome> {
        McpClient::call_tool(self, name, arguments).await
    }
}

/// Discover the newest live `/tmp/mae-{pid}[-agent].sock` and, if present, its
/// paired `/tmp/mae-{pid}.psk`. Mirrors `mae-mcp-shim`'s own `discover_socket`
/// PID-liveness scan. Prefers the PSK-required agent socket (ADR-048) when one
/// exists, so this harness gets `LocalModelsOnly` KB access for free; falls
/// back to the plain socket (no PSK) if no agent socket is running.
pub fn discover_connection() -> Option<(PathBuf, Option<PathBuf>)> {
    let entries = std::fs::read_dir("/tmp").ok()?;
    let mut agent_candidate: Option<(std::time::SystemTime, PathBuf, PathBuf)> = None;
    let mut plain_candidate: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(rest) = name.strip_prefix("mae-") else {
            continue;
        };
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };

        if let Some(pid_str) = rest.strip_suffix("-agent.sock") {
            if let Ok(pid) = pid_str.parse::<u32>() {
                if is_pid_alive(pid) {
                    let psk_path = PathBuf::from(format!("/tmp/mae-{pid}.psk"));
                    let is_newer = agent_candidate.as_ref().is_none_or(|(t, ..)| modified > *t);
                    if is_newer {
                        agent_candidate = Some((modified, path.clone(), psk_path));
                    }
                }
            }
        } else if let Some(pid_str) = rest.strip_suffix(".sock") {
            if let Ok(pid) = pid_str.parse::<u32>() {
                if is_pid_alive(pid) {
                    let is_newer = plain_candidate.as_ref().is_none_or(|(t, _)| modified > *t);
                    if is_newer {
                        plain_candidate = Some((modified, path.clone()));
                    }
                }
            }
        }
    }

    if let Some((_, sock, psk)) = agent_candidate {
        let psk = psk.exists().then_some(psk);
        return Some((sock, psk));
    }
    plain_candidate.map(|(_, sock)| (sock, None))
}

fn is_pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

impl McpClient {
    /// Connect to `socket_path`. If `psk` is `Some`, perform the ADR-048
    /// handshake (proves this is a first-party, same-machine client) and
    /// declare `declared_provider` at `initialize` time so it's trusted for
    /// `LocalModelsOnly` KBs. `psk` being `None` connects exactly like any
    /// existing MCP client (e.g. `mae-mcp-shim`) — no handshake attempted.
    pub async fn connect(
        socket_path: &Path,
        psk: Option<&str>,
        declared_provider: Option<&str>,
    ) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .await
            .with_context(|| format!("connecting to {}", socket_path.display()))?;
        let (read_half, write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let mut writer = write_half;

        if let Some(psk) = psk {
            PskAuth::new(psk)
                .client_handshake(&mut reader, &mut writer)
                .await
                .map_err(|e| anyhow::anyhow!("PSK handshake failed: {e}"))?;
        }

        let mut client = McpClient {
            reader,
            writer,
            next_id: 1,
        };
        client.initialize(declared_provider).await?;
        Ok(client)
    }

    async fn initialize(&mut self, declared_provider: Option<&str>) -> Result<()> {
        let mut params = serde_json::json!({
            "protocolVersion": mae_mcp::protocol::PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "mae-agent-cli", "version": env!("CARGO_PKG_VERSION") },
        });
        if let Some(provider) = declared_provider {
            params["declaredProvider"] = serde_json::Value::String(provider.to_string());
        }
        self.request("initialize", Some(params)).await?;
        self.notify("notifications/initialized", None).await?;
        Ok(())
    }

    pub async fn list_tools(&mut self) -> Result<Vec<ToolInfo>> {
        let result = self.request("tools/list", None).await?;
        let tools = result
            .get("tools")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("tools/list response missing 'tools'"))?;
        Ok(serde_json::from_value(tools)?)
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallOutcome> {
        let params = serde_json::json!({ "name": name, "arguments": arguments });
        let result = self.request("tools/call", Some(params)).await?;
        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        Ok(ToolCallOutcome {
            success: !is_error,
            text,
        })
    }

    async fn request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::Value::Number(id.into()),
            method: method.to_string(),
            params,
        };
        let body = serde_json::to_vec(&req)?;
        mae_mcp::write_framed(&mut self.writer, &body, WRITE_TIMEOUT).await?;

        let msg = mae_mcp::read_message(&mut self.reader)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("MCP connection closed while awaiting {method} reply")
            })?;
        let resp: JsonRpcResponse = serde_json::from_str(&msg)
            .with_context(|| format!("parsing {method} response: {msg}"))?;
        if let Some(err) = resp.error {
            bail!("{method} failed: {} ({})", err.message, err.code);
        }
        resp.result
            .ok_or_else(|| anyhow::anyhow!("{method} response had neither result nor error"))
    }

    /// Send a JSON-RPC notification (no `id`, no reply expected).
    async fn notify(&mut self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let body = serde_json::to_vec(&notification)?;
        mae_mcp::write_framed(&mut self.writer, &body, WRITE_TIMEOUT).await?;
        Ok(())
    }
}
