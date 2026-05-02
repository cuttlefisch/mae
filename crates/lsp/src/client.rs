//! LSP client — manages a language server subprocess and message exchange.
//!
//! The client spawns a language server process, performs the initialize/initialized
//! handshake, and provides methods for document synchronization notifications.
//! Incoming messages (responses, server requests, notifications) are forwarded
//! to a channel for the editor event loop to consume.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::protocol::*;
use crate::transport::{LspTransport, TransportError};

/// Shared map from request id to the oneshot sender waiting for the response.
type PendingMap = Arc<Mutex<HashMap<RequestId, oneshot::Sender<Response>>>>;

/// Events the editor receives from the LSP client.
#[derive(Debug)]
pub enum LspEvent {
    /// Server responded to a request.
    Response(Response),
    /// Server sent a notification (diagnostics, log messages, etc.).
    ServerNotification(Notification),
    /// Server sent a request (window/showMessage, workspace/configuration, etc.).
    ServerRequest(Request),
    /// Transport error — server probably died.
    Error(String),
    /// Server process exited.
    ServerExited,
}

/// Configuration for spawning a language server.
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    /// The command to run (e.g., "rust-analyzer", "pylsp").
    pub command: String,
    /// Arguments to pass to the command.
    pub args: Vec<String>,
    /// Root URI for the workspace (e.g., "file:///home/user/project").
    pub root_uri: Option<String>,
}

/// An active LSP client connected to a language server.
pub struct LspClient {
    /// Sender for outgoing messages (to the writer task).
    outgoing_tx: mpsc::Sender<Vec<u8>>,
    /// Receiver for incoming LSP events (for the editor). Responses with a matching
    /// pending request are routed directly via oneshot and do NOT appear here.
    pub event_rx: mpsc::Receiver<LspEvent>,
    /// Next request ID.
    next_id: AtomicI64,
    /// Pending requests awaiting responses. Shared with the reader task so it can
    /// route responses directly to the awaiter instead of going through event_rx.
    pending: PendingMap,
    /// Server capabilities from the initialize response.
    pub server_capabilities: Option<ServerCapabilities>,
    /// The text document sync mode the server requested.
    pub sync_kind: TextDocumentSyncKind,
    /// Document versions we're tracking.
    doc_versions: HashMap<String, i64>,
    /// Child process handle — kept alive until shutdown.
    _child: Child,
}

impl LspClient {
    /// Spawn a language server and perform the initialize handshake.
    ///
    /// Returns `(client, event_receiver)` on success. The caller should poll
    /// `event_rx` in their event loop.
    pub async fn start(config: LspServerConfig) -> Result<Self, String> {
        let mut child = Command::new(&config.command)
            .args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to spawn '{}': {}", config.command, e))?;

        let stdin = child.stdin.take().ok_or("no stdin on child process")?;
        let stdout = child.stdout.take().ok_or("no stdout on child process")?;

        let (event_tx, event_rx) = mpsc::channel(256);
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<Vec<u8>>(64);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        // Spawn reader task: reads messages from server stdout. Responses matching
        // a pending request are routed via oneshot; everything else goes to event_tx.
        let reader_event_tx = event_tx.clone();
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let mut transport = LspTransport::new(stdout, tokio::io::sink());
            loop {
                match transport.read_message().await {
                    Ok(Message::Response(r)) => {
                        // Check for a pending awaiter first.
                        let tx = reader_pending.lock().await.remove(&r.id);
                        if let Some(tx) = tx {
                            // Ignore send error — awaiter may have dropped.
                            let _ = tx.send(r);
                        } else if reader_event_tx.send(LspEvent::Response(r)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Notification(n)) => {
                        if reader_event_tx
                            .send(LspEvent::ServerNotification(n))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Message::Request(r)) => {
                        if reader_event_tx
                            .send(LspEvent::ServerRequest(r))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(TransportError::ConnectionClosed) => {
                        let _ = reader_event_tx.send(LspEvent::ServerExited).await;
                        break;
                    }
                    Err(e) => {
                        let _ = reader_event_tx
                            .send(LspEvent::Error(format!("{}", e)))
                            .await;
                        break;
                    }
                }
            }
        });

        // Spawn writer task: reads from outgoing_rx → writes to server stdin
        tokio::spawn(async move {
            let mut transport = LspTransport::new(tokio::io::empty(), stdin);
            let mut rx = outgoing_rx;
            while let Some(body) = rx.recv().await {
                if transport.write_raw(&body).await.is_err() {
                    break;
                }
            }
        });

        let mut client = LspClient {
            outgoing_tx,
            event_rx,
            next_id: AtomicI64::new(1),
            pending,
            server_capabilities: None,
            sync_kind: TextDocumentSyncKind::Full, // default until server tells us
            doc_versions: HashMap::new(),
            _child: child,
        };

        // Perform initialize handshake
        client.initialize(&config).await?;

        Ok(client)
    }

