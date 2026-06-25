//! DaemonClient — synchronous JSON-RPC client over Unix socket.
//!
//! Connects to `mae-daemon` via Unix domain socket using Content-Length
//! framed JSON-RPC (same protocol as MCP server and daemon).
//!
//! Uses blocking I/O (`std::os::unix::net::UnixStream`) so it can be
//! called from synchronous `KbQueryLayer` trait methods without requiring
//! a tokio runtime in the calling context.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The default `mae-daemon` control-socket path — the **single source of truth**
/// shared by the daemon (which binds it) and every client (CLI + editor) so they
/// can never drift. Resolves `$XDG_RUNTIME_DIR/mae-daemon.sock` (e.g.
/// `/run/user/1000/mae-daemon.sock`), falling back to the temp dir when no runtime
/// dir is set — matching the daemon's `dirs::runtime_dir().unwrap_or(temp_dir)`.
pub fn default_daemon_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("mae-daemon.sock")
}

/// Error type for daemon client operations.
#[derive(Debug)]
pub enum DaemonClientError {
    /// Connection failed or was lost.
    ConnectionError(String),
    /// I/O error during read/write.
    IoError(std::io::Error),
    /// JSON serialization/deserialization error.
    JsonError(String),
    /// Server returned a JSON-RPC error.
    RpcError { code: i64, message: String },
    /// Request timed out.
    Timeout,
}

impl std::fmt::Display for DaemonClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionError(msg) => write!(f, "Connection error: {msg}"),
            Self::IoError(e) => write!(f, "I/O error: {e}"),
            Self::JsonError(msg) => write!(f, "JSON error: {msg}"),
            Self::RpcError { code, message } => write!(f, "RPC error {code}: {message}"),
            Self::Timeout => write!(f, "Request timed out"),
        }
    }
}

impl From<std::io::Error> for DaemonClientError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

/// Synchronous JSON-RPC client for communicating with `mae-daemon`.
pub struct DaemonClient {
    socket_path: PathBuf,
    stream: Option<BufReader<UnixStream>>,
    next_id: AtomicU64,
    timeout: Duration,
}

