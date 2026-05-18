//! Collab bridge — translates between editor-side intents and the TCP connection
//! to the state server, and handles incoming collab events.
//!
//! Follows the same pattern as `lsp_bridge.rs` and `dap_bridge.rs`:
//! - `drain_collab_intents()` called every tick
//! - `handle_collab_event()` handles events from the background task
//! - `run_collab_task()` is the background tokio task owning the TCP connection

use mae_core::{CollabIntent, CollabStatus, Editor};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// --- Command / Event types ---

/// Commands sent from the main thread to the collab background task.
#[derive(Debug)]
pub enum CollabCommand {
    Connect {
        address: String,
    },
    Disconnect,
    ShareBuffer {
        doc_id: String,
        state_bytes: Vec<u8>,
    },
    ForceSync {
        doc_id: String,
    },
    ShowStatus,
    Doctor,
    StartServer,
    /// Send a yrs update to the state server for a synced buffer.
    SendUpdate {
        doc_id: String,
        update_base64: String,
    },
    /// List documents on the server.
    ListDocs {
        for_join: bool,
    },
    /// Join (resync) a document from the server.
    JoinDoc {
        doc_id: String,
    },
}

/// Events sent from the collab background task back to the main thread.
#[derive(Debug)]
pub enum CollabEvent {
    Connected {
        address: String,
        peer_count: usize,
    },
    Disconnected {
        reason: String,
    },
    RemoteUpdate {
        doc_id: String,
        update_bytes: Vec<u8>,
    },
    StatusReport {
        lines: Vec<String>,
    },
    DoctorReport {
        lines: Vec<String>,
    },
    ServerStarted {
        pid: u32,
    },
    ServerFailed {
        error: String,
    },
    Error {
        message: String,
    },
    /// Buffer successfully shared with the server.
    BufferShared {
        doc_id: String,
    },
    /// Server returned the document list.
    DocList {
        documents: Vec<String>,
        for_join: bool,
    },
    /// Joined a remote document — carries the full CRDT state.
    BufferJoined {
        doc_id: String,
        state_bytes: Vec<u8>,
    },
    /// Peer count changed (peer joined or left).
    PeerCountChanged {
        peer_count: usize,
    },
    /// A peer saved a shared document.
    PeerSaved {
        doc: String,
        saved_by: String,
    },
}

// --- Intent drain (called every tick) ---

/// Drain the pending collab intent from the editor and forward to the background task.
/// Safe to call every loop iteration.
pub(crate) fn drain_collab_intents(editor: &mut Editor, collab_tx: &mpsc::Sender<CollabCommand>) {
    let intent = match editor.pending_collab_intent.take() {
        Some(i) => i,
        None => return,
    };

    let cmd = match intent {
        CollabIntent::StartServer => CollabCommand::StartServer,
        CollabIntent::Connect { address } => CollabCommand::Connect { address },
        CollabIntent::Disconnect => CollabCommand::Disconnect,
        CollabIntent::ShowStatus => CollabCommand::ShowStatus,
        CollabIntent::ShareBuffer { buffer_name } => {
            // Enable sync on the buffer if not already enabled, then encode state.
            let idx = editor.find_buffer_by_name(&buffer_name);
            if let Some(idx) = idx {
                // Compute DocAddress from file_path + project root.
                let project_root = editor.active_project_root().map(|p| p.to_path_buf());
                let buf = &mut editor.buffers[idx];
                if buf.doc_address.is_none() {
                    buf.doc_address = compute_doc_address(buf, project_root.as_deref());
                }
                if buf.sync_doc.is_none() {
                    // Use PID + buffer index as a deterministic client ID.
                    let client_id = (std::process::id() as u64) << 16 | (idx as u64);
                    buf.enable_sync(client_id);
                    // Clear pending updates from enable_sync's initial insert —
                    // the full state is sent via ShareBuffer, not incremental updates.
                    buf.pending_sync_updates.clear();
                }
                let state_bytes = buf
                    .sync_doc
                    .as_ref()
                    .map(|s| s.encode_state())
                    .unwrap_or_default();
                // Use DocAddress-based doc_name for cross-session stability,
                // falling back to buffer name for unnamed/scratch buffers.
                let doc_id = buf
                    .doc_address
                    .as_ref()
                    .map(|a| a.to_doc_name())
                    .unwrap_or_else(|| buffer_name.clone());
                CollabCommand::ShareBuffer {
                    doc_id,
                    state_bytes,
                }
            } else {
                return; // Buffer not found
            }
        }
        CollabIntent::ForceSync { buffer_name } => CollabCommand::ForceSync {
            doc_id: buffer_name,
        },
        CollabIntent::Doctor => CollabCommand::Doctor,
        CollabIntent::ListDocs => CollabCommand::ListDocs { for_join: false },
        CollabIntent::ListDocsForJoin => CollabCommand::ListDocs { for_join: true },
        CollabIntent::JoinDoc { doc_id } => CollabCommand::JoinDoc { doc_id },
    };

    let kind = collab_command_name(&cmd);
    if collab_tx.try_send(cmd).is_err() {
        warn!(
            kind,
            "collab command channel full or closed — intent dropped"
        );
    }
}

/// Compute a `DocAddress` from a buffer's file path and project root.
fn compute_doc_address(
    buf: &mae_core::Buffer,
    project_root: Option<&std::path::Path>,
) -> Option<mae_sync::DocAddress> {
    if let Some(fp) = buf.file_path() {
        let rel_path = if let Some(root) = project_root {
            fp.strip_prefix(root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| fp.to_string_lossy().to_string())
        } else {
            fp.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| fp.to_string_lossy().to_string())
        };
        // FNV-1a hash of project root for stable short identifier.
        let project_hash = if let Some(root) = project_root {
            let bytes = root.to_string_lossy();
            let mut h: u64 = 0xcbf29ce484222325;
            for b in bytes.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            format!("{h:012x}")
        } else {
            "no-project".to_string()
        };
        Some(mae_sync::DocAddress::File {
            project_hash,
            rel_path,
        })
    } else {
        // No file path — treat as shared scratch buffer.
        Some(mae_sync::DocAddress::Shared {
            name: buf.name.clone(),
        })
    }
}

fn collab_command_name(cmd: &CollabCommand) -> &'static str {
    match cmd {
        CollabCommand::Connect { .. } => "connect",
        CollabCommand::Disconnect => "disconnect",
        CollabCommand::ShareBuffer { .. } => "share-buffer",
        CollabCommand::ForceSync { .. } => "force-sync",
        CollabCommand::ShowStatus => "show-status",
        CollabCommand::Doctor => "doctor",
        CollabCommand::StartServer => "start-server",
        CollabCommand::SendUpdate { .. } => "send-update",
        CollabCommand::ListDocs { .. } => "list-docs",
        CollabCommand::JoinDoc { .. } => "join-doc",
    }
}