    /// Send the initialize request and wait for the response.
    async fn initialize(&mut self, config: &LspServerConfig) -> Result<(), String> {
        let params = InitializeParams {
            process_id: Some(std::process::id() as i64),
            root_uri: config.root_uri.clone(),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    synchronization: Some(TextDocumentSyncClientCapabilities {
                        did_save: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            client_info: Some(ClientInfo {
                name: "MAE".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        };

        let response = self
            .request(
                "initialize",
                Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(30),
            )
            .await
            .map_err(|e| format!("initialize failed: {}", e))?;

        if let Some(err) = response.error {
            return Err(format!(
                "server returned error: {} ({})",
                err.message, err.code
            ));
        }

        // Parse server capabilities
        if let Some(result) = response.result {
            if let Ok(init_result) = serde_json::from_value::<InitializeResult>(result) {
                if let Some(ref sync) = init_result.capabilities.text_document_sync {
                    self.sync_kind = TextDocumentSyncKind::from_value(sync);
                }
                self.server_capabilities = Some(init_result.capabilities);
            }
        }

        // Send "initialized" notification
        let notif = Notification::new("initialized", Some(serde_json::json!({})));
        self.send_notification_raw(&notif).await?;

        Ok(())
    }

    /// Send a textDocument/didOpen notification.
    pub async fn did_open(
        &mut self,
        uri: &str,
        language_id: &str,
        text: &str,
    ) -> Result<(), String> {
        let version = 0;
        self.doc_versions.insert(uri.to_string(), version);

        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.to_string(),
                language_id: language_id.to_string(),
                version,
                text: text.to_string(),
            },
        };

        let notif = Notification::new(
            "textDocument/didOpen",
            Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
        );
        self.send_notification_raw(&notif).await
    }

    /// Send a textDocument/didChange notification (full sync).
    pub async fn did_change(&mut self, uri: &str, text: &str) -> Result<(), String> {
        let version = self.doc_versions.get(uri).copied().unwrap_or(0) + 1;
        self.doc_versions.insert(uri.to_string(), version);

        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.to_string(),
                version,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                text: text.to_string(),
            }],
        };

