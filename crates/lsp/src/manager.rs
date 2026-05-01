//! LspManager — multi-language LSP client coordinator.
//!
//! The editor holds a command channel to a single `LspManager` task. The
//! manager spawns language server clients on demand (keyed by language id),
//! tracks which documents are open, and forwards server notifications
//! (diagnostics, etc.) back to the editor via a single merged event channel.
//!
//! This mirrors the AI architecture (AiCommand/AiEvent) to keep the editor's
//! event loop uniform.

use std::collections::HashMap;

use tokio::sync::mpsc;

use crate::client::{LspClient, LspEvent, LspServerConfig};
use crate::protocol::{
    CompletionItem, CompletionResponse, Diagnostic, Location, Notification, Position,
    PublishDiagnosticsParams, Range,
};

/// Commands the editor sends to the LSP task.
#[derive(Debug)]
pub enum LspCommand {
    /// A buffer was opened — send didOpen to the matching server.
    DidOpen {
        uri: String,
        language_id: String,
        text: String,
    },
    /// A buffer was edited — send didChange.
    DidChange {
        uri: String,
        language_id: String,
        text: String,
    },
    /// A buffer was saved — send didSave.
    DidSave {
        uri: String,
        language_id: String,
        text: Option<String>,
    },
    /// A buffer was closed — send didClose.
    DidClose { uri: String, language_id: String },
    /// Request definition at the given position.
    GotoDefinition {
        uri: String,
        language_id: String,
        position: Position,
    },
    /// Request references at the given position.
    FindReferences {
        uri: String,
        language_id: String,
        position: Position,
        include_declaration: bool,
    },
    /// Request hover info at the given position.
    Hover {
        uri: String,
        language_id: String,
        position: Position,
    },
    /// Request completion items at the given position.
    Completion {
        uri: String,
        language_id: String,
        position: Position,
    },
    /// Search for symbols across the workspace.
    WorkspaceSymbol { language_id: String, query: String },
    /// List symbols in a document.
    DocumentSymbols { uri: String, language_id: String },
    /// Request code actions at a range.
    CodeAction {
        uri: String,
        language_id: String,
        range: Range,
        diagnostics: Vec<serde_json::Value>,
    },
    /// Request document highlights at a position.
    DocumentHighlight {
        uri: String,
        language_id: String,
        position: Position,
        generation: u64,
    },
    /// Shut down all clients.
    Shutdown,
}

/// Events the LSP task forwards to the editor.
#[derive(Debug)]
pub enum LspTaskEvent {
    /// A language server was started successfully.
    ServerStarted { language_id: String },
    /// A language server failed to start.
    ServerStartFailed { language_id: String, error: String },
    /// A language server exited.
    ServerExited { language_id: String },
    /// Definition response — empty locations means "not found".
    DefinitionResult {
        uri: String,
        locations: Vec<Location>,
    },
    /// References response.
    ReferencesResult {
        uri: String,
        locations: Vec<Location>,
    },
    /// Hover response.
    HoverResult {
        uri: String,
        contents: String,
        range: Option<Range>,
    },
    /// Diagnostics published for a URI — replaces any prior diagnostics for
    /// that URI (LSP contract).
    DiagnosticsPublished {
        uri: String,
        diagnostics: Vec<Diagnostic>,
    },
    /// A server sent us a notification (non-diagnostic: log, progress, etc.).
    ServerNotification {
        language_id: String,
        notification: Notification,
    },
    /// Completion response.
    CompletionResult {
        uri: String,
        items: Vec<CompletionItem>,
        is_incomplete: bool,
    },
    /// Workspace symbol response.
    WorkspaceSymbolResult {
        symbols: Vec<crate::protocol::SymbolInformation>,
    },
    /// Document symbol response.
    DocumentSymbolResult {
        uri: String,
        symbols: Vec<crate::protocol::DocumentSymbol>,
    },
    /// Code action response.
    CodeActionResult {
        uri: String,
        actions: Vec<crate::protocol::CodeAction>,
    },
    /// Document highlight response.
    DocumentHighlightResult {
        highlights: Vec<crate::protocol::DocumentHighlight>,
        generation: u64,
    },
    /// An error happened during a request.
    Error { message: String },
}

/// Manages multiple LSP clients, one per language id.
pub struct LspManager {
    configs: HashMap<String, LspServerConfig>,
    clients: HashMap<String, LspClient>,
    /// Merged stream of events from all clients.
    merged_event_tx: mpsc::Sender<(String, LspEvent)>,
}

impl LspManager {
    pub fn new(
        configs: HashMap<String, LspServerConfig>,
        merged_event_tx: mpsc::Sender<(String, LspEvent)>,
    ) -> Self {
        LspManager {
            configs,
            clients: HashMap::new(),
            merged_event_tx,
        }
    }

