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
    discover_connection_in(Path::new("/tmp"))
}

/// `discover_connection`, parameterized on the scan directory so the
/// newest-wins / agent-socket-preferred logic is testable against a tempdir
/// of fixture files instead of the real `/tmp`.
fn discover_connection_in(base: &Path) -> Option<(PathBuf, Option<PathBuf>)> {
    let entries = std::fs::read_dir(base).ok()?;
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
                    let psk_path = base.join(format!("mae-{pid}.psk"));
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

/// Extract a `tools/call` outcome from its raw JSON-RPC `result` value:
/// `isError` (default `false`) and `content[0].text` (default `""`).
fn parse_tool_call_result(result: &serde_json::Value) -> ToolCallOutcome {
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
    ToolCallOutcome {
        success: !is_error,
        text,
    }
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
        Self::from_stream(stream, psk, declared_provider).await
    }

    /// Shared post-connect logic: split, optionally PSK-handshake, initialize.
    /// Split out from `connect` so tests can drive it over an in-process
    /// `UnixStream::pair()` instead of a real filesystem socket.
    async fn from_stream(
        stream: UnixStream,
        psk: Option<&str>,
        declared_provider: Option<&str>,
    ) -> Result<Self> {
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
        Ok(parse_tool_call_result(&result))
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ---- pure logic: is_pid_alive / parse_tool_call_result ----

    #[test]
    fn is_pid_alive_true_for_own_process() {
        assert!(is_pid_alive(std::process::id()));
    }

    #[test]
    fn is_pid_alive_false_for_implausible_pid() {
        assert!(!is_pid_alive(u32::MAX));
    }

    #[test]
    fn parse_tool_call_result_success_with_text() {
        let value = serde_json::json!({
            "isError": false,
            "content": [{"type": "text", "text": "3 results"}],
        });
        let outcome = parse_tool_call_result(&value);
        assert!(outcome.success);
        assert_eq!(outcome.text, "3 results");
    }

    #[test]
    fn parse_tool_call_result_error_flag_sets_failure() {
        let value = serde_json::json!({
            "isError": true,
            "content": [{"type": "text", "text": "tool blew up"}],
        });
        let outcome = parse_tool_call_result(&value);
        assert!(!outcome.success);
        assert_eq!(outcome.text, "tool blew up");
    }

    #[test]
    fn parse_tool_call_result_missing_fields_default_safely() {
        let outcome = parse_tool_call_result(&serde_json::json!({}));
        assert!(outcome.success, "missing isError defaults to success");
        assert_eq!(outcome.text, "");
    }

    // ---- discover_connection_in ----

    #[test]
    fn discover_connection_in_returns_none_when_empty() {
        let dir = tempdir().unwrap();
        assert!(discover_connection_in(dir.path()).is_none());
    }

    #[test]
    fn discover_connection_in_falls_back_to_plain_when_no_agent_socket() {
        let dir = tempdir().unwrap();
        let pid = std::process::id();
        std::fs::write(dir.path().join(format!("mae-{pid}.sock")), b"").unwrap();

        let (sock, psk) = discover_connection_in(dir.path()).expect("should find plain socket");
        assert_eq!(sock, dir.path().join(format!("mae-{pid}.sock")));
        assert!(psk.is_none());
    }

    #[test]
    fn discover_connection_in_prefers_agent_socket_with_psk() {
        let dir = tempdir().unwrap();
        let pid = std::process::id();
        std::fs::write(dir.path().join(format!("mae-{pid}.sock")), b"").unwrap();
        std::fs::write(dir.path().join(format!("mae-{pid}-agent.sock")), b"").unwrap();
        std::fs::write(dir.path().join(format!("mae-{pid}.psk")), b"secret").unwrap();

        let (sock, psk) = discover_connection_in(dir.path()).expect("should find agent socket");
        assert_eq!(sock, dir.path().join(format!("mae-{pid}-agent.sock")));
        assert_eq!(psk, Some(dir.path().join(format!("mae-{pid}.psk"))));
    }

    #[test]
    fn discover_connection_in_agent_socket_without_psk_file_reports_none() {
        let dir = tempdir().unwrap();
        let pid = std::process::id();
        std::fs::write(dir.path().join(format!("mae-{pid}-agent.sock")), b"").unwrap();
        // No matching .psk file written.

        let (sock, psk) = discover_connection_in(dir.path()).unwrap();
        assert_eq!(sock, dir.path().join(format!("mae-{pid}-agent.sock")));
        assert!(psk.is_none());
    }

    #[test]
    fn discover_connection_in_ignores_dead_pid_entries() {
        let dir = tempdir().unwrap();
        // An implausible PID: nothing alive there.
        std::fs::write(dir.path().join("mae-4000000000.sock"), b"").unwrap();
        assert!(discover_connection_in(dir.path()).is_none());
    }

    #[test]
    fn discover_connection_in_skips_malformed_pid_with_extra_dash() {
        let dir = tempdir().unwrap();
        // `strip_suffix("-agent.sock")` on "mae-1-2-agent.sock" leaves "1-2",
        // which fails `u32::parse`. Must be skipped silently, not panic.
        std::fs::write(dir.path().join("mae-1-2-agent.sock"), b"").unwrap();
        assert!(discover_connection_in(dir.path()).is_none());
    }

    #[test]
    fn discover_connection_in_newest_agent_socket_wins_between_two_live_pids() {
        // Exercise the `is_newer` mtime-comparison branch against a SECOND
        // genuinely live pid (not just `None`, which every other test above
        // compares against). PID 1 (init) is always alive on any Linux box,
        // including containers, so this doesn't depend on any process this
        // test itself spawns.
        let own_pid = std::process::id();
        let init_pid = 1u32;

        // Scenario A: own pid's socket is written second (newer) -> wins.
        {
            let dir = tempdir().unwrap();
            std::fs::write(dir.path().join(format!("mae-{init_pid}-agent.sock")), b"").unwrap();
            std::thread::sleep(Duration::from_millis(50));
            std::fs::write(dir.path().join(format!("mae-{own_pid}-agent.sock")), b"").unwrap();

            let (sock, _psk) = discover_connection_in(dir.path()).expect("should find a candidate");
            assert_eq!(
                sock,
                dir.path().join(format!("mae-{own_pid}-agent.sock")),
                "newer own-pid socket should win over the older pid-1 socket"
            );
        }

        // Scenario B: pid 1's socket is written second (newer) -> wins.
        // Same comparison, opposite direction, so the branch isn't only ever
        // exercised with "our pid happens to always be newer".
        {
            let dir = tempdir().unwrap();
            std::fs::write(dir.path().join(format!("mae-{own_pid}-agent.sock")), b"").unwrap();
            std::thread::sleep(Duration::from_millis(50));
            std::fs::write(dir.path().join(format!("mae-{init_pid}-agent.sock")), b"").unwrap();

            let (sock, _psk) = discover_connection_in(dir.path()).expect("should find a candidate");
            assert_eq!(
                sock,
                dir.path().join(format!("mae-{init_pid}-agent.sock")),
                "newer pid-1 socket should win over the older own-pid socket"
            );
        }
    }

    // ---- real transport, faked in-process (no real filesystem socket) ----

    /// Reads framed JSON-RPC messages from `reader` until EOF, replying to
    /// every request (a message with an "id") via `respond`, and recording
    /// every message it saw (including notifications, which get no reply).
    /// Returns once the connection closes — real `McpClient`s never close
    /// proactively, so tests must `drop` their client to end this loop.
    async fn run_fake_server(
        mut reader: BufReader<OwnedReadHalf>,
        mut writer: OwnedWriteHalf,
        mut respond: impl FnMut(&str, &serde_json::Value) -> serde_json::Value,
    ) -> Vec<serde_json::Value> {
        let mut seen = Vec::new();
        loop {
            let Some(msg) = mae_mcp::read_message(&mut reader).await.unwrap() else {
                break;
            };
            let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
            if let Some(id) = parsed.get("id").cloned() {
                let method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let params = parsed
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let result = respond(method, &params);
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                });
                let body = serde_json::to_vec(&response).unwrap();
                mae_mcp::write_framed(&mut writer, &body, Duration::from_secs(5))
                    .await
                    .unwrap();
            }
            seen.push(parsed);
        }
        seen
    }

    /// Variant of `run_fake_server` whose `respond` closure returns the full
    /// JSON-RPC response *body* (an object with a top-level `"result"` or
    /// `"error"` key) rather than always being wrapped in a success
    /// `"result"` — lets tests drive `McpClient::request`'s error-handling
    /// branches (`resp.error` set, or neither `result` nor `error` present)
    /// that `run_fake_server` above has no way to produce.
    async fn run_fake_server_with_raw_response(
        mut reader: BufReader<OwnedReadHalf>,
        mut writer: OwnedWriteHalf,
        mut respond: impl FnMut(&str, &serde_json::Value) -> serde_json::Value,
    ) -> Vec<serde_json::Value> {
        let mut seen = Vec::new();
        loop {
            let Some(msg) = mae_mcp::read_message(&mut reader).await.unwrap() else {
                break;
            };
            let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
            if let Some(id) = parsed.get("id").cloned() {
                let method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let params = parsed
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let mut response = respond(method, &params);
                response["jsonrpc"] = serde_json::json!("2.0");
                response["id"] = id;
                let body = serde_json::to_vec(&response).unwrap();
                mae_mcp::write_framed(&mut writer, &body, Duration::from_secs(5))
                    .await
                    .unwrap();
            }
            seen.push(parsed);
        }
        seen
    }

    #[tokio::test]
    async fn request_error_response_surfaces_code_and_message() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (server_read, server_write) = server_stream.into_split();
        let server_reader = BufReader::new(server_read);

        let server = tokio::spawn(run_fake_server_with_raw_response(
            server_reader,
            server_write,
            |method, _params| match method {
                "tools/call" => serde_json::json!({
                    "error": {"code": -32000, "message": "boom"},
                }),
                _ => serde_json::json!({ "result": {} }),
            },
        ));

        let mut client = McpClient::from_stream(client_stream, None, None)
            .await
            .expect("connect should succeed");

        let err = client
            .call_tool("kb_search", serde_json::json!({"query": "x"}))
            .await
            .expect_err("tools/call error response should surface as Err");
        // Exact `bail!` format from `McpClient::request`.
        assert_eq!(err.to_string(), "tools/call failed: boom (-32000)");

        drop(client);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_with_neither_result_nor_error_is_reported_as_such() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (server_read, server_write) = server_stream.into_split();
        let server_reader = BufReader::new(server_read);

        let server = tokio::spawn(run_fake_server_with_raw_response(
            server_reader,
            server_write,
            |method, _params| match method {
                // Neither "result" nor "error" — malformed-but-parseable reply.
                "tools/list" => serde_json::json!({}),
                _ => serde_json::json!({ "result": {} }),
            },
        ));

        let mut client = McpClient::from_stream(client_stream, None, None)
            .await
            .expect("connect should succeed");

        let err = client
            .list_tools()
            .await
            .expect_err("response with neither result nor error should be an Err");
        assert_eq!(
            err.to_string(),
            "tools/list response had neither result nor error"
        );

        drop(client);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn client_connects_lists_tools_and_calls_one_over_a_paired_socket() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (server_read, server_write) = server_stream.into_split();
        let server_reader = BufReader::new(server_read);

        let server = tokio::spawn(run_fake_server(
            server_reader,
            server_write,
            |method, _params| match method {
                "tools/list" => serde_json::json!({
                    "tools": [{
                        "name": "kb_search",
                        "description": "search",
                        "inputSchema": {"type": "object", "properties": {}},
                    }]
                }),
                "tools/call" => serde_json::json!({
                    "isError": false,
                    "content": [{"type": "text", "text": "found 2 nodes"}],
                }),
                _ => serde_json::json!({}),
            },
        ));

        let mut client = McpClient::from_stream(client_stream, None, Some("ollama"))
            .await
            .expect("connect should succeed");

        let tools = client
            .list_tools()
            .await
            .expect("list_tools should succeed");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "kb_search");

        let outcome = client
            .call_tool("kb_search", serde_json::json!({"query": "x"}))
            .await
            .expect("call_tool should succeed");
        assert!(outcome.success);
        assert_eq!(outcome.text, "found 2 nodes");

        drop(client); // closes the socket -> the fake server's read loop hits EOF
        let seen = server.await.unwrap();

        // The `initialize` request should have carried the declared provider
        // (ADR-048), and the follow-up notification should have been seen too
        // (even though it got no reply).
        let init = seen
            .iter()
            .find(|m| m.get("method").and_then(|v| v.as_str()) == Some("initialize"))
            .expect("initialize was sent");
        assert_eq!(init["params"]["declaredProvider"].as_str(), Some("ollama"));
        assert!(
            seen.iter()
                .any(|m| m.get("method").and_then(|v| v.as_str())
                    == Some("notifications/initialized"))
        );
    }

    #[tokio::test]
    async fn full_turn_round_trips_through_the_real_socket_transport() {
        use mae_ai::{
            AgentProvider, Message, MessageContent, ProviderError, ProviderResponse, StopReason,
            ToolCall, ToolDefinition,
        };
        use std::sync::Mutex as StdMutex;

        /// Deterministic scripted provider — same shape as agent_loop.rs's own
        /// test-only `ScriptedProvider`, kept local since that one lives in a
        /// private test module of a different file.
        struct ScriptedProvider(StdMutex<Vec<ProviderResponse>>);

        #[async_trait::async_trait]
        impl AgentProvider for ScriptedProvider {
            async fn send(
                &self,
                _messages: &[Message],
                _tools: &[ToolDefinition],
                _system_prompt: &str,
            ) -> std::result::Result<ProviderResponse, ProviderError> {
                Ok(self.0.lock().unwrap().remove(0))
            }
            fn name(&self) -> &str {
                "scripted"
            }
        }

        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (server_read, server_write) = server_stream.into_split();
        let server_reader = BufReader::new(server_read);
        let server = tokio::spawn(run_fake_server(
            server_reader,
            server_write,
            |method, _params| match method {
                "tools/call" => serde_json::json!({
                    "isError": false,
                    "content": [{"type": "text", "text": "3 nodes found"}],
                }),
                _ => serde_json::json!({}),
            },
        ));

        let mut client = McpClient::from_stream(client_stream, None, None)
            .await
            .expect("connect should succeed");

        let provider = ScriptedProvider(StdMutex::new(vec![
            ProviderResponse {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "kb_search".into(),
                    arguments: serde_json::json!({"query": "buffer"}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            ProviderResponse {
                text: Some("Found 3 matching nodes.".into()),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]));

        let mut messages = Vec::new();
        let mut events = Vec::new();
        crate::agent_loop::run_turn(
            crate::agent_loop::TurnContext {
                provider: &provider,
                executor: &mut client,
                tools: &[],
                system_prompt: "system",
            },
            &mut messages,
            &crate::agent_loop::TurnConfig::default(),
            "search for buffer",
            |e| events.push(e),
        )
        .await
        .expect("turn should complete");

        assert!(events.iter().any(|e| matches!(
            e,
            crate::agent_loop::AgentEvent::ToolCallFinished { name, success: true, output }
                if name == "kb_search" && output == "3 nodes found"
        )));
        assert!(matches!(
            messages.last().map(|m| &m.content),
            Some(MessageContent::Text(t)) if t == "Found 3 matching nodes."
        ));

        drop(client);
        server.await.unwrap();
    }
}