// --- Event handling (main thread) ---

/// Handle an event from the collab background task — update editor state.
pub(crate) fn handle_collab_event(editor: &mut Editor, event: CollabEvent) {
    match event {
        CollabEvent::Connected {
            address,
            peer_count,
        } => {
            info!(address = %address, peers = peer_count, "collab connected");
            editor.collab_status = CollabStatus::Connected { peer_count };
            editor.set_status(format!("Connected to {} ({} peers)", address, peer_count));
            editor.mark_full_redraw();
        }
        CollabEvent::Disconnected { reason } => {
            info!(reason = %reason, "collab disconnected");
            editor.collab_status = CollabStatus::Disconnected;
            editor.set_status(format!("Collab disconnected: {}", reason));
            // Clear sync state on all synced buffers to prevent stale docs
            // from causing content duplication on reconnect.
            for buf_name in &editor.collab_synced_buffers.clone() {
                if let Some(idx) = editor.find_buffer_by_name(buf_name) {
                    editor.buffers[idx].sync_doc = None;
                    editor.buffers[idx].pending_sync_updates.clear();
                }
            }
            editor.collab_synced_docs = 0;
            editor.collab_synced_buffers.clear();
            editor.mark_full_redraw();
        }
        CollabEvent::RemoteUpdate {
            doc_id,
            update_bytes,
        } => {
            if let Some(idx) = editor.find_buffer_by_name(&doc_id) {
                match editor.buffers[idx].apply_sync_update(&update_bytes) {
                    Ok(()) => {
                        debug!(doc = %doc_id, update_bytes = update_bytes.len(), "applied remote sync update");
                        editor.mark_full_redraw();
                    }
                    Err(e) => {
                        warn!(doc = %doc_id, error = %e, "failed to apply remote sync update");
                    }
                }
            } else {
                warn!(doc = %doc_id, "remote update for unknown buffer — name mismatch?");
            }
        }
        CollabEvent::StatusReport { lines } => {
            let content = lines.join("\n");
            let idx = editor.find_or_create_buffer("*Collab Status*", || {
                let mut buf = mae_core::Buffer::new();
                buf.name = "*Collab Status*".to_string();
                buf.kind = mae_core::BufferKind::Text;
                buf
            });
            editor.buffers[idx].replace_contents(&content);
            editor.switch_to_buffer(idx);
            editor.mark_full_redraw();
        }
        CollabEvent::DoctorReport { lines } => {
            let content = lines.join("\n");
            let idx = editor.find_or_create_buffer("*Collab Doctor*", || {
                let mut buf = mae_core::Buffer::new();
                buf.name = "*Collab Doctor*".to_string();
                buf.kind = mae_core::BufferKind::Text;
                buf
            });
            editor.buffers[idx].replace_contents(&content);
            editor.switch_to_buffer(idx);
            editor.mark_full_redraw();
        }
        CollabEvent::ServerStarted { pid } => {
            info!(pid = pid, "state server started");
            editor.set_status(format!("State server started (PID {})", pid));
            editor.mark_full_redraw();
        }
        CollabEvent::ServerFailed { error } => {
            error!(error = %error, "state server failed to start");
            editor.set_status(format!("State server failed: {}", error));
            editor.mark_full_redraw();
        }
        CollabEvent::Error { message } => {
            warn!(error = %message, "collab error");
            editor.set_status(format!("Collab: {}", message));
            editor.mark_full_redraw();
        }
        CollabEvent::BufferShared { doc_id } => {
            info!(doc = %doc_id, "buffer shared");
            editor.collab_synced_buffers.insert(doc_id.clone());
            editor.collab_synced_docs = editor.collab_synced_buffers.len();
            editor.set_status(format!("Shared: {}", doc_id));
            editor.mark_full_redraw();
        }
        CollabEvent::DocList {
            documents,
            for_join,
        } => {
            if for_join {
                // Open a palette picker with the document names.
                if documents.is_empty() {
                    editor.set_status("No documents on server");
                } else {
                    let names: Vec<&str> = documents.iter().map(|s| s.as_str()).collect();
                    let palette =
                        mae_core::command_palette::CommandPalette::for_collab_join(&names);
                    editor.command_palette = Some(palette);
                    editor.set_mode(mae_core::Mode::CommandPalette);
                    editor.mark_full_redraw();
                }
            } else {
                // Create a *Collab Docs* buffer with the listing.
                let content = if documents.is_empty() {
                    "No documents shared on server.".to_string()
                } else {
                    let mut lines = vec![format!(
                        "Shared Documents ({})\n{}",
                        documents.len(),
                        "=".repeat(40)
                    )];
                    for doc in &documents {
                        lines.push(format!("  {}", doc));
                    }
                    lines.push(String::new());
                    lines
                        .push("Use :collab-join <name> or SPC C j to join a document.".to_string());
                    lines.join("\n")
                };
                let idx = editor.find_or_create_buffer("*Collab Docs*", || {
                    let mut buf = mae_core::Buffer::new();
                    buf.name = "*Collab Docs*".to_string();
                    buf.kind = mae_core::BufferKind::Text;
                    buf
                });
                editor.buffers[idx].replace_contents(&content);
                editor.switch_to_buffer(idx);
                editor.mark_full_redraw();
            }
        }
        CollabEvent::BufferJoined {
            doc_id,
            state_bytes,
        } => {
            // Parse DocAddress from doc_id for structured addressing.
            let doc_addr = mae_sync::DocAddress::parse(&doc_id);
            // Use a display-friendly name for the buffer.
            let buf_name = match &doc_addr {
                Some(mae_sync::DocAddress::File { rel_path, .. }) => rel_path.clone(),
                _ => doc_id.clone(),
            };
            // Find or create buffer, load sync state directly (no merge).
            let idx = editor.find_or_create_buffer(&buf_name, || {
                let mut buf = mae_core::Buffer::new();
                buf.name = buf_name.clone();
                buf.kind = mae_core::BufferKind::Text;
                buf
            });
            // Snapshot project root before mutable borrow of buffer.
            let project_root = editor.active_project_root().map(|p| p.to_path_buf());
            // Deterministic client ID: PID << 16 | buffer index.
            let client_id = (std::process::id() as u64) << 16 | (idx as u64);
            let load_ok = {
                let buf = &mut editor.buffers[idx];
                match buf.load_sync_state(&state_bytes, client_id) {
                    Ok(()) => {
                        // Set doc_address for save policy resolution.
                        buf.doc_address = doc_addr.clone();
                        // Resolve file_path from DocAddress or doc_id.
                        // Always set file_path — file may not exist yet (created on :w).
                        if buf.file_path().is_none() {
                            let rel = match &doc_addr {
                                Some(mae_sync::DocAddress::File { rel_path, .. }) => {
                                    rel_path.clone()
                                }
                                _ => doc_id.clone(),
                            };
                            // Try project_root/rel_path first, then CWD/rel_path.
                            let resolved = if let Some(root) = &project_root {
                                let rooted = root.join(&rel);
                                if rooted.exists() {
                                    rooted.canonicalize().unwrap_or(rooted)
                                } else {
                                    rooted // set even if doesn't exist
                                }
                            } else if let Ok(cwd) = std::env::current_dir() {
                                let cwd_path = cwd.join(&rel);
                                if cwd_path.exists() {
                                    cwd_path.canonicalize().unwrap_or(cwd_path)
                                } else {
                                    cwd_path // set even if doesn't exist
                                }
                            } else {
                                std::path::PathBuf::from(&rel)
                            };
                            buf.set_file_path(resolved);
                        }
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            };
            match load_ok {
                Ok(()) => {
                    // Detect language from doc_id for syntax highlighting.
                    {
                        let content = editor.buffers[idx].text();
                        let path_hint = std::path::Path::new(&doc_id);
                        if let Some(lang) =
                            mae_core::syntax::language_for_buffer(path_hint, &content)
                        {
                            editor.syntax.set_language(idx, lang);
                            editor.buffers[idx]
                                .local_options
                                .apply_defaults(&lang.default_local_options());
                            // Force tree-sitter reparse so the full structural
                            // parser (compute_org_spans) runs on the joined buffer.
                            editor.syntax.invalidate(idx);
                        }
                    }
                    editor.collab_synced_buffers.insert(doc_id.clone());
                    editor.collab_synced_docs = editor.collab_synced_buffers.len();
                    editor.switch_to_buffer(idx);
                    editor.set_status(format!("Joined: {}", doc_id));
                    editor.mark_full_redraw();
                }
                Err(e) => {
                    editor.set_status(format!("Failed to join {}: {}", doc_id, e));
                }
            }
        }
        CollabEvent::PeerCountChanged { peer_count } => {
            if let CollabStatus::Connected { .. } = editor.collab_status {
                editor.collab_status = CollabStatus::Connected { peer_count };
                editor.set_status(format!("Peer count: {}", peer_count));
                editor.mark_full_redraw();
            }
        }
        CollabEvent::PeerSaved { doc, saved_by } => {
            editor.set_status(format!("[{}] saved by {}", doc, saved_by));
            // Mark the local buffer clean if we have it (content matches what was saved).
            if let Some(idx) = editor.find_buffer_by_name(&doc) {
                editor.buffers[idx].modified = false;
            }
            editor.mark_full_redraw();
        }
    }
}

// --- Background task ---

/// Deferred spawn state — holds the background task's channel ends and config.
/// Created by `setup_collab_channels`, consumed by `spawn_collab_task`.
pub(crate) struct CollabSpawn {
    cmd_rx: mpsc::Receiver<CollabCommand>,
    evt_tx: mpsc::Sender<CollabEvent>,
    reconnect_secs: u64,
    write_timeout_ms: u64,
    auto_connect_addr: Option<String>,
    cmd_tx_clone: mpsc::Sender<CollabCommand>,
}

/// Create collab channels and read config. Does NOT require a tokio runtime.
/// Returns `(event_rx, command_tx, spawn)` — caller must pass `spawn` to
/// `spawn_collab_task()` from within a tokio runtime context.
pub(crate) fn setup_collab_channels(
    editor: &Editor,
) -> (
    mpsc::Receiver<CollabEvent>,
    mpsc::Sender<CollabCommand>,
    CollabSpawn,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<CollabCommand>(32);
    let (evt_tx, evt_rx) = mpsc::channel::<CollabEvent>(64);

    let reconnect_secs = editor.collab_reconnect_interval;
    let write_timeout_ms = editor.collab_write_timeout_ms;

    let auto_connect_addr =
        if editor.collab_auto_connect && !editor.collab_server_address.is_empty() {
            Some(editor.collab_server_address.clone())
        } else {
            None
        };

    let spawn = CollabSpawn {
        cmd_rx,
        evt_tx,
        reconnect_secs,
        write_timeout_ms,
        auto_connect_addr,
        cmd_tx_clone: cmd_tx.clone(),
    };

    (evt_rx, cmd_tx, spawn)
}

/// Spawn the collab background task. MUST be called from within a tokio runtime.
pub(crate) fn spawn_collab_task(spawn: CollabSpawn) {
    let write_timeout = std::time::Duration::from_millis(spawn.write_timeout_ms);
    tokio::spawn(run_collab_task(
        spawn.cmd_rx,
        spawn.evt_tx,
        spawn.reconnect_secs,
        write_timeout,
    ));

    // Auto-connect if configured
    if let Some(addr) = spawn.auto_connect_addr {
        let _ = spawn
            .cmd_tx_clone
            .try_send(CollabCommand::Connect { address: addr });
    }
}

/// Kinds of pending request-response correlations.
#[derive(Debug)]
enum PendingResponseKind {
    ListDocs { for_join: bool },
    JoinDoc { doc_id: String },
    ShareBuffer { doc_id: String },
    ForceSync { doc_id: String },
    SyncUpdate { doc_id: String },
    Subscribe,
}

/// Background task that owns the TCP connection to the state server.
///
/// Receives commands from the main thread, manages the connection lifecycle,
/// and forwards events back.
async fn run_collab_task(
    mut cmd_rx: mpsc::Receiver<CollabCommand>,
    evt_tx: mpsc::Sender<CollabEvent>,
    reconnect_secs: u64,
    write_timeout: std::time::Duration,
) {
    use mae_mcp::{read_message, write_framed};
    use std::collections::HashMap;
    use tokio::io::BufReader;
    use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::net::TcpStream;

    let mut reader: Option<BufReader<OwnedReadHalf>> = None;
    let mut writer: Option<OwnedWriteHalf> = None;
    let mut target_address: Option<String> = None;
    let mut shared_docs: Vec<String> = Vec::new();
    let mut reconnect_enabled = false;
    let mut next_request_id: u64 = 10; // Start after handshake IDs
    let mut pending_responses: HashMap<u64, PendingResponseKind> = HashMap::new();

    /// Helper: set up owned read/write halves from a fresh TCP stream.
    fn install_connection(
        stream: TcpStream,
        rd: &mut Option<BufReader<OwnedReadHalf>>,
        wr: &mut Option<OwnedWriteHalf>,
    ) {
        let (r, w) = stream.into_split();
        *rd = Some(BufReader::new(r));
        *wr = Some(w);
    }

    /// Helper: tear down connection.
    fn tear_down(rd: &mut Option<BufReader<OwnedReadHalf>>, wr: &mut Option<OwnedWriteHalf>) {
        *rd = None;
        *wr = None;
    }

    loop {
        let connected = reader.is_some();

        if connected {
            let buf_reader = reader.as_mut().unwrap();

            tokio::select! {
                Some(cmd) = cmd_rx.recv() => {
                    match cmd {
                        CollabCommand::Disconnect => {
                            tear_down(&mut reader, &mut writer);
                            reconnect_enabled = false;
                            shared_docs.clear();
                            pending_responses.clear();
                            let _ = evt_tx.send(CollabEvent::Disconnected {
                                reason: "user requested".to_string(),
                            }).await;
                            continue;
                        }
                        CollabCommand::ShowStatus => {
                            let lines = build_status_lines(
                                target_address.as_deref().unwrap_or("?"),
                                true,
                                &shared_docs,
                            );
                            let _ = evt_tx.send(CollabEvent::StatusReport { lines }).await;
                        }
                        CollabCommand::Doctor => {
                            let lines = build_doctor_lines(
                                target_address.as_deref().unwrap_or("?"),
                                true,
                            );
                            let _ = evt_tx.send(CollabEvent::DoctorReport { lines }).await;
                        }
                        CollabCommand::ShareBuffer { doc_id, state_bytes } => {
                            if let Some(ref mut w) = writer {
                                // Atomic share: server deletes old doc + applies update in one step.
                                let update_b64 = mae_sync::encoding::update_to_base64(&state_bytes);
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "sync/share",
                                    "params": {
                                        "doc": doc_id,
                                        "update": update_b64,
                                    }
                                });
                                let body = serde_json::to_vec(&req).unwrap();
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::ShareBuffer { doc_id });
                                } else {
                                    let _ = evt_tx.send(CollabEvent::Error {
                                        message: format!("Failed to share {}", doc_id),
                                    }).await;
                                }
                            }
                        }
                        CollabCommand::ForceSync { doc_id } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "sync/full_state",
                                    "params": { "doc": doc_id }
                                });
                                let body = serde_json::to_vec(&req).unwrap();
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::ForceSync { doc_id });
                                } else {
                                    let _ = evt_tx.send(CollabEvent::Error {
                                        message: format!("Failed to sync {}", doc_id),
                                    }).await;
                                }
                            }
                        }
                        CollabCommand::SendUpdate { doc_id, update_base64 } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "sync/update",
                                    "params": {
                                        "doc": doc_id,
                                        "update": update_base64,
                                    }
                                });
                                let body = serde_json::to_vec(&req).unwrap();
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::SyncUpdate { doc_id });
                                }
                            }
                        }
                        CollabCommand::ListDocs { for_join } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "docs/list",
                                });
                                let body = serde_json::to_vec(&req).unwrap();
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::ListDocs { for_join });
                                } else {
                                    let _ = evt_tx.send(CollabEvent::Error {
                                        message: "Failed to list documents".to_string(),
                                    }).await;
                                }
                            }
                        }
                        CollabCommand::JoinDoc { doc_id } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "sync/resync",
                                    "params": { "doc": doc_id },
                                });
                                let body = serde_json::to_vec(&req).unwrap();
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::JoinDoc { doc_id: doc_id.clone() });
                                    if !shared_docs.contains(&doc_id) {
                                        shared_docs.push(doc_id);
                                    }
                                } else {
                                    let _ = evt_tx.send(CollabEvent::Error {
                                        message: format!("Failed to join {}", doc_id),
                                    }).await;
                                }
                            }
                        }
                        CollabCommand::Connect { address } => {
                            tear_down(&mut reader, &mut writer);
                            pending_responses.clear();
                            target_address = Some(address);
                            continue;
                        }
                        CollabCommand::StartServer => {
                            let _ = evt_tx.send(CollabEvent::Error {
                                message: "Already connected to a state server".to_string(),
                            }).await;
                        }
                    }
                }
                msg = read_message(buf_reader) => {
                    match msg {
                        Ok(Some(text)) => {
                            handle_incoming_message(
                                &text,
                                &evt_tx,
                                &mut pending_responses,
                                &mut shared_docs,
                            ).await;
                        }
                        Ok(None) | Err(_) => {
                            tear_down(&mut reader, &mut writer);
                            shared_docs.clear();
                            pending_responses.clear();
                            let _ = evt_tx.send(CollabEvent::Disconnected {
                                reason: "connection lost".to_string(),
                            }).await;
                            if reconnect_enabled {
                                continue;
                            }
                        }
                    }
                }
            }
        } else {
            // No connection — wait for commands or handle reconnection
            if reconnect_enabled {
                if let Some(ref addr) = target_address {
                    let addr_clone = addr.clone();
                    tokio::select! {
                        Some(cmd) = cmd_rx.recv() => {
                            handle_disconnected_cmd(
                                cmd, &evt_tx, &mut reader, &mut writer,
                                &mut target_address, &mut reconnect_enabled,
                                &mut shared_docs, &mut next_request_id,
                                &mut pending_responses, write_timeout,
                            ).await;
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(reconnect_secs)) => {
                            if let Ok(mut stream) = TcpStream::connect(&addr_clone).await {
                                if let Some(peer_count) = send_initialize(&mut stream, write_timeout).await {
                                    install_connection(stream, &mut reader, &mut writer);
                                    // Subscribe to sync_update events (B4 fix).
                                    if let Some(ref mut w) = writer {
                                        send_subscribe(w, &mut next_request_id, &mut pending_responses, write_timeout).await;
                                    }
                                    let _ = evt_tx.send(CollabEvent::Connected {
                                        address: addr_clone,
                                        peer_count,
                                    }).await;
                                }
                            } else {
                                debug!(addr = %addr_clone, "reconnect failed, will retry");
                            }
                        }
                    }
                } else {
                    reconnect_enabled = false;
                }
            } else {
                let Some(cmd) = cmd_rx.recv().await else {
                    break;
                };
                handle_disconnected_cmd(
                    cmd,
                    &evt_tx,
                    &mut reader,
                    &mut writer,
                    &mut target_address,
                    &mut reconnect_enabled,
                    &mut shared_docs,
                    &mut next_request_id,
                    &mut pending_responses,
                    write_timeout,
                )
                .await;
            }
        }
    }
}