    /// Ensure a client is started for `language_id`. Spawns a forwarder task
    /// that drains the client's event channel into the merged channel.
    async fn ensure_client(&mut self, language_id: &str) -> Result<&mut LspClient, String> {
        if !self.clients.contains_key(language_id) {
            let config = self
                .configs
                .get(language_id)
                .ok_or_else(|| format!("no LSP server configured for language '{}'", language_id))?
                .clone();

            let mut client = LspClient::start(config).await?;

            // Drain the client's event_rx into the merged channel, tagging with language_id.
            // We swap the receiver out so we own it in the forwarder.
            let mut rx = std::mem::replace(&mut client.event_rx, mpsc::channel(1).1);
            let lang = language_id.to_string();
            let merged = self.merged_event_tx.clone();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    if merged.send((lang.clone(), event)).await.is_err() {
                        break;
                    }
                }
            });

            self.clients.insert(language_id.to_string(), client);
        }
        Ok(self.clients.get_mut(language_id).unwrap())
    }

    pub async fn did_open(
        &mut self,
        language_id: &str,
        uri: &str,
        text: &str,
    ) -> Result<(), String> {
        let client = self.ensure_client(language_id).await?;
        client.did_open(uri, language_id, text).await
    }

    pub async fn did_change(
        &mut self,
        language_id: &str,
        uri: &str,
        text: &str,
    ) -> Result<(), String> {
        let client = match self.clients.get_mut(language_id) {
            Some(c) => c,
            None => return Ok(()), // no server → nothing to sync
        };
        client.did_change(uri, text).await
    }

    pub async fn did_save(
        &mut self,
        language_id: &str,
        uri: &str,
        text: Option<&str>,
    ) -> Result<(), String> {
        let client = match self.clients.get_mut(language_id) {
            Some(c) => c,
            None => return Ok(()),
        };
        client.did_save(uri, text).await
    }

    pub async fn did_close(&mut self, language_id: &str, uri: &str) -> Result<(), String> {
        let client = match self.clients.get_mut(language_id) {
            Some(c) => c,
            None => return Ok(()),
        };
        client.did_close(uri).await
    }

    pub async fn goto_definition(
        &mut self,
        language_id: &str,
        uri: &str,
        position: Position,
    ) -> Result<Vec<Location>, String> {
        let client = self.ensure_client(language_id).await?;
        let resp = client.request_definition(uri, position).await?;
        Ok(resp.locations)
    }

    pub async fn find_references(
        &mut self,
        language_id: &str,
        uri: &str,
        position: Position,
        include_declaration: bool,
    ) -> Result<Vec<Location>, String> {
        let client = self.ensure_client(language_id).await?;
        let resp = client
            .request_references(uri, position, include_declaration)
            .await?;
        Ok(resp.locations)
    }

    pub async fn hover(
        &mut self,
        language_id: &str,
        uri: &str,
        position: Position,
    ) -> Result<(String, Option<Range>), String> {
        let client = self.ensure_client(language_id).await?;
        let resp = client.request_hover(uri, position).await?;
        Ok((resp.contents, resp.range))
    }

    pub async fn completion(
        &mut self,
        language_id: &str,
        uri: &str,
        position: Position,
    ) -> Result<CompletionResponse, String> {
        let client = self.ensure_client(language_id).await?;
        client.request_completion(uri, position).await
    }

    pub async fn workspace_symbol(
        &mut self,
        language_id: &str,
        query: &str,
    ) -> Result<Vec<crate::protocol::SymbolInformation>, String> {
        let client = self.ensure_client(language_id).await?;
        let resp = client.request_workspace_symbol(query).await?;
        Ok(resp.symbols)
    }

    pub async fn document_symbols(
        &mut self,
        language_id: &str,
        uri: &str,
    ) -> Result<Vec<crate::protocol::DocumentSymbol>, String> {
        let client = self.ensure_client(language_id).await?;
        let resp = client.request_document_symbols(uri).await?;
        Ok(resp.symbols)
    }

    pub async fn code_action(
        &mut self,
        language_id: &str,
        uri: &str,
        range: Range,
        diagnostics: Vec<serde_json::Value>,
    ) -> Result<Vec<crate::protocol::CodeAction>, String> {
        let client = self.ensure_client(language_id).await?;
        let resp = client.request_code_action(uri, range, diagnostics).await?;
        Ok(resp.actions)
    }

    pub async fn document_highlight(
        &mut self,
        language_id: &str,
        uri: &str,
        position: Position,
    ) -> Result<Vec<crate::protocol::DocumentHighlight>, String> {
        let client = match self.clients.get(language_id) {
            Some(_) => self.clients.get(language_id).unwrap(),
            None => return Ok(vec![]),
        };
        let resp = client.request_document_highlight(uri, position).await?;
        Ok(resp.highlights)
    }

    pub async fn shutdown_all(&mut self) {
        for (_, mut client) in self.clients.drain() {
            let _ = client.shutdown().await;
        }
    }

    /// Return the set of configured language ids.
    pub fn configured_languages(&self) -> Vec<String> {
        self.configs.keys().cloned().collect()
    }

    /// Whether a client for `language_id` is currently running.
    pub fn is_running(&self, language_id: &str) -> bool {
        self.clients.contains_key(language_id)
    }
}

