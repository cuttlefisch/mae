//! DAP client — manages a debug adapter subprocess and message exchange.
//!
//! The client spawns a debug adapter process, performs the initialize
//! handshake, and provides methods for launching/attaching targets,
//! setting breakpoints, and driving execution. Incoming messages
//! (responses and events) are routed to a channel for the editor
//! event loop to consume.
//!
//! Structured after `mae_lsp::LspClient`, but DAP correlates responses
//! via `request_seq` rather than JSON-RPC `id`, and exposes a stream
//! of events (stopped, output, terminated, exited, etc.) that the
//! editor drives its debug UI from.
//!
//! Lifecycle:
//!     start() -> initialize -> wait for `initialized` event
//!                -> caller issues configuration (setBreakpoints, etc.)
//!                -> configurationDone()
//!                -> ... normal operation ...
//!                -> disconnect() / terminate() at shutdown

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::protocol::*;
use crate::transport::{DapTransport, TransportError};

/// Shared map from request `seq` to a oneshot sender awaiting its response.
type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<DapResponse>>>>;

/// Events the editor receives from the DAP client. Responses correlated
/// via `request_seq` do NOT appear here — they're routed back to the
/// awaiting `request()` caller via oneshot.
#[derive(Debug)]
pub enum DapEventKind {
    /// Adapter sent an event (stopped, output, terminated, exited, ...).
    Event(DapEvent),
    /// Adapter sent a response that had no awaiting requester.
    OrphanResponse(DapResponse),
    /// Adapter sent a reverse request (e.g. runInTerminal). Rare.
    ReverseRequest(DapRequest),
    /// Transport error — adapter probably died.
    Error(String),
    /// Adapter process exited / connection closed.
    AdapterExited,
}

/// Configuration for spawning a debug adapter.
#[derive(Debug, Clone)]
pub struct DapServerConfig {
    /// The command to run (e.g. "lldb-dap", "codelldb", "debugpy-adapter").
    pub command: String,
    /// Arguments to pass to the command.
    pub args: Vec<String>,
    /// Adapter id reported in the initialize handshake (informational).
    pub adapter_id: String,
}

/// An active DAP client connected to a debug adapter.
pub struct DapClient {
    /// Sender feeding the writer task with already-serialized messages.
    outgoing_tx: mpsc::Sender<Vec<u8>>,
    /// Receiver for events / orphan responses. The editor event loop
    /// drains this in the main `tokio::select!`.
    pub event_rx: mpsc::Receiver<DapEventKind>,
    /// Next outgoing `seq` value.
    next_seq: AtomicI64,
    /// Pending requests awaiting responses.
    pending: PendingMap,
    /// Capabilities returned by the adapter in the initialize response.
    pub capabilities: Option<Capabilities>,
    /// Whether the adapter has emitted the `initialized` event yet.
    /// `configuration_done()` should only be sent after this flips true.
    pub initialized: bool,
    /// Child process handle — kept alive until drop.
    _child: Option<Child>,
}

impl DapClient {
    /// Spawn a debug adapter and perform the initialize handshake.
    pub async fn start(config: DapServerConfig) -> Result<Self, String> {
        let mut child = Command::new(&config.command)
            .args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to spawn debug adapter '{}': {}", config.command, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or("no stdin on debug adapter subprocess")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("no stdout on debug adapter subprocess")?;

        let (event_tx, event_rx) = mpsc::channel(256);
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<Vec<u8>>(64);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        spawn_reader_task(stdout, event_tx.clone(), pending.clone());
        spawn_writer_task(stdin, outgoing_rx);

        let mut client = DapClient {
            outgoing_tx,
            event_rx,
            next_seq: AtomicI64::new(1),
            pending,
            capabilities: None,
            initialized: false,
            _child: Some(child),
        };

        client.initialize(&config).await?;

        Ok(client)
    }

    /// For testing: construct a client around pre-connected in-memory streams
    /// instead of a real subprocess.
    #[doc(hidden)]
    pub async fn from_streams<R, W>(reader: R, writer: W, adapter_id: &str) -> Result<Self, String>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (event_tx, event_rx) = mpsc::channel(256);
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<Vec<u8>>(64);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        spawn_reader_task(reader, event_tx.clone(), pending.clone());
        spawn_writer_task(writer, outgoing_rx);