/// Handle an incoming JSON-RPC message from the server.
/// Dispatches to response handler or notification handler based on content.
async fn handle_incoming_message(
    text: &str,
    evt_tx: &mpsc::Sender<CollabEvent>,
    pending_responses: &mut std::collections::HashMap<u64, PendingResponseKind>,
    shared_docs: &mut Vec<String>,
) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };

    // Case 1: JSON-RPC response (has `id` + (`result` or `error`), no `method`)
    if let Some(id) = val.get("id").and_then(|v| v.as_u64()) {
        if val.get("method").is_none() {
            if let Some(kind) = pending_responses.remove(&id) {
                handle_response(&val, kind, evt_tx, shared_docs).await;
            }
            return;
        }
    }

    // Case 2: Server notification (has `method`, no `id` or id is null)
    if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
        match method {
            // B3 fix: server sends "notifications/sync_update" with nested event data.
            "notifications/sync_update" => {
                debug!("received sync_update notification from server");
                if let Some(params) = val.get("params") {
                    // Server format: {"params": {"seq": N, "event": {"type": "sync_update", "data": {"buffer_name": "...", "update_base64": "..."}}}}
                    // The "data" key comes from serde's #[serde(tag = "type", content = "data")] on EditorEvent.
                    let event_data = params
                        .get("event")
                        .and_then(|e| e.get("data").or_else(|| e.get("sync_update")));
                    if let Some(sync_data) = event_data {
                        let buffer_name = sync_data
                            .get("buffer_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let update_b64 = sync_data
                            .get("update_base64")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if let Ok(bytes) = mae_sync::encoding::base64_to_update(update_b64) {
                            let _ = evt_tx
                                .send(CollabEvent::RemoteUpdate {
                                    doc_id: buffer_name,
                                    update_bytes: bytes,
                                })
                                .await;
                        }
                    }
                }
            }
            // Also handle direct sync/update format (legacy / future compat).
            "sync/update" => {
                if let Some(params) = val.get("params") {
                    let doc_id = params
                        .get("doc")
                        .or_else(|| params.get("buffer_name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let update_b64 = params
                        .get("update")
                        .or_else(|| params.get("update_base64"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if let Ok(bytes) = mae_sync::encoding::base64_to_update(update_b64) {
                        let _ = evt_tx
                            .send(CollabEvent::RemoteUpdate {
                                doc_id,
                                update_bytes: bytes,
                            })
                            .await;
                    }
                }
            }
            "notifications/peer_joined" => {
                if let Some(params) = val.get("params") {
                    let event = params.get("event").unwrap_or(params);
                    let data = event.get("data").unwrap_or(event);
                    let peer_count =
                        data.get("peer_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let _ = evt_tx
                        .send(CollabEvent::PeerCountChanged { peer_count })
                        .await;
                }
            }
            "notifications/peer_left" => {
                if let Some(params) = val.get("params") {
                    let event = params.get("event").unwrap_or(params);
                    let data = event.get("data").unwrap_or(event);
                    let peer_count =
                        data.get("peer_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let _ = evt_tx
                        .send(CollabEvent::PeerCountChanged { peer_count })
                        .await;
                }
            }
            "notifications/save_committed" => {
                if let Some(params) = val.get("params") {
                    let event = params.get("event").unwrap_or(params);
                    let data = event.get("data").unwrap_or(event);
                    let doc = data
                        .get("doc")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let saved_by = data
                        .get("saved_by")
                        .and_then(|v| v.as_str())
                        .unwrap_or("peer")
                        .to_string();
                    let _ = evt_tx.send(CollabEvent::PeerSaved { doc, saved_by }).await;
                }
            }
            _ => {
                debug!(method = method, "unhandled server notification");
            }
        }
    }
}

/// Handle a correlated JSON-RPC response based on the pending request kind.
async fn handle_response(
    val: &serde_json::Value,
    kind: PendingResponseKind,
    evt_tx: &mpsc::Sender<CollabEvent>,
    shared_docs: &mut Vec<String>,
) {
    let result = val.get("result");

    match kind {
        PendingResponseKind::ShareBuffer { doc_id } => {
            if val.get("error").is_some() {
                let _ = evt_tx
                    .send(CollabEvent::Error {
                        message: format!("Failed to share {}", doc_id),
                    })
                    .await;
            } else {
                if !shared_docs.contains(&doc_id) {
                    shared_docs.push(doc_id.clone());
                }
                let _ = evt_tx.send(CollabEvent::BufferShared { doc_id }).await;
            }
        }
        PendingResponseKind::ListDocs { for_join } => {
            let documents = result
                .and_then(|r| r.get("documents"))
                .and_then(|d| d.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let _ = evt_tx
                .send(CollabEvent::DocList {
                    documents,
                    for_join,
                })
                .await;
        }
        PendingResponseKind::JoinDoc { doc_id } => {
            // sync/resync response: {"result": {"doc": "...", "state": "<base64>", "sv": "<base64>"}}
            let state_b64 = result
                .and_then(|r| r.get("state"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            match mae_sync::encoding::base64_to_update(state_b64) {
                Ok(state_bytes) => {
                    let _ = evt_tx
                        .send(CollabEvent::BufferJoined {
                            doc_id,
                            state_bytes,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = evt_tx
                        .send(CollabEvent::Error {
                            message: format!("Failed to decode state for {}: {}", doc_id, e),
                        })
                        .await;
                }
            }
        }
        PendingResponseKind::ForceSync { doc_id } => {
            // sync/full_state response: {"result": {"doc": "...", "state": "<base64>"}}
            // Use BufferJoined (load_sync_state path) to avoid content duplication
            // that occurs when applying full state as an incremental update.
            let state_b64 = result
                .and_then(|r| r.get("state"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if !state_b64.is_empty() {
                match mae_sync::encoding::base64_to_update(state_b64) {
                    Ok(state_bytes) => {
                        let _ = evt_tx
                            .send(CollabEvent::BufferJoined {
                                doc_id,
                                state_bytes,
                            })
                            .await;
                    }
                    Err(e) => {
                        let _ = evt_tx
                            .send(CollabEvent::Error {
                                message: format!("Failed to decode resync for {}: {}", doc_id, e),
                            })
                            .await;
                    }
                }
            }
        }
        PendingResponseKind::SyncUpdate { doc_id } => {
            if let Some(err) = val.get("error") {
                warn!(doc = %doc_id, error = ?err, "server rejected sync update");
            }
        }
        PendingResponseKind::Subscribe => {
            // Acknowledgement — no action needed.
        }
    }
}

/// Send `notifications/subscribe` to opt into sync_update events (B4 fix).
async fn send_subscribe(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    next_id: &mut u64,
    pending: &mut std::collections::HashMap<u64, PendingResponseKind>,
    timeout: std::time::Duration,
) {
    use mae_mcp::write_framed;

    let req_id = *next_id;
    *next_id += 1;
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "notifications/subscribe",
        "params": {
            "types": ["sync_update"]
        }
    });
    let body = serde_json::to_vec(&req).unwrap();
    if write_framed(writer, &body, timeout).await.is_ok() {
        pending.insert(req_id, PendingResponseKind::Subscribe);
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_disconnected_cmd(
    cmd: CollabCommand,
    evt_tx: &mpsc::Sender<CollabEvent>,
    reader: &mut Option<tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>>,
    writer: &mut Option<tokio::net::tcp::OwnedWriteHalf>,
    target_address: &mut Option<String>,
    reconnect_enabled: &mut bool,
    shared_docs: &mut Vec<String>,
    next_request_id: &mut u64,
    pending_responses: &mut std::collections::HashMap<u64, PendingResponseKind>,
    write_timeout: std::time::Duration,
) {
    use tokio::io::BufReader;

    match cmd {
        CollabCommand::Connect { address } => {
            *target_address = Some(address.clone());
            match tokio::net::TcpStream::connect(&address).await {
                Ok(mut stream) => {
                    if let Some(peer_count) = send_initialize(&mut stream, write_timeout).await {
                        let (r, w) = stream.into_split();
                        *reader = Some(BufReader::new(r));
                        *writer = Some(w);
                        *reconnect_enabled = true;
                        // Subscribe to sync_update events (B4 fix).
                        if let Some(ref mut w) = writer {
                            send_subscribe(w, next_request_id, pending_responses, write_timeout)
                                .await;
                        }
                        let _ = evt_tx
                            .send(CollabEvent::Connected {
                                address,
                                peer_count,
                            })
                            .await;
                    } else {
                        *reconnect_enabled = true;
                        let _ = evt_tx
                            .send(CollabEvent::Error {
                                message: format!("Handshake failed with {}", address),
                            })
                            .await;
                    }
                }
                Err(e) => {
                    *reconnect_enabled = true;
                    let _ = evt_tx
                        .send(CollabEvent::Error {
                            message: format!("Cannot connect to {}: {}", address, e),
                        })
                        .await;
                }
            }
        }
        CollabCommand::StartServer => {
            match tokio::process::Command::new("mae-state-server")
                .arg("start")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(child) => {
                    let pid = child.id().unwrap_or(0);
                    if let Err(e) = evt_tx.send(CollabEvent::ServerStarted { pid }).await {
                        warn!("failed to send ServerStarted event: {}", e);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let default_addr = mae_core::DEFAULT_COLLAB_ADDRESS.to_string();
                    let addr = target_address
                        .clone()
                        .unwrap_or_else(|| default_addr.clone());
                    *target_address = Some(addr.clone());
                    match tokio::net::TcpStream::connect(&addr).await {
                        Ok(mut stream) => {
                            if let Some(peer_count) =
                                send_initialize(&mut stream, write_timeout).await
                            {
                                let (r, w) = stream.into_split();
                                *reader = Some(BufReader::new(r));
                                *writer = Some(w);
                                *reconnect_enabled = true;
                                // Subscribe after server start too.
                                if let Some(ref mut w) = writer {
                                    send_subscribe(
                                        w,
                                        next_request_id,
                                        pending_responses,
                                        write_timeout,
                                    )
                                    .await;
                                }
                                if let Err(e) = evt_tx
                                    .send(CollabEvent::Connected {
                                        address: addr,
                                        peer_count,
                                    })
                                    .await
                                {
                                    warn!("failed to send Connected event: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            let _ = evt_tx
                                .send(CollabEvent::Error {
                                    message: format!("Server started but connect failed: {}", e),
                                })
                                .await;
                        }
                    }
                }
                Err(e) => {
                    let _ = evt_tx
                        .send(CollabEvent::ServerFailed {
                            error: format!("Failed to spawn mae-state-server: {}", e),
                        })
                        .await;
                }
            }
        }
        CollabCommand::ShowStatus => {
            let lines = build_status_lines(
                target_address.as_deref().unwrap_or("not configured"),
                false,
                shared_docs,
            );
            let _ = evt_tx.send(CollabEvent::StatusReport { lines }).await;
        }
        CollabCommand::Doctor => {
            let lines =
                build_doctor_lines(target_address.as_deref().unwrap_or("not configured"), false);
            let _ = evt_tx.send(CollabEvent::DoctorReport { lines }).await;
        }
        CollabCommand::Disconnect => {
            *reconnect_enabled = false;
            shared_docs.clear();
        }
        CollabCommand::ShareBuffer { doc_id, .. } => {
            let _ = evt_tx
                .send(CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot share '{}'", doc_id),
                })
                .await;
        }
        CollabCommand::ForceSync { doc_id } => {
            let _ = evt_tx
                .send(CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot sync '{}'", doc_id),
                })
                .await;
        }
        CollabCommand::SendUpdate { .. } => {
            // Silently drop — not connected.
        }
        CollabCommand::ListDocs { .. } => {
            let _ = evt_tx
                .send(CollabEvent::Error {
                    message: "Not connected \u{2014} cannot list documents".to_string(),
                })
                .await;
        }
        CollabCommand::JoinDoc { doc_id } => {
            let _ = evt_tx
                .send(CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot join '{}'", doc_id),
                })
                .await;
        }
    }
}

/// Send JSON-RPC `initialize` handshake to the state server.
/// Returns `Some(peer_count)` on success, `None` on failure.
/// Reads the response to extract `serverInfo.connections`.
async fn send_initialize(
    stream: &mut tokio::net::TcpStream,
    timeout: std::time::Duration,
) -> Option<usize> {
    use mae_mcp::write_framed;

    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "client_name": "mae-editor",
            "version": env!("CARGO_PKG_VERSION"),
        }
    });
    let body = serde_json::to_vec(&init_req).unwrap();
    if write_framed(stream, &body, timeout).await.is_err() {
        return None;
    }

    // Read the initialize response before the stream is split.
    let mut buf_reader = tokio::io::BufReader::new(&mut *stream);
    match mae_mcp::read_message(&mut buf_reader).await {
        Ok(Some(text)) => {
            let peer_count = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("result")?
                        .get("serverInfo")?
                        .get("connections")?
                        .as_u64()
                })
                .map(|c| c as usize)
                .unwrap_or(0);
            Some(peer_count)
        }
        _ => None,
    }
}

fn build_status_lines(address: &str, connected: bool, shared_docs: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("Collaborative Editing Status".to_string());
    lines.push(String::from_utf8(vec![b'='; 40]).unwrap());
    lines.push(format!(
        "Connection:  {}",
        if connected {
            format!("Connected ({})", address)
        } else {
            "Disconnected".to_string()
        }
    ));
    lines.push(String::new());
    if shared_docs.is_empty() {
        lines.push("No documents shared.".to_string());
    } else {
        lines.push(format!("Synced Documents ({}):", shared_docs.len()));
        for doc in shared_docs {
            lines.push(format!("  {}", doc));
        }
    }
    lines.push(String::new());
    lines.push(format!("Server: {}", address));
    lines
}

fn build_doctor_lines(address: &str, connected: bool) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("Collab Doctor".to_string());
    lines.push(String::from_utf8(vec![b'='; 20]).unwrap());
    if connected {
        lines.push(format!("\u{2713} State server reachable ({})", address));
        lines.push("\u{2713} Protocol: JSON-RPC 2.0 (Content-Length framing)".to_string());
        lines.push(format!(
            "\u{2713} Client version: {}",
            env!("CARGO_PKG_VERSION")
        ));
    } else {
        lines.push(format!("\u{2717} State server not reachable ({})", address));
        lines.push(String::new());
        lines.push("Troubleshooting:".to_string());
        lines.push("  1. Is mae-state-server running?".to_string());
        lines.push("     Start: systemctl --user start mae-state-server".to_string());
        lines.push(format!("     Or:    mae-state-server --bind {}", address));
        lines.push("  2. Check if the port is listening:".to_string());
        lines.push("     ss -tlnp | grep 9473".to_string());
        lines.push("  3. Check firewall:".to_string());
        lines.push(
            "     Fedora:  sudo firewall-cmd --add-port=9473/tcp --permanent && sudo firewall-cmd --reload"
                .to_string(),
        );
        lines.push("     Ubuntu:  sudo ufw allow 9473/tcp".to_string());
        lines.push(format!(
            "  4. Test connectivity: nc -zv {} {}",
            address.split(':').next().unwrap_or("127.0.0.1"),
            address.split(':').next_back().unwrap_or("9473")
        ));
        lines.push("  5. Use SPC C s to start a local server".to_string());
    }
    lines.push(String::new());
    lines.push("Commands:".to_string());
    lines.push("  SPC C l  — list shared documents on server".to_string());
    lines.push("  SPC C j  — join a shared document".to_string());
    lines.push(String::new());
    lines.push("! No authentication configured (trusted LAN mode)".to_string());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_collab_intent_connect() {
        let mut editor = Editor::new();
        editor.pending_collab_intent = Some(CollabIntent::Connect {
            address: "127.0.0.1:9473".to_string(),
        });
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        assert!(editor.pending_collab_intent.is_none());
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, CollabCommand::Connect { .. }));
    }

    #[test]
    fn drain_collab_intent_empty_is_noop() {
        let mut editor = Editor::new();
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drain_collab_share_enables_sync() {
        let mut editor = Editor::new();
        let buf_name = editor.buffers[0].name.clone();
        editor.pending_collab_intent = Some(CollabIntent::ShareBuffer {
            buffer_name: buf_name.clone(),
        });
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        match cmd {
            CollabCommand::ShareBuffer {
                doc_id,
                state_bytes,
            } => {
                // Buffer with no file_path gets DocAddress::Shared, serialized as "shared:{name}".
                assert_eq!(doc_id, format!("shared:{}", buf_name));
                assert!(
                    !state_bytes.is_empty(),
                    "state bytes should be non-empty after enable_sync"
                );
            }
            other => panic!("expected ShareBuffer, got {:?}", other),
        }
        // Sync should now be enabled on the buffer.
        assert!(editor.buffers[0].sync_doc.is_some());
    }

    #[test]
    fn drain_collab_list_docs() {
        let mut editor = Editor::new();
        editor.pending_collab_intent = Some(CollabIntent::ListDocs);
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, CollabCommand::ListDocs { for_join: false }));
    }

    #[test]
    fn drain_collab_join_doc() {
        let mut editor = Editor::new();
        editor.pending_collab_intent = Some(CollabIntent::JoinDoc {
            doc_id: "test.org".to_string(),
        });
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        match cmd {
            CollabCommand::JoinDoc { doc_id } => assert_eq!(doc_id, "test.org"),
            other => panic!("expected JoinDoc, got {:?}", other),
        }
    }

    #[test]
    fn handle_connected_event() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::Connected {
                address: "127.0.0.1:9473".to_string(),
                peer_count: 2,
            },
        );
        assert_eq!(
            editor.collab_status,
            CollabStatus::Connected { peer_count: 2 }
        );
    }

    #[test]
    fn handle_disconnected_event() {
        let mut editor = Editor::new();
        editor.collab_status = CollabStatus::Connected { peer_count: 1 };
        editor.collab_synced_buffers.insert("test.rs".to_string());
        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );
        assert_eq!(editor.collab_status, CollabStatus::Disconnected);
        assert_eq!(editor.collab_synced_docs, 0);
        assert!(editor.collab_synced_buffers.is_empty());
    }

    #[test]
    fn handle_buffer_shared_event() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferShared {
                doc_id: "main.rs".to_string(),
            },
        );
        assert!(editor.collab_synced_buffers.contains("main.rs"));
        assert_eq!(editor.collab_synced_docs, 1);
        assert!(editor.status_msg.contains("Shared: main.rs"));
    }

    #[test]
    fn handle_doc_list_event_creates_buffer() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::DocList {
                documents: vec!["a.rs".to_string(), "b.rs".to_string()],
                for_join: false,
            },
        );
        let idx = editor.find_buffer_by_name("*Collab Docs*");
        assert!(idx.is_some());
        let buf = &editor.buffers[idx.unwrap()];
        assert!(buf.text().contains("a.rs"));
        assert!(buf.text().contains("b.rs"));
    }

    #[test]
    fn handle_doc_list_for_join_opens_palette() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::DocList {
                documents: vec!["file1.org".to_string()],
                for_join: true,
            },
        );
        assert!(editor.command_palette.is_some());
        let palette = editor.command_palette.as_ref().unwrap();
        assert_eq!(palette.purpose, mae_core::PalettePurpose::CollabJoin);
        assert!(palette.entries.iter().any(|e| e.name == "file1.org"));
    }

    #[test]
    fn handle_status_report_creates_buffer() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::StatusReport {
                lines: vec!["line1".to_string(), "line2".to_string()],
            },
        );
        let idx = editor.find_buffer_by_name("*Collab Status*");
        assert!(idx.is_some());
    }

    #[test]
    fn handle_doctor_report_creates_buffer() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::DoctorReport {
                lines: vec!["ok".to_string()],
            },
        );
        let idx = editor.find_buffer_by_name("*Collab Doctor*");
        assert!(idx.is_some());
    }

    #[test]
    fn status_lines_connected() {
        let lines = build_status_lines("127.0.0.1:9473", true, &["main.rs".to_string()]);
        assert!(lines.iter().any(|l| l.contains("Connected")));
        assert!(lines.iter().any(|l| l.contains("main.rs")));
    }

    #[test]
    fn doctor_lines_disconnected() {
        let lines = build_doctor_lines("127.0.0.1:9473", false);
        assert!(lines.iter().any(|l| l.contains("\u{2717}")));
        assert!(lines.iter().any(|l| l.contains("Troubleshooting")));
    }

    #[test]
    fn doctor_lines_include_join_and_list() {
        let lines = build_doctor_lines("127.0.0.1:9473", false);
        assert!(lines.iter().any(|l| l.contains("SPC C l")));
        assert!(lines.iter().any(|l| l.contains("SPC C j")));
    }

    #[tokio::test]
    async fn handle_incoming_sync_update_notification_serde_format() {
        // Test the actual serde format: #[serde(tag = "type", content = "data")]
        let (tx, mut rx) = mpsc::channel(8);
        let mut pending = std::collections::HashMap::new();
        let mut shared = Vec::new();

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/sync_update",
            "params": {
                "seq": 1,
                "event": {
                    "type": "sync_update",
                    "data": {
                        "buffer_name": "test.rs",
                        "update_base64": "AQIDBA==",
                        "wal_seq": 0
                    }
                }
            }
        });
        handle_incoming_message(&msg.to_string(), &tx, &mut pending, &mut shared).await;
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::RemoteUpdate { doc_id, .. } => {
                assert_eq!(doc_id, "test.rs");
            }
            other => panic!("expected RemoteUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_incoming_sync_update_notification_legacy_format() {
        // Test backward compat with the old "sync_update" key format.
        let (tx, mut rx) = mpsc::channel(8);
        let mut pending = std::collections::HashMap::new();
        let mut shared = Vec::new();

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/sync_update",
            "params": {
                "seq": 1,
                "event": {
                    "sync_update": {
                        "buffer_name": "legacy.rs",
                        "update_base64": "AQIDBA==",
                        "wal_seq": 0
                    }
                }
            }
        });
        handle_incoming_message(&msg.to_string(), &tx, &mut pending, &mut shared).await;
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::RemoteUpdate { doc_id, .. } => {
                assert_eq!(doc_id, "legacy.rs");
            }
            other => panic!("expected RemoteUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_response_list_docs() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "documents": ["a.rs", "b.org"]
            }
        });
        handle_response(
            &val,
            PendingResponseKind::ListDocs { for_join: true },
            &tx,
            &mut shared,
        )
        .await;
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::DocList {
                documents,
                for_join,
            } => {
                assert!(for_join);
                assert_eq!(documents, vec!["a.rs", "b.org"]);
            }
            other => panic!("expected DocList, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_response_share_buffer() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "doc": "test.rs", "wal_seq": 1 }
        });
        handle_response(
            &val,
            PendingResponseKind::ShareBuffer {
                doc_id: "test.rs".to_string(),
            },
            &tx,
            &mut shared,
        )
        .await;
        assert!(shared.contains(&"test.rs".to_string()));
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, CollabEvent::BufferShared { doc_id } if doc_id == "test.rs"));
    }

    // -----------------------------------------------------------------------
    // Bug 2 regression: join must set language AND invalidate syntax cache
    // -----------------------------------------------------------------------

    #[test]
    fn buffer_joined_sets_language_and_invalidates_syntax() {
        let mut editor = Editor::new();

        // Create a sync doc with org content, then encode its state bytes.
        let org_content = "#+TITLE: Test\n\n- bullet one\n- bullet two\n";
        let sync = mae_sync::text::TextSync::with_client_id(org_content, 1);
        let state_bytes = sync.encode_state();

        // Feed a BufferJoined event with an org doc_id.
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "daily.org".to_string(),
                state_bytes,
            },
        );

        let idx = editor
            .find_buffer_by_name("daily.org")
            .expect("joined buffer should exist");

        // Language should be detected as Org.
        let lang = editor.syntax.language_of(idx);
        assert_eq!(
            lang,
            Some(mae_core::syntax::Language::Org),
            "joined .org buffer should have Org language set"
        );

        // The syntax cache should be invalidated (no stale spans/tree).
        assert!(
            !editor
                .syntax
                .has_cached_spans(idx, editor.buffers[idx].generation),
            "syntax cache should be invalidated after join (no stale spans)"
        );

        // Buffer content should match the shared org content.
        assert!(editor.buffers[idx].text().contains("bullet one"));
    }

    #[test]
    fn buffer_joined_non_org_gets_no_language() {
        let mut editor = Editor::new();

        let content = "just plain text\n";
        let sync = mae_sync::text::TextSync::with_client_id(content, 1);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "notes.txt".to_string(),
                state_bytes,
            },
        );

        let idx = editor
            .find_buffer_by_name("notes.txt")
            .expect("joined buffer should exist");

        // .txt files don't have a tree-sitter grammar, so no language set.
        assert_eq!(editor.syntax.language_of(idx), None);
    }

    // -----------------------------------------------------------------------
    // Bug 1 regression: unbiased select ensures server messages are processed
    // -----------------------------------------------------------------------
    // NOTE: The actual `run_collab_task` loop requires a real TCP connection,
    // so we can't unit-test it directly. Instead we verify the architectural
    // property: `handle_incoming_message` correctly processes a notification
    // even when called after a burst of commands. This test ensures the
    // message-handling path itself works; the `biased` removal ensures it
    // actually gets called.

    #[tokio::test]
    async fn server_notification_processed_after_command_burst() {
        let (tx, mut rx) = mpsc::channel(32);
        let mut pending = std::collections::HashMap::new();
        let mut shared = Vec::new();

        // Simulate N sync_update notifications arriving in quick succession
        // (as would happen when they pile up during biased starvation).
        for i in 0..5 {
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/sync_update",
                "params": {
                    "seq": i,
                    "event": {
                        "type": "sync_update",
                        "data": {
                            "buffer_name": format!("file{}.rs", i),
                            "update_base64": "AQIDBA==",
                            "wal_seq": i
                        }
                    }
                }
            });
            handle_incoming_message(&msg.to_string(), &tx, &mut pending, &mut shared).await;
        }

        // All 5 should have produced RemoteUpdate events.
        let mut received = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let CollabEvent::RemoteUpdate { doc_id, .. } = event {
                received.push(doc_id);
            }
        }
        assert_eq!(
            received.len(),
            5,
            "all queued server notifications must be processed; got {:?}",
            received
        );
    }
}