        let notif = Notification::new(
            "textDocument/didChange",
            Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
        );
        self.send_notification_raw(&notif).await
    }

    /// Send a textDocument/didSave notification.
    pub async fn did_save(&mut self, uri: &str, text: Option<&str>) -> Result<(), String> {
        let params = DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            text: text.map(String::from),
        };

        let notif = Notification::new(
            "textDocument/didSave",
            Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
        );
        self.send_notification_raw(&notif).await
    }

    /// Send a textDocument/didClose notification.
    pub async fn did_close(&mut self, uri: &str) -> Result<(), String> {
        self.doc_versions.remove(uri);

        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
        };

        let notif = Notification::new(
            "textDocument/didClose",
            Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
        );
        self.send_notification_raw(&notif).await
    }

    /// Send a workspace/didChangeWorkspaceFolders notification.
    pub async fn did_change_workspace_folders(&self, added_uris: &[String]) -> Result<(), String> {
        let params = serde_json::json!({
            "event": {
                "added": added_uris.iter().map(|uri| serde_json::json!({
                    "uri": uri,
                    "name": "project"
                })).collect::<Vec<serde_json::Value>>(),
                "removed": []
            }
        });
        let notif = Notification::new("workspace/didChangeWorkspaceFolders", Some(params));
        self.send_notification_raw(&notif).await
    }

    /// Send a request and wait for the matching response via oneshot.
    ///
    /// Registers a pending entry in the correlation map before writing the
    /// request — the reader task will route the response directly back here
    /// instead of through `event_rx`. On timeout the pending entry is cleaned up.
    pub async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
        timeout: std::time::Duration,
    ) -> Result<Response, String> {
        let id = self.next_request_id();
        let request_id = RequestId::Integer(id);

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        let req = Request::new(id, method, params);
        if let Err(e) = self.send_request_raw(&req).await {
            // Clean up pending if send failed.
            self.pending.lock().await.remove(&request_id);
            return Err(e);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&request_id);
                Err("response channel closed".into())
            }
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err(format!("request '{}' timed out", method))
            }
        }
    }

    /// textDocument/definition — returns locations for the symbol at `position`.
    pub async fn request_definition(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<DefinitionResponse, String> {
        let params = TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position,
        };
        let resp = self
            .request(
                "textDocument/definition",
                Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(10),
            )
            .await?;

        if let Some(err) = resp.error {
            return Err(format!("server error: {} ({})", err.message, err.code));
        }
        let result = resp.result.unwrap_or(serde_json::Value::Null);
        Ok(DefinitionResponse::from_value(result))
    }

    /// textDocument/references — returns all references to the symbol at `position`.
    pub async fn request_references(
        &self,
        uri: &str,
        position: Position,
        include_declaration: bool,
    ) -> Result<ReferencesResponse, String> {
        let params = ReferenceParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position,
            context: ReferenceContext {
                include_declaration,
            },
        };
        let resp = self
            .request(
                "textDocument/references",
                Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(10),
            )
            .await?;

        if let Some(err) = resp.error {
            return Err(format!("server error: {} ({})", err.message, err.code));
        }
        let result = resp.result.unwrap_or(serde_json::Value::Null);
        Ok(ReferencesResponse::from_value(result))
    }

    /// textDocument/completion — returns completion items at `position`.
    pub async fn request_completion(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<CompletionResponse, String> {
        let params = CompletionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position,
        };
        let resp = self
            .request(
                "textDocument/completion",
                Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(5),
            )
            .await?;

        if let Some(err) = resp.error {
            return Err(format!("server error: {} ({})", err.message, err.code));
        }
        let result = resp.result.unwrap_or(serde_json::Value::Null);
        Ok(CompletionResponse::from_value(result))
    }

    /// textDocument/hover — returns type/documentation at `position`.
    pub async fn request_hover(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<HoverResponse, String> {
        let params = TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position,
        };
        let resp = self
            .request(
                "textDocument/hover",
                Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(5),
            )
            .await?;

        if let Some(err) = resp.error {
            return Err(format!("server error: {} ({})", err.message, err.code));
        }
        let result = resp.result.unwrap_or(serde_json::Value::Null);
        Ok(HoverResponse::from_value(result))
    }

    /// workspace/symbol — search for symbols across the workspace.
    pub async fn request_workspace_symbol(
        &self,
        query: &str,
    ) -> Result<WorkspaceSymbolResponse, String> {
        let params = WorkspaceSymbolParams {
            query: query.to_string(),
        };
        let resp = self
            .request(
                "workspace/symbol",
                Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(15),
            )
            .await?;

        if let Some(err) = resp.error {
            return Err(format!("server error: {} ({})", err.message, err.code));
        }
        let result = resp.result.unwrap_or(serde_json::Value::Null);
        Ok(WorkspaceSymbolResponse::from_value(result))
    }

    /// textDocument/documentSymbol — list symbols in a document.
    pub async fn request_document_symbols(
        &self,
        uri: &str,
    ) -> Result<DocumentSymbolResponse, String> {
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
        };
        let resp = self
            .request(
                "textDocument/documentSymbol",
                Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(10),
            )
            .await?;

        if let Some(err) = resp.error {
            return Err(format!("server error: {} ({})", err.message, err.code));
        }
        let result = resp.result.unwrap_or(serde_json::Value::Null);
        Ok(DocumentSymbolResponse::from_value(result))
    }

    /// textDocument/documentHighlight — returns symbol occurrences at `position`.
    pub async fn request_document_highlight(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<DocumentHighlightResponse, String> {
        let params = TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position,
        };
        let resp = self
            .request(
                "textDocument/documentHighlight",
                Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(5),
            )
            .await?;

        if let Some(err) = resp.error {
            return Err(format!("server error: {} ({})", err.message, err.code));
        }
        let result = resp.result.unwrap_or(serde_json::Value::Null);
        Ok(DocumentHighlightResponse::from_value(result))
    }

    /// textDocument/codeAction — returns code actions available at `range`.
    pub async fn request_code_action(
        &self,
        uri: &str,
        range: Range,
        diagnostics: Vec<serde_json::Value>,
    ) -> Result<CodeActionResponse, String> {
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            range,
            context: CodeActionContext { diagnostics },
        };
        let resp = self
            .request(
                "textDocument/codeAction",
                Some(serde_json::to_value(&params).map_err(|e| e.to_string())?),
                std::time::Duration::from_secs(5),
            )
            .await?;

        if let Some(err) = resp.error {
            return Err(format!("server error: {} ({})", err.message, err.code));
        }
        let result = resp.result.unwrap_or(serde_json::Value::Null);
        Ok(CodeActionResponse::from_value(result))
    }

    /// Send shutdown request and exit notification for graceful teardown.
    pub async fn shutdown(&mut self) -> Result<(), String> {
        let _ = self
            .request("shutdown", None, std::time::Duration::from_secs(5))
            .await;
        let notif = Notification::new("exit", None);
        let _ = self.send_notification_raw(&notif).await;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn next_request_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    async fn send_request_raw(&self, req: &Request) -> Result<(), String> {
        let body = serde_json::to_vec(req).map_err(|e| e.to_string())?;
        self.outgoing_tx
            .send(body)
            .await
            .map_err(|_| "outgoing channel closed".to_string())
    }

    async fn send_notification_raw(&self, notif: &Notification) -> Result<(), String> {
        let body = serde_json::to_vec(notif).map_err(|e| e.to_string())?;
        self.outgoing_tx
            .send(body)
            .await
            .map_err(|_| "outgoing channel closed".to_string())
    }
}