        let mut client = DapClient {
            outgoing_tx,
            event_rx,
            next_seq: AtomicI64::new(1),
            pending,
            capabilities: None,
            initialized: false,
            _child: None,
        };

        let config = DapServerConfig {
            command: String::new(),
            args: Vec::new(),
            adapter_id: adapter_id.to_string(),
        };
        client.initialize(&config).await?;

        Ok(client)
    }

    /// Issue the `initialize` request. The adapter replies with capabilities
    /// and later (asynchronously) emits the `initialized` event, at which
    /// point the editor may send configuration (setBreakpoints, etc.) and
    /// finally `configurationDone`.
    async fn initialize(&mut self, config: &DapServerConfig) -> Result<(), String> {
        let args = InitializeRequestArguments {
            client_id: Some("mae".into()),
            client_name: Some("MAE Editor".into()),
            adapter_id: Some(config.adapter_id.clone()),
            lines_start_at1: true,
            columns_start_at1: true,
            supports_variable_type: true,
            supports_variable_paging: false,
            supports_run_in_terminal_request: false,
            supports_memory_references: false,
            supports_progress_reporting: false,
            supports_invalidated_event: false,
        };
        let resp = self
            .request(
                "initialize",
                Some(serde_json::to_value(&args).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(30),
            )
            .await
            .map_err(|e| format!("initialize failed: {}", e))?;

        if !resp.success {
            return Err(format!(
                "initialize rejected: {}",
                resp.message.unwrap_or_default()
            ));
        }
        if let Some(body) = resp.body {
            if let Ok(caps) = serde_json::from_value::<Capabilities>(body) {
                self.capabilities = Some(caps);
            }
        }
        Ok(())
    }

    /// Record that the adapter has emitted the `initialized` event.
    /// The editor event loop calls this after observing the event.
    pub fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    /// Send `launch` with adapter-specific arguments.
    pub async fn launch(&self, args: serde_json::Value) -> Result<DapResponse, String> {
        self.request("launch", Some(args), std::time::Duration::from_secs(30))
            .await
    }

    /// Send `attach` with adapter-specific arguments.
    pub async fn attach(&self, args: serde_json::Value) -> Result<DapResponse, String> {
        self.request("attach", Some(args), std::time::Duration::from_secs(30))
            .await
    }

    /// Send `configurationDone`. Only call after the adapter has emitted the
    /// `initialized` event AND after all pre-run configuration requests
    /// (e.g. setBreakpoints) have succeeded.
    pub async fn configuration_done(&self) -> Result<DapResponse, String> {
        self.request(
            "configurationDone",
            None,
            std::time::Duration::from_secs(10),
        )
        .await
    }

    /// Send `setBreakpoints` for a single source file.
    pub async fn set_breakpoints(
        &self,
        source_path: &str,
        breakpoints: Vec<SourceBreakpoint>,
    ) -> Result<SetBreakpointsResponseBody, String> {
        let args = SetBreakpointsArguments {
            source: Source {
                name: std::path::Path::new(source_path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(String::from),
                path: Some(source_path.to_string()),
            },
            breakpoints: Some(breakpoints),
        };
        let resp = self
            .request(
                "setBreakpoints",
                Some(serde_json::to_value(&args).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(10),
            )
            .await?;
        if !resp.success {
            return Err(format!(
                "setBreakpoints rejected: {}",
                resp.message.unwrap_or_default()
            ));
        }
        let body = resp
            .body
            .ok_or_else(|| "setBreakpoints response missing body".to_string())?;
        serde_json::from_value(body).map_err(|e| format!("malformed setBreakpoints body: {}", e))
    }

    /// Request the adapter's thread list.
    pub async fn threads(&self) -> Result<Vec<DapThread>, String> {
        let resp = self
            .request("threads", None, std::time::Duration::from_secs(5))
            .await?;
        if !resp.success {
            return Err(format!(
                "threads rejected: {}",
                resp.message.unwrap_or_default()
            ));
        }
        let body = resp
            .body
            .ok_or_else(|| "threads response missing body".to_string())?;
        let parsed: ThreadsResponseBody =
            serde_json::from_value(body).map_err(|e| format!("malformed threads body: {}", e))?;
        Ok(parsed.threads)
    }

    /// Request a stack trace for a thread.
    pub async fn stack_trace(
        &self,
        thread_id: i64,
        levels: Option<i64>,
    ) -> Result<StackTraceResponseBody, String> {
        let args = StackTraceArguments {
            thread_id,
            start_frame: None,
            levels,
        };
        let resp = self
            .request(
                "stackTrace",
                Some(serde_json::to_value(&args).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(5),
            )
            .await?;
        if !resp.success {
            return Err(format!(
                "stackTrace rejected: {}",
                resp.message.unwrap_or_default()
            ));
        }
        let body = resp
            .body
            .ok_or_else(|| "stackTrace response missing body".to_string())?;
        serde_json::from_value(body).map_err(|e| format!("malformed stackTrace body: {}", e))
    }

    /// Request the scopes list for a stack frame.
    pub async fn scopes(&self, frame_id: i64) -> Result<Vec<DapScope>, String> {
        let args = ScopesArguments { frame_id };
        let resp = self
            .request(
                "scopes",
                Some(serde_json::to_value(&args).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(5),
            )
            .await?;
        if !resp.success {
            return Err(format!(
                "scopes rejected: {}",
                resp.message.unwrap_or_default()
            ));
        }
        let body = resp
            .body
            .ok_or_else(|| "scopes response missing body".to_string())?;
        let parsed: ScopesResponseBody =
            serde_json::from_value(body).map_err(|e| format!("malformed scopes body: {}", e))?;
        Ok(parsed.scopes)
    }

    /// Request the variables for a variables_reference (from a scope or a
    /// compound variable).
    pub async fn variables(&self, variables_reference: i64) -> Result<Vec<DapVariable>, String> {
        let args = VariablesArguments {
            variables_reference,
        };
        let resp = self
            .request(
                "variables",
                Some(serde_json::to_value(&args).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(5),
            )
            .await?;
        if !resp.success {
            return Err(format!(
                "variables rejected: {}",
                resp.message.unwrap_or_default()
            ));
        }
        let body = resp
            .body
            .ok_or_else(|| "variables response missing body".to_string())?;
        let parsed: VariablesResponseBody =
            serde_json::from_value(body).map_err(|e| format!("malformed variables body: {}", e))?;
        Ok(parsed.variables)
    }

    /// Resume execution.
    pub async fn continue_(&self, thread_id: i64) -> Result<DapResponse, String> {
        let args = ContinueArguments { thread_id };
        self.request(
            "continue",
            Some(serde_json::to_value(&args).map_err(|e| e.to_string())?),
            std::time::Duration::from_secs(5),
        )
        .await
    }

    /// Step over.
    pub async fn next(&self, thread_id: i64) -> Result<DapResponse, String> {
        let args = NextArguments { thread_id };
        self.request(
            "next",
            Some(serde_json::to_value(&args).map_err(|e| e.to_string())?),
            std::time::Duration::from_secs(5),
        )
        .await
    }

    /// Step in.
    pub async fn step_in(&self, thread_id: i64) -> Result<DapResponse, String> {
        let args = StepInArguments { thread_id };
        self.request(
            "stepIn",
            Some(serde_json::to_value(&args).map_err(|e| e.to_string())?),
            std::time::Duration::from_secs(5),
        )
        .await
    }

    /// Step out.
    pub async fn step_out(&self, thread_id: i64) -> Result<DapResponse, String> {
        let args = StepOutArguments { thread_id };
        self.request(
            "stepOut",
            Some(serde_json::to_value(&args).map_err(|e| e.to_string())?),
            std::time::Duration::from_secs(5),
        )
        .await
    }

    /// Ask the adapter to terminate the debuggee (soft, if supported).
    pub async fn terminate(&self) -> Result<DapResponse, String> {
        self.request("terminate", None, std::time::Duration::from_secs(5))
            .await
    }

    /// Disconnect from the adapter (hard shutdown). The adapter is expected
    /// to exit its process after responding.
    pub async fn disconnect(&self, terminate_debuggee: bool) -> Result<DapResponse, String> {
        let args = serde_json::json!({ "terminateDebuggee": terminate_debuggee });
        self.request(
            "disconnect",
            Some(args),
            std::time::Duration::from_secs(5),
        )
        .await
    }

    /// Send a request and wait for the matching response via oneshot.
    pub async fn request(
        &self,
        command: &str,
        arguments: Option<serde_json::Value>,
        timeout: std::time::Duration,
    ) -> Result<DapResponse, String> {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(seq, tx);

        let msg = DapMessage::Request(DapRequest {
            seq,
            command: command.to_string(),
            arguments,
        });
        let bytes = match encode_message(&msg) {
            Ok(b) => b,
            Err(e) => {
                self.pending.lock().await.remove(&seq);
                return Err(e);
            }
        };
        if self.outgoing_tx.send(bytes).await.is_err() {
            self.pending.lock().await.remove(&seq);
            return Err("adapter writer channel closed".into());
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&seq);
                Err("response channel closed".into())
            }
            Err(_) => {
                self.pending.lock().await.remove(&seq);
                Err(format!("request '{}' timed out", command))
            }
        }
    }
}

/// Serialize a DAP message to the wire format: `Content-Length: N\r\n\r\n<body>`.
fn encode_message(msg: &DapMessage) -> Result<Vec<u8>, String> {
    let body = serde_json::to_vec(msg).map_err(|e| format!("serialize failed: {}", e))?;
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    Ok(out)
}

fn spawn_reader_task<R>(reader: R, event_tx: mpsc::Sender<DapEventKind>, pending: PendingMap)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut transport = DapTransport::new(reader, tokio::io::sink());
        loop {
            match transport.read_message().await {
                Ok(DapMessage::Response(r)) => {
                    let tx = pending.lock().await.remove(&r.request_seq);
                    if let Some(tx) = tx {
                        let _ = tx.send(r);
                    } else if event_tx.send(DapEventKind::OrphanResponse(r)).await.is_err() {
                        break;
                    }
                }
                Ok(DapMessage::Event(e)) => {
                    if event_tx.send(DapEventKind::Event(e)).await.is_err() {
                        break;
                    }
                }
                Ok(DapMessage::Request(r)) => {
                    if event_tx.send(DapEventKind::ReverseRequest(r)).await.is_err() {
                        break;
                    }
                }
                Err(TransportError::ConnectionClosed) => {
                    let _ = event_tx.send(DapEventKind::AdapterExited).await;
                    break;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(DapEventKind::Error(format!("{}", e)))
                        .await;
                    break;
                }
            }
        }
    });
}