impl DaemonClient {
    /// Create a new client targeting the given socket path.
    /// Does not connect immediately — call `connect()` or `ensure_connected()`.
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            stream: None,
            next_id: AtomicU64::new(1),
            timeout: Duration::from_secs(10),
        }
    }

    /// Set the read/write timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Check if currently connected.
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// Socket path this client targets.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Connect to the daemon socket.
    pub fn connect(&mut self) -> Result<(), DaemonClientError> {
        let stream = UnixStream::connect(&self.socket_path).map_err(|e| {
            DaemonClientError::ConnectionError(format!(
                "Failed to connect to {}: {e}",
                self.socket_path.display()
            ))
        })?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        self.stream = Some(BufReader::new(stream));
        Ok(())
    }

    /// Disconnect from the daemon.
    pub fn disconnect(&mut self) {
        self.stream = None;
    }

    /// Ensure connected, reconnecting if needed.
    fn ensure_connected(&mut self) -> Result<(), DaemonClientError> {
        if self.stream.is_none() {
            self.connect()?;
        }
        Ok(())
    }

    /// Send a JSON-RPC request and return the result value.
    /// On connection error, attempts one reconnect.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, DaemonClientError> {
        // First attempt
        match self.call_inner(method, &params) {
            Ok(v) => Ok(v),
            Err(DaemonClientError::IoError(_) | DaemonClientError::ConnectionError(_)) => {
                // Reconnect and retry once
                self.disconnect();
                self.connect()?;
                self.call_inner(method, &params)
            }
            Err(e) => Err(e),
        }
    }

    /// Mint a P2P join ticket ("magnet link") for `kb_id` via the daemon's
    /// `p2p/mint_ticket` control method, returning the `mae://join/…` string.
    /// Shared by the editor's `DaemonControl` impl and the `mae` CLI so both
    /// drive the identical backend (ADR-025 §"Driving surfaces").
    pub fn mint_p2p_ticket(&mut self, kb_id: &str) -> Result<String, DaemonClientError> {
        let result = self.call("p2p/mint_ticket", json!({ "kb_id": kb_id }))?;
        result
            .get("ticket")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| DaemonClientError::RpcError {
                code: -32603,
                message: "daemon p2p/mint_ticket returned no ticket".to_string(),
            })
    }

    /// Queue a P2P join from a `ticket` ("magnet link") via the daemon's
    /// `p2p/join_ticket` control method. The background dialer then connects + pulls
    /// the KB (after the owner approves). Returns the daemon's human-readable
    /// confirmation message. Shared by the editor's `DaemonControl` impl and the
    /// `mae` CLI (ADR-025 §"Driving surfaces").
    pub fn join_p2p_ticket(&mut self, ticket: &str) -> Result<String, DaemonClientError> {
        let result = self.call("p2p/join_ticket", json!({ "ticket": ticket }))?;
        // Prefer the daemon's friendly message; fall back to a kb_id confirmation.
        if let Some(msg) = result.get("message").and_then(|v| v.as_str()) {
            return Ok(msg.to_string());
        }
        let kb_id = result
            .get("kb_id")
            .and_then(|v| v.as_str())
            .unwrap_or("the KB");
        Ok(format!("Join recorded for {kb_id}."))
    }

    fn call_inner(&mut self, method: &str, params: &Value) -> Result<Value, DaemonClientError> {
        self.ensure_connected()?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let body = serde_json::to_vec(&request)
            .map_err(|e| DaemonClientError::JsonError(e.to_string()))?;

        // Write Content-Length framed message
        let reader = self.stream.as_mut().unwrap();
        let writer = reader.get_mut();
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        writer.write_all(header.as_bytes())?;
        writer.write_all(&body)?;
        writer.flush()?;

        // Read response (Content-Length framed)
        let response_str = read_cl_message(self.stream.as_mut().unwrap())?;

        let response: Value = serde_json::from_str(&response_str)
            .map_err(|e| DaemonClientError::JsonError(e.to_string()))?;

        // Check for JSON-RPC error
        if let Some(err) = response.get("error") {
            let code = err["code"].as_i64().unwrap_or(-32603);
            let message = err["message"]
                .as_str()
                .unwrap_or("Unknown error")
                .to_string();
            return Err(DaemonClientError::RpcError { code, message });
        }

        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// Read a single Content-Length framed message from a buffered reader.
fn read_cl_message<R: BufRead>(reader: &mut R) -> Result<String, DaemonClientError> {
    let mut content_length: Option<usize> = None;

    // Read headers
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(DaemonClientError::ConnectionError(
                "EOF while reading headers".to_string(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break; // Empty line = end of headers
        }
        if let Some(val) = trimmed.strip_prefix("Content-Length:") {
            content_length = val.trim().parse().ok();
        }
    }

    let len = content_length.ok_or_else(|| {
        DaemonClientError::ConnectionError("Missing Content-Length header".to_string())
    })?;

    // Guard against unreasonable message sizes (100 MB limit)
    const MAX_MESSAGE_SIZE: usize = 100 * 1024 * 1024;
    if len > MAX_MESSAGE_SIZE {
        return Err(DaemonClientError::ConnectionError(format!(
            "Content-Length {len} exceeds maximum {MAX_MESSAGE_SIZE}"
        )));
    }

    // Read body
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;

    String::from_utf8(body)
        .map_err(|e| DaemonClientError::JsonError(format!("Invalid UTF-8 in response: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_initial_state() {
        let client = DaemonClient::new("/tmp/nonexistent.sock");
        assert!(!client.is_connected());
        assert_eq!(client.socket_path(), Path::new("/tmp/nonexistent.sock"));
    }

    #[test]
    fn connect_to_nonexistent_fails() {
        let mut client = DaemonClient::new("/tmp/mae-test-nonexistent.sock");
        let result = client.connect();
        assert!(result.is_err());
        assert!(!client.is_connected());
    }

    #[test]
    fn error_display() {
        let e = DaemonClientError::RpcError {
            code: -32601,
            message: "Method not found".into(),
        };
        assert_eq!(format!("{e}"), "RPC error -32601: Method not found");
    }
}