/// Convert a filesystem path to a `file://` URI.
pub fn path_to_uri(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    format!("file://{}", abs.display())
}

/// Guess the LSP language ID from a file extension.
pub fn language_id_from_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("js") => "javascript",
        Some("ts") => "typescript",
        Some("tsx") => "typescriptreact",
        Some("jsx") => "javascriptreact",
        Some("go") => "go",
        Some("c") | Some("h") => "c",
        Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => "cpp",
        Some("java") => "java",
        Some("rb") => "ruby",
        Some("scm") | Some("ss") => "scheme",
        Some("toml") => "toml",
        Some("json") => "json",
        Some("yaml") | Some("yml") => "yaml",
        Some("md") => "markdown",
        Some("html") | Some("htm") => "html",
        Some("css") => "css",
        Some("sh") | Some("bash") | Some("zsh") => "shellscript",
        Some("lua") => "lua",
        Some("zig") => "zig",
        _ => "plaintext",
    }
}

// ---------------------------------------------------------------------------
// Test-only harness for constructing an LspClient without a real subprocess.
// ---------------------------------------------------------------------------

/// Simulated server-side handle used by tests. Exposes the client's outgoing
/// channel (to inspect what the client sent) and lets tests inject responses
/// as if a real language server replied.
#[cfg(test)]
pub(crate) struct TestHarness {
    /// Outgoing body bytes that the client sent to the "server".
    pub outgoing_rx: mpsc::Receiver<Vec<u8>>,
    /// Sender tests use to inject LspEvents (Response/Notification/Request).
    pub incoming_tx: mpsc::Sender<LspEvent>,
    /// Shared pending map — tests may inspect but normally don't need to touch it.
    #[allow(dead_code)]
    pub pending: PendingMap,
}