/// Long-running task that owns an `LspManager` and processes commands.
///
/// Returns when `cmd_rx` is closed or `LspCommand::Shutdown` is received.
pub async fn run_lsp_task(
    configs: HashMap<String, LspServerConfig>,
    mut cmd_rx: mpsc::Receiver<LspCommand>,
    event_tx: mpsc::Sender<LspTaskEvent>,
) {
    let (merged_tx, mut merged_rx) = mpsc::channel::<(String, LspEvent)>(256);
    let mut manager = LspManager::new(configs, merged_tx);

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(LspCommand::Shutdown) => break,
                    Some(cmd) => handle_command(&mut manager, cmd, &event_tx).await,
                    None => break, // editor dropped the sender
                }
            }
            maybe_event = merged_rx.recv() => {
                let Some((language_id, event)) = maybe_event else { continue };
                let out = match event {
                    LspEvent::ServerNotification(notif) => {
                        // Intercept publishDiagnostics so the editor can
                        // update its diagnostic store directly.
                        if notif.method == "textDocument/publishDiagnostics" {
                            if let Some(params) = notif
                                .params
                                .as_ref()
                                .and_then(PublishDiagnosticsParams::from_value)
                            {
                                LspTaskEvent::DiagnosticsPublished {
                                    uri: params.uri,
                                    diagnostics: params.diagnostics,
                                }
                            } else {
                                // Malformed — drop silently.
                                continue;
                            }
                        } else {
                            LspTaskEvent::ServerNotification {
                                language_id,
                                notification: notif,
                            }
                        }
                    }
                    LspEvent::ServerExited => {
                        // Drop the dead client so the next request for this
                        // language triggers a fresh `ensure_client` → restart.
                        manager.clients.remove(&language_id);
                        LspTaskEvent::ServerExited { language_id }
                    }
                    LspEvent::Error(e) => LspTaskEvent::Error {
                        message: format!("{}: {}", language_id, e),
                    },
                    LspEvent::Response(_) => continue,
                    LspEvent::ServerRequest(_) => continue,
                };
                let _ = event_tx.send(out).await;
            }
        }
    }

    manager.shutdown_all().await;
}