fn spawn_writer_task<W>(writer: W, mut rx: mpsc::Receiver<Vec<u8>>)
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use tokio::io::AsyncWriteExt;
    tokio::spawn(async move {
        let mut w = writer;
        while let Some(bytes) = rx.recv().await {
            if w.write_all(&bytes).await.is_err() {
                break;
            }
            if w.flush().await.is_err() {
                break;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny hand-rolled mock adapter: reads messages off an in-memory
    /// stream and produces scripted responses and events. Each scripted
    /// reply is an `Action`.
    enum Action {
        /// Respond to the next incoming request with this body (success).
        Respond(serde_json::Value),
        /// Respond success=true with no body.
        RespondOk,
        /// Respond success=false with this message.
        RespondErr(&'static str),
        /// Emit an event (independent of any request).
        EmitEvent(&'static str, serde_json::Value),
    }

    /// Run a mock adapter in a background task. Returns streams the client
    /// should wrap: (adapter_stdout_for_client, adapter_stdin_from_client).
    fn spawn_mock_adapter(
        script: Vec<Action>,
    ) -> (
        impl tokio::io::AsyncRead + Unpin + Send + 'static,
        impl tokio::io::AsyncWrite + Unpin + Send + 'static,
    ) {
        // client reads from client_read, writes to client_write.
        let (client_side, adapter_side) = tokio::io::duplex(8192);
        let (client_read, client_write) = tokio::io::split(client_side);
        let (adapter_read, adapter_write) = tokio::io::split(adapter_side);

        tokio::spawn(async move {
            let mut transport = DapTransport::new(adapter_read, adapter_write);
            let mut out_seq: i64 = 1000;
            for action in script {
                match action {
                    Action::EmitEvent(name, body) => {
                        let msg = DapMessage::Event(DapEvent {
                            seq: out_seq,
                            event: name.to_string(),
                            body: Some(body),
                        });
                        out_seq += 1;
                        if transport.write_message(&msg).await.is_err() {
                            break;
                        }
                    }
                    Action::Respond(body) => {
                        // Wait for a request first
                        match transport.read_message().await {
                            Ok(DapMessage::Request(r)) => {
                                let resp = DapMessage::Response(DapResponse {
                                    seq: out_seq,
                                    request_seq: r.seq,
                                    success: true,
                                    command: r.command,
                                    message: None,
                                    body: Some(body),
                                });
                                out_seq += 1;
                                if transport.write_message(&resp).await.is_err() {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                    Action::RespondOk => match transport.read_message().await {
                        Ok(DapMessage::Request(r)) => {
                            let resp = DapMessage::Response(DapResponse {
                                seq: out_seq,
                                request_seq: r.seq,
                                success: true,
                                command: r.command,
                                message: None,
                                body: None,
                            });
                            out_seq += 1;
                            if transport.write_message(&resp).await.is_err() {
                                break;
                            }
                        }
                        _ => break,
                    },
                    Action::RespondErr(err) => match transport.read_message().await {
                        Ok(DapMessage::Request(r)) => {
                            let resp = DapMessage::Response(DapResponse {
                                seq: out_seq,
                                request_seq: r.seq,
                                success: false,
                                command: r.command,
                                message: Some(err.to_string()),
                                body: None,
                            });
                            out_seq += 1;
                            if transport.write_message(&resp).await.is_err() {
                                break;
                            }
                        }
                        _ => break,
                    },
                }
            }
            // Keep the stream open briefly so late reads don't fail spuriously.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });

        (client_read, client_write)
    }

    fn init_caps() -> serde_json::Value {
        serde_json::json!({
            "supportsConfigurationDoneRequest": true,
            "supportsConditionalBreakpoints": true,
            "supportsTerminateRequest": true,
        })
    }

    #[tokio::test]
    async fn start_performs_initialize_handshake_and_parses_capabilities() {
        let (r, w) = spawn_mock_adapter(vec![Action::Respond(init_caps())]);
        let client = DapClient::from_streams(r, w, "mock").await.unwrap();
        let caps = client.capabilities.expect("capabilities populated");
        assert!(caps.supports_configuration_done_request);
        assert!(caps.supports_conditional_breakpoints);
        assert!(caps.supports_terminate_request);
        assert!(!client.initialized, "initialized event not yet seen");
    }

    #[tokio::test]
    async fn initialized_event_flows_through_event_channel() {
        let (r, w) = spawn_mock_adapter(vec![
            Action::Respond(init_caps()),
            Action::EmitEvent("initialized", serde_json::json!({})),
        ]);
        let mut client = DapClient::from_streams(r, w, "mock").await.unwrap();
        let evt = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            client.event_rx.recv(),
        )
        .await
        .expect("timed out waiting for event")
        .expect("channel closed");
        match evt {
            DapEventKind::Event(e) => assert_eq!(e.event, "initialized"),
            other => panic!("expected initialized event, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn configuration_done_after_initialized() {
        let (r, w) = spawn_mock_adapter(vec![
            Action::Respond(init_caps()),
            Action::EmitEvent("initialized", serde_json::json!({})),
            Action::RespondOk, // configurationDone
        ]);
        let mut client = DapClient::from_streams(r, w, "mock").await.unwrap();
        // Drain the initialized event
        let _ = client.event_rx.recv().await;
        client.mark_initialized();
        let resp = client.configuration_done().await.unwrap();
        assert!(resp.success);
        assert_eq!(resp.command, "configurationDone");
    }

    #[tokio::test]
    async fn set_breakpoints_returns_parsed_body() {
        let body = serde_json::json!({
            "breakpoints": [
                {"id": 1, "verified": true, "line": 42}
            ]
        });
        let (r, w) = spawn_mock_adapter(vec![
            Action::Respond(init_caps()),
            Action::Respond(body),
        ]);
        let client = DapClient::from_streams(r, w, "mock").await.unwrap();
        let bps = client
            .set_breakpoints(
                "/tmp/example.rs",
                vec![SourceBreakpoint {
                    line: 42,
                    condition: None,
                    hit_condition: None,
                }],
            )
            .await
            .unwrap();
        assert_eq!(bps.breakpoints.len(), 1);
        assert_eq!(bps.breakpoints[0].line, Some(42));
        assert!(bps.breakpoints[0].verified);
    }

    #[tokio::test]
    async fn threads_returns_parsed_list() {
        let body = serde_json::json!({
            "threads": [
                {"id": 1, "name": "main"},
                {"id": 2, "name": "worker"}
            ]
        });
        let (r, w) = spawn_mock_adapter(vec![
            Action::Respond(init_caps()),
            Action::Respond(body),
        ]);
        let client = DapClient::from_streams(r, w, "mock").await.unwrap();
        let threads = client.threads().await.unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].name, "main");
        assert_eq!(threads[1].id, 2);
    }

    #[tokio::test]
    async fn stopped_event_delivered_to_event_channel() {
        let (r, w) = spawn_mock_adapter(vec![
            Action::Respond(init_caps()),
            Action::EmitEvent(
                "stopped",
                serde_json::json!({"reason": "breakpoint", "threadId": 1}),
            ),
        ]);
        let mut client = DapClient::from_streams(r, w, "mock").await.unwrap();
        let evt = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            client.event_rx.recv(),
        )
        .await
        .unwrap()
        .unwrap();
        match evt {
            DapEventKind::Event(e) => {
                assert_eq!(e.event, "stopped");
                let body: StoppedEventBody = serde_json::from_value(e.body.unwrap()).unwrap();
                assert_eq!(body.reason, "breakpoint");
                assert_eq!(body.thread_id, Some(1));
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn request_error_is_surfaced_as_err() {
        let (r, w) = spawn_mock_adapter(vec![
            Action::Respond(init_caps()),
            Action::RespondErr("breakpoint rejected"),
        ]);
        let client = DapClient::from_streams(r, w, "mock").await.unwrap();
        let err = client
            .set_breakpoints("/tmp/x.rs", vec![])
            .await
            .unwrap_err();
        assert!(
            err.contains("breakpoint rejected"),
            "unexpected error: {}",
            err
        );
    }

    #[tokio::test]
    async fn initialize_failure_returns_err() {
        let (r, w) = spawn_mock_adapter(vec![Action::RespondErr("no such adapter")]);
        let result = DapClient::from_streams(r, w, "mock").await;
        let err = match result {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        assert!(err.contains("initialize"), "unexpected error: {}", err);
    }

    #[tokio::test]
    async fn continue_next_stepin_stepout_all_round_trip() {
        let (r, w) = spawn_mock_adapter(vec![
            Action::Respond(init_caps()),
            Action::RespondOk,
            Action::RespondOk,
            Action::RespondOk,
            Action::RespondOk,
        ]);
        let client = DapClient::from_streams(r, w, "mock").await.unwrap();
        assert_eq!(client.continue_(1).await.unwrap().command, "continue");
        assert_eq!(client.next(1).await.unwrap().command, "next");
        assert_eq!(client.step_in(1).await.unwrap().command, "stepIn");
        assert_eq!(client.step_out(1).await.unwrap().command, "stepOut");
    }

    #[tokio::test]
    async fn disconnect_sends_terminate_debuggee_flag() {
        let (r, w) = spawn_mock_adapter(vec![
            Action::Respond(init_caps()),
            Action::RespondOk,
        ]);
        let client = DapClient::from_streams(r, w, "mock").await.unwrap();
        let resp = client.disconnect(true).await.unwrap();
        assert_eq!(resp.command, "disconnect");
        assert!(resp.success);
    }

    #[tokio::test]
    async fn adapter_exit_produces_adapter_exited_event() {
        // No script actions after initialize — mock task exits, stream closes.
        let (r, w) = spawn_mock_adapter(vec![Action::Respond(init_caps())]);
        let mut client = DapClient::from_streams(r, w, "mock").await.unwrap();
        // Wait for AdapterExited (comes after the mock task's sleep + drop).
        let evt = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            client.event_rx.recv(),
        )
        .await
        .expect("timed out")
        .expect("channel closed");
        assert!(matches!(evt, DapEventKind::AdapterExited));
    }

    #[tokio::test]
    async fn request_timeout_cleans_up_pending() {
        // Adapter responds to initialize only, then goes silent.
        let (r, w) = spawn_mock_adapter(vec![Action::Respond(init_caps())]);
        let client = DapClient::from_streams(r, w, "mock").await.unwrap();
        let err = client
            .request("threads", None, std::time::Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(err.contains("timed out"), "unexpected error: {}", err);
        // Confirm pending map was cleaned up.
        let pending = client.pending.lock().await;
        assert!(pending.is_empty(), "pending map should be cleaned on timeout");
    }
}