#[cfg(test)]
impl LspClient {
    /// Construct an LspClient wired to in-memory channels for testing.
    /// Returns `(client, harness)`.
    pub(crate) fn new_for_test() -> (Self, TestHarness) {
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<Vec<u8>>(64);
        let (event_tx, event_rx) = mpsc::channel(256);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        // Spawn a "router" task that mimics the real reader: it moves events from
        // the test's incoming_tx to either a pending awaiter or event_rx.
        let (incoming_tx, mut incoming_rx) = mpsc::channel::<LspEvent>(64);
        let router_pending = pending.clone();
        let router_event_tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = incoming_rx.recv().await {
                match event {
                    LspEvent::Response(r) => {
                        let tx = router_pending.lock().await.remove(&r.id);
                        if let Some(tx) = tx {
                            let _ = tx.send(r);
                        } else if router_event_tx.send(LspEvent::Response(r)).await.is_err() {
                            break;
                        }
                    }
                    other => {
                        if router_event_tx.send(other).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // We need a Child handle — spawn a trivial process and keep it alive.
        // `true` is POSIX; on non-unix we'd need something else.
        let child = Command::new("sleep")
            .arg("3600")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn test child process");

        let client = LspClient {
            outgoing_tx,
            event_rx,
            next_id: AtomicI64::new(1),
            pending: pending.clone(),
            server_capabilities: None,
            sync_kind: TextDocumentSyncKind::Full,
            doc_versions: HashMap::new(),
            _child: child,
        };

        let harness = TestHarness {
            outgoing_rx,
            incoming_tx,
            pending,
        };

        (client, harness)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_to_uri_absolute() {
        let uri = path_to_uri(Path::new("/home/user/project/main.rs"));
        assert_eq!(uri, "file:///home/user/project/main.rs");
    }

    #[test]
    fn language_id_rust() {
        assert_eq!(language_id_from_path(Path::new("foo.rs")), "rust");
    }

    #[test]
    fn language_id_python() {
        assert_eq!(language_id_from_path(Path::new("script.py")), "python");
    }

    #[test]
    fn language_id_unknown_extension() {
        assert_eq!(language_id_from_path(Path::new("data.xyz")), "plaintext");
    }

    #[test]
    fn language_id_no_extension() {
        assert_eq!(language_id_from_path(Path::new("Makefile")), "plaintext");
    }

    #[test]
    fn language_id_scheme() {
        assert_eq!(language_id_from_path(Path::new("init.scm")), "scheme");
    }

    #[test]
    fn language_id_toml() {
        assert_eq!(language_id_from_path(Path::new("Cargo.toml")), "toml");
    }

    #[test]
    fn language_id_typescript() {
        assert_eq!(language_id_from_path(Path::new("app.ts")), "typescript");
        assert_eq!(
            language_id_from_path(Path::new("App.tsx")),
            "typescriptreact"
        );
    }

    /// Parse the outgoing bytes as a JSON-RPC Request.
    fn parse_outgoing(bytes: &[u8]) -> Request {
        serde_json::from_slice::<Request>(bytes).expect("outgoing should be a valid Request")
    }

    #[tokio::test]
    async fn request_definition_sends_correct_params() {
        let (client, mut harness) = LspClient::new_for_test();
        let position = Position {
            line: 10,
            character: 5,
        };

        // Fire request in a background task since it awaits a response.
        let handle =
            tokio::spawn(
                async move { client.request_definition("file:///test.rs", position).await },
            );

        // Grab the outgoing request bytes
        let bytes = harness.outgoing_rx.recv().await.expect("outgoing");
        let req = parse_outgoing(&bytes);
        assert_eq!(req.method, "textDocument/definition");
        let params = req.params.unwrap();
        assert_eq!(params["textDocument"]["uri"], "file:///test.rs");
        assert_eq!(params["position"]["line"], 10);
        assert_eq!(params["position"]["character"], 5);

        // Send back a response
        let response = Response::ok(
            req.id.clone(),
            serde_json::json!({
                "uri": "file:///def.rs",
                "range": {
                    "start": {"line": 3, "character": 0},
                    "end":   {"line": 3, "character": 10}
                }
            }),
        );
        harness
            .incoming_tx
            .send(LspEvent::Response(response))
            .await
            .unwrap();

        let def_resp = handle.await.unwrap().unwrap();
        assert_eq!(def_resp.locations.len(), 1);
        assert_eq!(def_resp.locations[0].uri, "file:///def.rs");
    }

    #[tokio::test]
    async fn request_references_sends_correct_params() {
        let (client, mut harness) = LspClient::new_for_test();
        let position = Position {
            line: 7,
            character: 2,
        };

        let handle = tokio::spawn(async move {
            client
                .request_references("file:///test.rs", position, true)
                .await
        });

        let bytes = harness.outgoing_rx.recv().await.expect("outgoing");
        let req = parse_outgoing(&bytes);
        assert_eq!(req.method, "textDocument/references");
        let params = req.params.unwrap();
        assert_eq!(params["context"]["includeDeclaration"], true);

        let response = Response::ok(
            req.id.clone(),
            serde_json::json!([
                {
                    "uri": "file:///a.rs",
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}
                },
                {
                    "uri": "file:///b.rs",
                    "range": {"start": {"line": 5, "character": 0}, "end": {"line": 5, "character": 3}}
                }
            ]),
        );
        harness
            .incoming_tx
            .send(LspEvent::Response(response))
            .await
            .unwrap();

        let refs_resp = handle.await.unwrap().unwrap();
        assert_eq!(refs_resp.locations.len(), 2);
    }

    #[tokio::test]
    async fn request_hover_returns_parsed_contents() {
        let (client, mut harness) = LspClient::new_for_test();
        let position = Position {
            line: 0,
            character: 0,
        };

        let handle =
            tokio::spawn(async move { client.request_hover("file:///test.rs", position).await });

        let bytes = harness.outgoing_rx.recv().await.expect("outgoing");
        let req = parse_outgoing(&bytes);
        assert_eq!(req.method, "textDocument/hover");

        let response = Response::ok(
            req.id.clone(),
            serde_json::json!({
                "contents": {"kind": "markdown", "value": "fn foo() -> i32"}
            }),
        );
        harness
            .incoming_tx
            .send(LspEvent::Response(response))
            .await
            .unwrap();

        let hover = handle.await.unwrap().unwrap();
        assert_eq!(hover.contents, "fn foo() -> i32");
    }

    #[tokio::test]
    async fn request_times_out_when_no_response() {
        let (client, mut harness) = LspClient::new_for_test();

        let handle = tokio::spawn(async move {
            client
                .request("someMethod", None, std::time::Duration::from_millis(50))
                .await
        });

        // Drain outgoing but don't reply
        let _ = harness.outgoing_rx.recv().await;

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timed out"));
    }

    #[tokio::test]
    async fn request_error_response_propagated() {
        let (client, mut harness) = LspClient::new_for_test();
        let position = Position {
            line: 0,
            character: 0,
        };

        let handle =
            tokio::spawn(
                async move { client.request_definition("file:///test.rs", position).await },
            );

        let bytes = harness.outgoing_rx.recv().await.expect("outgoing");
        let req = parse_outgoing(&bytes);

        let response = Response::error(req.id.clone(), -32601, "Method not found");
        harness
            .incoming_tx
            .send(LspEvent::Response(response))
            .await
            .unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Method not found"));
    }

    #[tokio::test]
    async fn concurrent_requests_routed_correctly() {
        let (client, mut harness) = LspClient::new_for_test();

        // Fire two requests concurrently
        let client_a = std::sync::Arc::new(client);
        let ca = client_a.clone();
        let cb = client_a.clone();

        let h1 = tokio::spawn(async move {
            ca.request_hover(
                "file:///a.rs",
                Position {
                    line: 1,
                    character: 0,
                },
            )
            .await
        });
        let h2 = tokio::spawn(async move {
            cb.request_hover(
                "file:///b.rs",
                Position {
                    line: 2,
                    character: 0,
                },
            )
            .await
        });

        // Capture both outgoing requests
        let b1 = harness.outgoing_rx.recv().await.unwrap();
        let b2 = harness.outgoing_rx.recv().await.unwrap();
        let r1 = parse_outgoing(&b1);
        let r2 = parse_outgoing(&b2);

        // Reply to r2 FIRST (out of order) to verify correlation works
        harness
            .incoming_tx
            .send(LspEvent::Response(Response::ok(
                r2.id.clone(),
                serde_json::json!({"contents": "second"}),
            )))
            .await
            .unwrap();
        harness
            .incoming_tx
            .send(LspEvent::Response(Response::ok(
                r1.id.clone(),
                serde_json::json!({"contents": "first"}),
            )))
            .await
            .unwrap();

        let resp1 = h1.await.unwrap().unwrap();
        let resp2 = h2.await.unwrap().unwrap();

        assert_eq!(resp1.contents, "first");
        assert_eq!(resp2.contents, "second");
    }

    #[test]
    fn doc_version_tracking() {
        // Verify the version tracking logic in isolation
        let mut versions: HashMap<String, i64> = HashMap::new();
        let uri = "file:///test.rs";

        // First open = version 0
        versions.insert(uri.to_string(), 0);
        assert_eq!(versions[uri], 0);

        // Each change increments
        let v = versions.get(uri).copied().unwrap_or(0) + 1;
        versions.insert(uri.to_string(), v);
        assert_eq!(versions[uri], 1);

        let v = versions.get(uri).copied().unwrap_or(0) + 1;
        versions.insert(uri.to_string(), v);
        assert_eq!(versions[uri], 2);
    }
}