async fn handle_command(
    manager: &mut LspManager,
    cmd: LspCommand,
    event_tx: &mpsc::Sender<LspTaskEvent>,
) {
    match cmd {
        LspCommand::DidOpen {
            uri,
            language_id,
            text,
        } => {
            let not_configured = !manager.configs.contains_key(&language_id);
            if not_configured {
                return; // silent — no server for this language
            }
            let started = !manager.is_running(&language_id);
            match manager.did_open(&language_id, &uri, &text).await {
                Ok(()) => {
                    if started {
                        let _ = event_tx
                            .send(LspTaskEvent::ServerStarted { language_id })
                            .await;
                    }
                }
                Err(e) => {
                    let _ = event_tx
                        .send(LspTaskEvent::ServerStartFailed {
                            language_id,
                            error: e,
                        })
                        .await;
                }
            }
        }
        LspCommand::DidChange {
            uri,
            language_id,
            text,
        } => {
            let _ = manager.did_change(&language_id, &uri, &text).await;
        }
        LspCommand::DidSave {
            uri,
            language_id,
            text,
        } => {
            let _ = manager.did_save(&language_id, &uri, text.as_deref()).await;
        }
        LspCommand::DidClose { uri, language_id } => {
            let _ = manager.did_close(&language_id, &uri).await;
        }
        LspCommand::GotoDefinition {
            uri,
            language_id,
            position,
        } => match manager.goto_definition(&language_id, &uri, position).await {
            Ok(locations) => {
                let _ = event_tx
                    .send(LspTaskEvent::DefinitionResult { uri, locations })
                    .await;
            }
            Err(e) => {
                let _ = event_tx.send(LspTaskEvent::Error { message: e }).await;
            }
        },
        LspCommand::FindReferences {
            uri,
            language_id,
            position,
            include_declaration,
        } => match manager
            .find_references(&language_id, &uri, position, include_declaration)
            .await
        {
            Ok(locations) => {
                let _ = event_tx
                    .send(LspTaskEvent::ReferencesResult { uri, locations })
                    .await;
            }
            Err(e) => {
                let _ = event_tx.send(LspTaskEvent::Error { message: e }).await;
            }
        },
        LspCommand::Hover {
            uri,
            language_id,
            position,
        } => match manager.hover(&language_id, &uri, position).await {
            Ok((contents, range)) => {
                let _ = event_tx
                    .send(LspTaskEvent::HoverResult {
                        uri,
                        contents,
                        range,
                    })
                    .await;
            }
            Err(e) => {
                let _ = event_tx.send(LspTaskEvent::Error { message: e }).await;
            }
        },
        LspCommand::Completion {
            uri,
            language_id,
            position,
        } => match manager.completion(&language_id, &uri, position).await {
            Ok(resp) => {
                let _ = event_tx
                    .send(LspTaskEvent::CompletionResult {
                        uri,
                        items: resp.items,
                        is_incomplete: resp.is_incomplete,
                    })
                    .await;
            }
            Err(e) => {
                let _ = event_tx.send(LspTaskEvent::Error { message: e }).await;
            }
        },
        LspCommand::WorkspaceSymbol { language_id, query } => {
            match manager.workspace_symbol(&language_id, &query).await {
                Ok(symbols) => {
                    let _ = event_tx
                        .send(LspTaskEvent::WorkspaceSymbolResult { symbols })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx.send(LspTaskEvent::Error { message: e }).await;
                }
            }
        }
        LspCommand::DocumentSymbols { uri, language_id } => {
            match manager.document_symbols(&language_id, &uri).await {
                Ok(symbols) => {
                    let _ = event_tx
                        .send(LspTaskEvent::DocumentSymbolResult { uri, symbols })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx.send(LspTaskEvent::Error { message: e }).await;
                }
            }
        }
        LspCommand::CodeAction {
            uri,
            language_id,
            range,
            diagnostics,
        } => match manager
            .code_action(&language_id, &uri, range, diagnostics)
            .await
        {
            Ok(actions) => {
                let _ = event_tx
                    .send(LspTaskEvent::CodeActionResult { uri, actions })
                    .await;
            }
            Err(e) => {
                let _ = event_tx.send(LspTaskEvent::Error { message: e }).await;
            }
        },
        LspCommand::DocumentHighlight {
            uri,
            language_id,
            position,
            generation,
        } => match manager
            .document_highlight(&language_id, &uri, position)
            .await
        {
            Ok(highlights) => {
                let _ = event_tx
                    .send(LspTaskEvent::DocumentHighlightResult {
                        highlights,
                        generation,
                    })
                    .await;
            }
            Err(_) => {
                // Silently ignore highlight errors — they're not user-initiated.
            }
        },
        LspCommand::Shutdown => {
            manager.shutdown_all().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manager_rejects_unconfigured_language() {
        let (tx, _rx) = mpsc::channel(16);
        let mut manager = LspManager::new(HashMap::new(), tx);
        let result = manager.did_open("rust", "file:///test.rs", "").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no LSP server configured"));
    }

    #[tokio::test]
    async fn manager_configured_languages() {
        let mut configs = HashMap::new();
        configs.insert(
            "rust".into(),
            LspServerConfig {
                command: "rust-analyzer".into(),
                args: vec![],
                root_uri: None,
            },
        );
        configs.insert(
            "python".into(),
            LspServerConfig {
                command: "pylsp".into(),
                args: vec![],
                root_uri: None,
            },
        );
        let (tx, _rx) = mpsc::channel(16);
        let manager = LspManager::new(configs, tx);
        let mut langs = manager.configured_languages();
        langs.sort();
        assert_eq!(langs, vec!["python".to_string(), "rust".to_string()]);
    }

    #[tokio::test]
    async fn did_change_noop_without_open() {
        let (tx, _rx) = mpsc::channel(16);
        let mut manager = LspManager::new(HashMap::new(), tx);
        // No client for this language — should silently succeed.
        let result = manager.did_change("rust", "file:///test.rs", "").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_lsp_task_shuts_down_on_command() {
        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        let (evt_tx, _evt_rx) = mpsc::channel(16);
        let handle = tokio::spawn(run_lsp_task(HashMap::new(), cmd_rx, evt_tx));

        cmd_tx.send(LspCommand::Shutdown).await.unwrap();
        drop(cmd_tx);

        // Task should exit within a reasonable time.
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn goto_definition_error_for_unconfigured() {
        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        let (evt_tx, mut evt_rx) = mpsc::channel(16);
        tokio::spawn(run_lsp_task(HashMap::new(), cmd_rx, evt_tx));

        cmd_tx
            .send(LspCommand::GotoDefinition {
                uri: "file:///test.rs".into(),
                language_id: "rust".into(),
                position: Position {
                    line: 0,
                    character: 0,
                },
            })
            .await
            .unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), evt_rx.recv())
            .await
            .expect("event")
            .expect("some event");
        match event {
            LspTaskEvent::Error { message } => {
                assert!(message.contains("no LSP server configured"));
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }
}
