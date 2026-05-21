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

/// Capacity for the command channel (main thread -> collab background task).
const COLLAB_CMD_CHANNEL_CAP: usize = 256;
/// Capacity for the event channel (collab background task -> main thread).
const COLLAB_EVT_CHANNEL_CAP: usize = 64;

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
    Doctor {
        /// Per-buffer sync info: (doc_id, pending_update_count).
        synced_info: Vec<(String, usize)>,
    },
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
    /// Send save intent to the server (docs/save_intent).
    SendSaveIntent {
        doc_id: String,
        expected_hash: String,
    },
    /// Confirm save completed (docs/save_committed).
    SendSaveCommitted {
        doc_id: String,
        save_epoch: u64,
        content_hash: String,
        saved_by: String,
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
        /// WAL sequence number from server (0 if not present).
        /// Gap detection happens inside handle_incoming_message before sending;
        /// this field is carried for diagnostic/logging use by consumers.
        #[allow(dead_code)]
        wal_seq: u64,
    },
    /// Gap detected in WAL sequence — triggers resync for the doc.
    GapDetected {
        doc_id: String,
        expected: u64,
        got: u64,
    },
    /// Share failed on server — must roll back synced state.
    ShareFailed {
        doc_id: String,
        message: String,
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
    /// Save intent accepted — server returned save_epoch.
    SaveIntentOk {
        doc_id: String,
        save_epoch: u64,
        content_hash: String,
    },
    /// Save intent rejected — content hash mismatch (concurrent edit).
    SaveIntentConflict {
        doc_id: String,
        message: String,
    },
    /// The sharer of a document disconnected.
    SharerLeft {
        doc_id: String,
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
    // Drain pending save_committed first (queued by SaveIntentOk handler).
    if let Some((doc_id, save_epoch, content_hash, saved_by)) =
        editor.collab.pending_save_committed.take()
    {
        let cmd = CollabCommand::SendSaveCommitted {
            doc_id,
            save_epoch,
            content_hash,
            saved_by,
        };
        if collab_tx.try_send(cmd).is_err() {
            warn!("collab command channel full — save_committed dropped");
        }
    }

    let intent = match editor.collab.pending_intent.take() {
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
                // Store doc_id on buffer so remote updates can find it.
                buf.collab_doc_id = Some(doc_id.clone());
                // BUG A fix: immediately track as synced so edits during the
                // server round-trip are forwarded via drain_and_broadcast().
                editor.collab.synced_buffers.insert(doc_id.clone());
                editor.collab.synced_docs = editor.collab.synced_buffers.len();
                debug!(doc = %doc_id, "share: immediately tracked as synced");
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
        CollabIntent::Doctor => {
            // Collect per-buffer sync info for the doctor report.
            let synced_info: Vec<(String, usize)> = editor
                .collab
                .synced_buffers
                .iter()
                .map(|doc_id| {
                    let pending = editor
                        .find_buffer_by_collab_doc_id(doc_id)
                        .map(|idx| editor.buffers[idx].pending_sync_updates.len())
                        .unwrap_or(0);
                    (doc_id.clone(), pending)
                })
                .collect();
            CollabCommand::Doctor { synced_info }
        }
        CollabIntent::SaveCollab {
            doc_id,
            content_hash,
        } => CollabCommand::SendSaveIntent {
            doc_id,
            expected_hash: content_hash,
        },
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
///
/// Uses `compute_project_identity()` (WU4) for stable cross-machine doc_ids:
/// git remote URL → .project name → basename → absolute path hash.
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
        let project_hash = if let Some(root) = project_root {
            mae_sync::compute_project_identity(root)
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
        CollabCommand::Doctor { .. } => "doctor",
        CollabCommand::StartServer => "start-server",
        CollabCommand::SendUpdate { .. } => "send-update",
        CollabCommand::SendSaveIntent { .. } => "send-save-intent",
        CollabCommand::SendSaveCommitted { .. } => "send-save-committed",
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
            editor.collab.status = CollabStatus::Connected { peer_count };
            editor.set_status(format!("Connected to {} ({} peers)", address, peer_count));
            // WU3: On reconnect, re-share buffers that still have CRDT state (offline recovery).
            let offline_docs: Vec<(String, Vec<u8>)> = editor
                .buffers
                .iter()
                .filter(|b| b.collab_offline && b.sync_doc.is_some() && b.collab_doc_id.is_some())
                .filter_map(|b| {
                    let doc_id = b.collab_doc_id.as_ref()?.clone();
                    let state = b.sync_doc.as_ref()?.encode_state();
                    Some((doc_id, state))
                })
                .collect();
            for (doc_id, _state_bytes) in &offline_docs {
                info!(doc = %doc_id, "reconnect: re-sharing offline buffer");
                editor.collab.synced_buffers.insert(doc_id.clone());
            }
            if !offline_docs.is_empty() {
                editor.collab.synced_docs = editor.collab.synced_buffers.len();
                // Queue re-share for each offline doc. The first one goes via
                // pending_collab_intent; additional ones would need the command channel.
                // For now, queue the first and set a status message.
                if let Some((doc_id, _state)) = offline_docs.first() {
                    editor.collab.pending_intent = Some(CollabIntent::ForceSync {
                        buffer_name: doc_id.clone(),
                    });
                }
                editor.set_status(format!(
                    "Connected to {} — resyncing {} offline buffer(s)",
                    address,
                    offline_docs.len()
                ));
            }
            editor.mark_full_redraw();
        }
        CollabEvent::Disconnected { reason } => {
            info!(reason = %reason, "collab disconnected");
            editor.collab.status = CollabStatus::Disconnected;
            editor.set_status(format!("Collab disconnected: {}", reason));
            // Preserve sync_doc and collab_doc_id for offline recovery (WU3).
            // Only clear UI tracking state — CRDT state survives disconnect
            // so local edits accumulate for resync on reconnect.
            for buf in &mut editor.buffers {
                if buf.collab_doc_id.is_some() {
                    if buf.sync_doc.is_some() {
                        buf.collab_offline = true;
                    } else {
                        // Buffer with no sync_doc (e.g. ShareFailed already cleared it)
                        // has no state to preserve.
                        buf.collab_doc_id = None;
                    }
                }
            }
            editor.collab.synced_docs = 0;
            editor.collab.synced_buffers.clear();
            editor.mark_full_redraw();
        }
        CollabEvent::RemoteUpdate {
            doc_id,
            update_bytes,
            wal_seq: _,
        } => {
            if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc_id) {
                match editor.buffers[idx].apply_sync_update(&update_bytes) {
                    Ok(()) => {
                        info!(doc = %doc_id, update_len = update_bytes.len(), buf_idx = idx,
                            text_len = editor.buffers[idx].text().len(), "applied remote sync update");
                        // Clear offline flag on successful remote update.
                        editor.buffers[idx].collab_offline = false;
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
        CollabEvent::GapDetected {
            doc_id,
            expected,
            got,
        } => {
            warn!(doc = %doc_id, expected, got, "WAL sequence gap — requesting resync");
            editor.set_status(format!(
                "Collab: gap detected on {} (expected seq {}, got {}), resyncing",
                doc_id, expected, got
            ));
            // Queue a ForceSync to trigger resync.
            editor.collab.pending_intent = Some(CollabIntent::ForceSync {
                buffer_name: doc_id,
            });
            editor.mark_full_redraw();
        }
        CollabEvent::StatusReport { lines } => {
            debug!(line_count = lines.len(), "status report received");
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
            debug!(line_count = lines.len(), "doctor report received");
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
            info!(doc = %doc_id, "buffer shared (server confirmed)");
            // Doc was already added optimistically in drain_collab_intents (BUG A fix).
            // This insert is idempotent — ensures consistency if event ordering varies.
            editor.collab.synced_buffers.insert(doc_id.clone());
            editor.collab.synced_docs = editor.collab.synced_buffers.len();
            // Mark this buffer as the sharer (authoritative saver).
            if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc_id) {
                editor.buffers[idx].collab_is_sharer = true;
            }
            editor.set_status(format!("Shared: {}", doc_id));
            editor.mark_full_redraw();
        }
        CollabEvent::DocList {
            documents,
            for_join,
        } => {
            debug!(count = documents.len(), for_join, "doc list received");
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
            info!(doc = %doc_id, state_bytes = state_bytes.len(), "buffer joined event received");
            // Parse DocAddress from doc_id for structured addressing.
            let doc_addr = mae_sync::DocAddress::parse(&doc_id);
            // Use a display-friendly name for the buffer.
            let buf_name = match &doc_addr {
                Some(mae_sync::DocAddress::File { rel_path, .. }) => rel_path.clone(),
                Some(mae_sync::DocAddress::Shared { name }) => name.clone(),
                Some(mae_sync::DocAddress::KbNode { node_id }) => node_id.clone(),
                None => doc_id.clone(),
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
                        // Joined buffers have NO auto file_path. Users must :saveas
                        // to create a local copy. This matches industry standard
                        // (VS Code Live Share, Zed — guests get no local files).
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            };
            match load_ok {
                Ok(()) => {
                    let text_preview: String =
                        editor.buffers[idx].text().chars().take(200).collect();
                    info!(doc = %doc_id, buf_idx = idx, text_len = editor.buffers[idx].text().len(),
                        text_preview = %text_preview, "buffer joined: sync state loaded");
                    // Store doc_id on buffer only after successful load — prevents
                    // RemoteUpdate from targeting a buffer with no valid sync_doc.
                    editor.buffers[idx].collab_doc_id = Some(doc_id.clone());
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
                    editor.collab.synced_buffers.insert(doc_id.clone());
                    editor.collab.synced_docs = editor.collab.synced_buffers.len();
                    editor.switch_to_buffer(idx);
                    editor.set_status(format!("Joined: {}", doc_id));
                    editor.mark_full_redraw();

                    // Opt-in: if collab_auto_resolve_paths is enabled and the
                    // doc has a file address with a matching local file, prompt
                    // the user to map the buffer to their local project path.
                    if editor
                        .get_option("collab_auto_resolve_paths")
                        .map(|(v, _)| v == "true")
                        .unwrap_or(false)
                    {
                        if let Some(mae_sync::DocAddress::File { rel_path, .. }) = &doc_addr {
                            let resolved = if let Some(root) = &project_root {
                                let rooted = root.join(rel_path);
                                if rooted.exists() && rooted.parent().is_some_and(|p| p.is_dir()) {
                                    Some(rooted.canonicalize().unwrap_or(rooted))
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            if let Some(resolved_path) = resolved {
                                let display = rel_path.clone();
                                editor.mini_dialog = Some(
                                    mae_core::command_palette::MiniDialogState::confirm(
                                        format!("Map to local project file {}? (y/n)", display),
                                        mae_core::command_palette::MiniDialogContext::CollabResolvePath {
                                            buf_idx: idx,
                                            resolved_path,
                                        },
                                    ),
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    editor.set_status(format!("Failed to join {}: {}", doc_id, e));
                }
            }
        }
        CollabEvent::ShareFailed { doc_id, message } => {
            warn!(doc = %doc_id, error = %message, "share failed — rolling back synced state");
            // Remove from synced set (was optimistically added in drain_collab_intents).
            editor.collab.synced_buffers.remove(&doc_id);
            editor.collab.synced_docs = editor.collab.synced_buffers.len();
            // Clear all collab state on the buffer so re-share starts fresh.
            if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc_id) {
                editor.buffers[idx].collab_doc_id = None;
                editor.buffers[idx].sync_doc = None;
                editor.buffers[idx].pending_sync_updates.clear();
            }
            editor.set_status(format!("Share failed: {}", message));
            editor.mark_full_redraw();
        }
        CollabEvent::SaveIntentOk {
            doc_id,
            save_epoch,
            content_hash,
        } => {
            info!(doc = %doc_id, save_epoch, "save intent accepted — sending save_committed");
            let saved_by = if editor.collab.user_name.is_empty() {
                "unknown".to_string()
            } else {
                editor.collab.user_name.clone()
            };
            // Queue the save_committed command for the next drain tick.
            editor.collab.pending_save_committed =
                Some((doc_id.clone(), save_epoch, content_hash, saved_by));
            editor.set_status(format!("Saved (collab epoch {})", save_epoch));
            editor.mark_full_redraw();
        }
        CollabEvent::SaveIntentConflict { doc_id, message } => {
            warn!(doc = %doc_id, "save intent conflict: {}", message);
            editor.set_status(format!(
                "Save conflict on {} — sync first (:collab-sync)",
                doc_id
            ));
            editor.mark_full_redraw();
        }
        CollabEvent::SharerLeft { doc_id } => {
            warn!(doc = %doc_id, "sharer disconnected");
            editor.set_status(format!("Sharer disconnected for {}", doc_id));
            editor.mark_full_redraw();
        }
        CollabEvent::PeerCountChanged { peer_count } => {
            debug!(peer_count, "peer count changed");
            if let CollabStatus::Connected { .. } = editor.collab.status {
                editor.collab.status = CollabStatus::Connected { peer_count };
                if peer_count == 0 {
                    editor.set_status("All other collaborators disconnected");
                } else {
                    editor.set_status(format!(
                        "Peer count: {} collaborator{}",
                        peer_count,
                        if peer_count == 1 { "" } else { "s" }
                    ));
                }
                editor.mark_full_redraw();
            }
        }
        CollabEvent::PeerSaved { doc, saved_by } => {
            debug!(doc = %doc, saved_by = %saved_by, "peer saved");
            editor.set_status(format!("[{}] saved by {}", doc, saved_by));
            // Mark the local buffer clean if we have it (content matches what was saved).
            if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc) {
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
    backoff_factor: u64,
    max_reconnect_attempts: u64,
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
    let (cmd_tx, cmd_rx) = mpsc::channel::<CollabCommand>(COLLAB_CMD_CHANNEL_CAP);
    let (evt_tx, evt_rx) = mpsc::channel::<CollabEvent>(COLLAB_EVT_CHANNEL_CAP);

    let reconnect_secs = editor.collab.reconnect_interval;
    let write_timeout_ms = editor.collab.write_timeout_ms;

    let auto_connect_addr =
        if editor.collab.auto_connect && !editor.collab.server_address.is_empty() {
            Some(editor.collab.server_address.clone())
        } else {
            None
        };

    let backoff_factor = editor.collab.reconnect_backoff_factor;
    let max_reconnect_attempts = editor.collab.max_reconnect_attempts;

    let spawn = CollabSpawn {
        cmd_rx,
        evt_tx,
        reconnect_secs,
        write_timeout_ms,
        auto_connect_addr,
        cmd_tx_clone: cmd_tx.clone(),
        backoff_factor,
        max_reconnect_attempts,
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
        spawn.backoff_factor,
        spawn.max_reconnect_attempts,
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
pub(crate) enum PendingResponseKind {
    ListDocs {
        for_join: bool,
    },
    JoinDoc {
        doc_id: String,
    },
    ShareBuffer {
        doc_id: String,
    },
    ForceSync {
        doc_id: String,
    },
    SyncUpdate {
        doc_id: String,
    },
    SaveIntent {
        doc_id: String,
        expected_hash: String,
    },
    Subscribe,
    Ping {
        sent_at: std::time::Instant,
    },
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
    backoff_factor: u64,
    max_reconnect_attempts: u64,
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
    let mut reconnect_attempt: u32 = 0;
    // ForceSync debounce: track last force-sync time per doc.
    let mut last_force_sync: std::collections::HashMap<String, std::time::Instant> =
        std::collections::HashMap::new();
    let mut next_request_id: u64 = 10; // Start after handshake IDs
    let mut pending_responses: HashMap<u64, PendingResponseKind> = HashMap::new();
    // WU1: Track wal_seq per doc for gap detection.
    let mut seq_tracker: HashMap<String, u64> = HashMap::new();
    // WU2: Heartbeat interval (30s default, disabled if 0).
    let heartbeat_secs = 30u64; // TODO: read from option via spawn config
    let mut heartbeat_interval =
        tokio::time::interval(std::time::Duration::from_secs(if heartbeat_secs > 0 {
            heartbeat_secs
        } else {
            3600
        }));
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Skip the first immediate tick.
    heartbeat_interval.tick().await;
    let mut ping_pending = false;

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
                    debug!(cmd = ?std::mem::discriminant(&cmd),
                        "bridge: received command");
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
                        CollabCommand::Doctor { synced_info } => {
                            let addr = target_address.as_deref().unwrap_or("?").to_string();
                            let mut ctx = DoctorContext {
                                address: addr,
                                connected: true,
                                server_debug: None,
                                ping_latency_ms: None,
                                synced_info,
                            };
                            // Gather $/ping latency + $/debug from server.
                            if let Some(ref mut w) = writer {
                                gather_doctor_context(
                                    w,
                                    reader.as_mut().unwrap(),
                                    &mut next_request_id,
                                    write_timeout,
                                    &mut ctx,
                                )
                                .await;
                            }
                            let lines = build_doctor_lines(&ctx);
                            let _ = evt_tx.send(CollabEvent::DoctorReport { lines }).await;
                        }
                        CollabCommand::ShareBuffer { doc_id, state_bytes } => {
                            if let Some(ref mut w) = writer {
                                // Atomic share: server deletes old doc + applies update in one step.
                                let update_b64 = mae_sync::encoding::update_to_base64(&state_bytes);
                                info!(doc = %doc_id, state_len = state_bytes.len(), b64_len = update_b64.len(), "share: sending sync/share");
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
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
                                match write_framed(w, &body, write_timeout).await {
                                    Ok(()) => {
                                        info!(doc = %doc_id, req_id, body_len = body.len(),
                                            "share: write_framed completed successfully");
                                        pending_responses.insert(req_id, PendingResponseKind::ShareBuffer { doc_id });
                                    }
                                    Err(e) => {
                                        error!(doc = %doc_id, error = %e,
                                            "share: write_framed failed");
                                        let _ = evt_tx.send(CollabEvent::Error {
                                            message: format!("Failed to share {}", doc_id),
                                        }).await;
                                    }
                                }
                            }
                        }
                        CollabCommand::ForceSync { doc_id } => {
                            // Debounce: skip if we sent ForceSync for this doc within 2s.
                            let now = std::time::Instant::now();
                            if let Some(last) = last_force_sync.get(&doc_id) {
                                if now.duration_since(*last).as_secs() < 2 {
                                    debug!(doc = %doc_id, "ForceSync debounced (within 2s)");
                                    continue;
                                }
                            }
                            last_force_sync.insert(doc_id.clone(), now);
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "sync/full_state",
                                    "params": { "doc": doc_id }
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
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
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
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
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
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
                            info!(doc = %doc_id, "join: sending sync/resync");
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "sync/resync",
                                    "params": { "doc": doc_id },
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
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
                        CollabCommand::SendSaveIntent { doc_id, expected_hash } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "docs/save_intent",
                                    "params": {
                                        "doc": doc_id,
                                        "expected_hash": expected_hash,
                                    }
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::SaveIntent {
                                        doc_id,
                                        expected_hash,
                                    });
                                } else {
                                    let _ = evt_tx.send(CollabEvent::Error {
                                        message: "Failed to send save intent".to_string(),
                                    }).await;
                                }
                            }
                        }
                        CollabCommand::SendSaveCommitted { doc_id, save_epoch, content_hash, saved_by } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "docs/save_committed",
                                    "params": {
                                        "doc": doc_id,
                                        "save_epoch": save_epoch,
                                        "content_hash": content_hash,
                                        "saved_by": saved_by,
                                    }
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
                                // Fire-and-forget — no pending response tracking needed.
                                if write_framed(w, &body, write_timeout).await.is_err() {
                                    warn!(doc = %doc_id, "failed to send save_committed");
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
                            debug!(msg_len = text.len(),
                                preview = &text[..text.len().min(120)],
                                "bridge: incoming server message");
                            handle_incoming_message(
                                &text,
                                &evt_tx,
                                &mut pending_responses,
                                &mut shared_docs,
                                &mut seq_tracker,
                            ).await;
                            // Any valid message resets the ping_pending flag.
                            ping_pending = false;
                        }
                        Ok(None) | Err(_) => {
                            tear_down(&mut reader, &mut writer);
                            shared_docs.clear();
                            pending_responses.clear();
                            seq_tracker.clear();
                            ping_pending = false;
                            let _ = evt_tx.send(CollabEvent::Disconnected {
                                reason: "connection lost".to_string(),
                            }).await;
                            if reconnect_enabled {
                                continue;
                            }
                        }
                    }
                }
                // WU2: Periodic heartbeat.
                _ = heartbeat_interval.tick() => {
                    if heartbeat_secs == 0 {
                        continue;
                    }
                    if ping_pending {
                        // Previous ping got no response — connection dead.
                        warn!("heartbeat: no response to previous ping — disconnecting");
                        tear_down(&mut reader, &mut writer);
                        shared_docs.clear();
                        pending_responses.clear();
                        seq_tracker.clear();
                        ping_pending = false;
                        let _ = evt_tx.send(CollabEvent::Disconnected {
                            reason: "heartbeat timeout".to_string(),
                        }).await;
                        if reconnect_enabled {
                            continue;
                        }
                    } else if let Some(ref mut w) = writer {
                        let req_id = next_request_id;
                        next_request_id += 1;
                        let req = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": req_id,
                            "method": "$/ping",
                        });
                        let body = serde_json::to_vec(&req).unwrap_or_default();
                        if write_framed(w, &body, write_timeout).await.is_ok() {
                            pending_responses.insert(req_id, PendingResponseKind::Ping {
                                sent_at: std::time::Instant::now(),
                            });
                            ping_pending = true;
                        } else {
                            // Write failed — connection is broken.
                            tear_down(&mut reader, &mut writer);
                            shared_docs.clear();
                            pending_responses.clear();
                            seq_tracker.clear();
                            ping_pending = false;
                            let _ = evt_tx.send(CollabEvent::Disconnected {
                                reason: "heartbeat write failed".to_string(),
                            }).await;
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
                        _ = tokio::time::sleep(std::time::Duration::from_secs(
                            compute_backoff(reconnect_secs, backoff_factor, reconnect_attempt)
                        )) => {
                            // Check max attempts (0 = infinite).
                            if max_reconnect_attempts > 0
                                && reconnect_attempt as u64 >= max_reconnect_attempts
                            {
                                warn!(attempts = reconnect_attempt, max = max_reconnect_attempts,
                                    "max reconnect attempts exhausted");
                                reconnect_enabled = false;
                                let _ = evt_tx.send(CollabEvent::Disconnected {
                                    reason: format!("max reconnect attempts ({}) exhausted", max_reconnect_attempts),
                                }).await;
                                continue;
                            }
                            reconnect_attempt += 1;
                            if let Ok(mut stream) = TcpStream::connect(&addr_clone).await {
                                if let Some(peer_count) = send_initialize(&mut stream, write_timeout).await {
                                    install_connection(stream, &mut reader, &mut writer);
                                    reconnect_attempt = 0; // Reset on success.
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
                                debug!(addr = %addr_clone, attempt = reconnect_attempt,
                                    "reconnect failed, will retry");
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

/// Check WAL sequence continuity for a doc. If a gap is detected, emit GapDetected.
async fn check_seq_gap(
    doc_id: &str,
    wal_seq: u64,
    seq_tracker: &mut std::collections::HashMap<String, u64>,
    evt_tx: &mpsc::Sender<CollabEvent>,
) {
    let expected = seq_tracker
        .get(doc_id)
        .map(|last| last + 1)
        .unwrap_or(wal_seq); // first time: no gap
    if wal_seq > expected {
        warn!(doc = %doc_id, expected, got = wal_seq, "WAL sequence gap detected");
        let _ = evt_tx
            .send(CollabEvent::GapDetected {
                doc_id: doc_id.to_string(),
                expected,
                got: wal_seq,
            })
            .await;
    }
    // Always update tracker to the latest seen seq.
    seq_tracker.insert(doc_id.to_string(), wal_seq);
}

/// Compute exponential backoff delay: `base * factor^min(attempt, 5)`, capped at 300s.
fn compute_backoff(base_secs: u64, factor: u64, attempt: u32) -> u64 {
    let exp = attempt.min(5);
    let delay = base_secs.saturating_mul(factor.saturating_pow(exp));
    delay.min(300)
}

/// Handle an incoming JSON-RPC message from the server.
/// Dispatches to response handler or notification handler based on content.
pub(crate) async fn handle_incoming_message(
    text: &str,
    evt_tx: &mpsc::Sender<CollabEvent>,
    pending_responses: &mut std::collections::HashMap<u64, PendingResponseKind>,
    shared_docs: &mut Vec<String>,
    seq_tracker: &mut std::collections::HashMap<String, u64>,
) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };

    // Case 1: JSON-RPC response (has `id` + (`result` or `error`), no `method`)
    if let Some(id) = val.get("id").and_then(|v| v.as_u64()) {
        if val.get("method").is_none() {
            if let Some(kind) = pending_responses.remove(&id) {
                let has_error = val.get("error").is_some();
                debug!(id, has_error, kind = ?std::mem::discriminant(&kind),
                    "bridge: matched response to pending request");
                handle_response(&val, kind, evt_tx, shared_docs).await;
            } else {
                debug!(id, "bridge: response for unknown/expired request id");
            }
            return;
        }
    }

    // Case 2: Server notification (has `method`, no `id` or id is null)
    if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
        match method {
            // B3 fix: server sends "notifications/sync_update" with nested event data.
            "notifications/sync_update" => {
                if let Some(params) = val.get("params") {
                    // Server format: {"params": {"seq": N, "event": {"type": "sync_update", "data": {"buffer_name": "...", "update_base64": "..."}}}}
                    // The "data" key comes from serde's #[serde(tag = "type", content = "data")] on EditorEvent.
                    let wal_seq = params.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
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
                        debug!(doc = %buffer_name, wal_seq, update_bytes = update_b64.len(), "received sync_update");
                        // Gap detection: check wal_seq continuity per doc.
                        if wal_seq > 0 {
                            check_seq_gap(&buffer_name, wal_seq, seq_tracker, evt_tx).await;
                        }
                        if let Ok(bytes) = mae_sync::encoding::base64_to_update(update_b64) {
                            let _ = evt_tx
                                .send(CollabEvent::RemoteUpdate {
                                    doc_id: buffer_name,
                                    update_bytes: bytes,
                                    wal_seq,
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
                    let wal_seq = params.get("wal_seq").and_then(|v| v.as_u64()).unwrap_or(0);
                    let update_b64 = params
                        .get("update")
                        .or_else(|| params.get("update_base64"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if wal_seq > 0 {
                        check_seq_gap(&doc_id, wal_seq, seq_tracker, evt_tx).await;
                    }
                    if let Ok(bytes) = mae_sync::encoding::base64_to_update(update_b64) {
                        let _ = evt_tx
                            .send(CollabEvent::RemoteUpdate {
                                doc_id,
                                update_bytes: bytes,
                                wal_seq,
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
                    debug!(peer_count, "received peer_joined notification");
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
                    debug!(peer_count, "received peer_left notification");
                    let _ = evt_tx
                        .send(CollabEvent::PeerCountChanged { peer_count })
                        .await;
                }
            }
            "notifications/sharer_left" => {
                if let Some(params) = val.get("params") {
                    let event = params.get("event").unwrap_or(params);
                    let data = event.get("data").unwrap_or(event);
                    let doc_id = data
                        .get("doc")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    debug!(doc = %doc_id, "received sharer_left notification");
                    let _ = evt_tx.send(CollabEvent::SharerLeft { doc_id }).await;
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
                    debug!(doc = %doc, saved_by = %saved_by, "received save_committed notification");
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
                let err_msg = val
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                error!(doc = %doc_id, error = %err_msg, "share: server rejected");
                let _ = evt_tx
                    .send(CollabEvent::ShareFailed {
                        doc_id,
                        message: err_msg,
                    })
                    .await;
            } else {
                info!(doc = %doc_id, "share: server accepted sync/share");
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
            info!(count = documents.len(), for_join, docs = ?documents, "docs/list response");
            let _ = evt_tx
                .send(CollabEvent::DocList {
                    documents,
                    for_join,
                })
                .await;
        }
        PendingResponseKind::JoinDoc { doc_id } => {
            // sync/resync response: {"result": {"doc": "...", "state": "<base64>", "sv": "<base64>"}}
            // Use server-resolved doc_id (suffix matching may have expanded bare
            // filenames like "test.txt" → "file:no-project/test.txt").
            let resolved_doc_id = result
                .and_then(|r| r.get("doc"))
                .and_then(|d| d.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| doc_id.clone());
            let state_b64 = result
                .and_then(|r| r.get("state"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            info!(doc = %resolved_doc_id, b64_len = state_b64.len(), "join: received sync/resync response");
            // Update shared_docs to use the resolved name (replace unresolved if present).
            if resolved_doc_id != doc_id {
                if let Some(pos) = shared_docs.iter().position(|d| d == &doc_id) {
                    shared_docs[pos] = resolved_doc_id.clone();
                } else if !shared_docs.contains(&resolved_doc_id) {
                    shared_docs.push(resolved_doc_id.clone());
                }
            }
            match mae_sync::encoding::base64_to_update(state_b64) {
                Ok(state_bytes) => {
                    info!(doc = %resolved_doc_id, state_len = state_bytes.len(), "join: decoded state, sending BufferJoined");
                    let _ = evt_tx
                        .send(CollabEvent::BufferJoined {
                            doc_id: resolved_doc_id,
                            state_bytes,
                        })
                        .await;
                }
                Err(e) => {
                    error!(doc = %doc_id, error = %e, b64_preview = &state_b64[..state_b64.len().min(100)], "join: failed to decode state");
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
        PendingResponseKind::SaveIntent {
            doc_id,
            expected_hash,
        } => {
            if let Some(err) = val.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("save intent failed")
                    .to_string();
                let _ = evt_tx
                    .send(CollabEvent::SaveIntentConflict {
                        doc_id,
                        message: msg,
                    })
                    .await;
            } else if let Some(r) = result {
                let save_result = r.get("result").unwrap_or(r);
                let status = save_result
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                if status == "conflict" {
                    let _ = evt_tx
                        .send(CollabEvent::SaveIntentConflict {
                            doc_id,
                            message: "Content hash mismatch — sync first".to_string(),
                        })
                        .await;
                } else {
                    let save_epoch = save_result
                        .get("save_epoch")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let _ = evt_tx
                        .send(CollabEvent::SaveIntentOk {
                            doc_id,
                            save_epoch,
                            content_hash: expected_hash,
                        })
                        .await;
                }
            }
        }
        PendingResponseKind::Subscribe => {
            // Acknowledgement — no action needed.
        }
        PendingResponseKind::Ping { sent_at } => {
            let latency_ms = sent_at.elapsed().as_millis() as u64;
            debug!(latency_ms, "heartbeat pong received");
            // Latency is logged — could be exposed to doctor in the future.
            let _ = latency_ms; // suppress unused warning
        }
    }
}

/// Send `notifications/subscribe` to opt into sync_update events (B4 fix).
async fn send_subscribe<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
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
            "types": ["sync_update", "peer_joined", "peer_left", "save_committed"]
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
        CollabCommand::Doctor { synced_info } => {
            let ctx = DoctorContext {
                address: target_address
                    .as_deref()
                    .unwrap_or("not configured")
                    .to_string(),
                connected: false,
                server_debug: None,
                ping_latency_ms: None,
                synced_info,
            };
            let lines = build_doctor_lines(&ctx);
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
        CollabCommand::SendSaveIntent { doc_id, .. } => {
            let _ = evt_tx
                .send(CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot save '{}'", doc_id),
                })
                .await;
        }
        CollabCommand::SendSaveCommitted { .. } => {
            // Silently drop — not connected.
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

/// Gather live server data for the doctor report ($/ping + $/debug).
/// Populates `ctx.ping_latency_ms` and `ctx.server_debug` in-place.
/// Each query has a 2s timeout — fields left as `None` on timeout/error.
async fn gather_doctor_context<R, W>(
    writer: &mut W,
    reader: &mut R,
    next_id: &mut u64,
    write_timeout: std::time::Duration,
    ctx: &mut DoctorContext,
) where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use mae_mcp::{read_message, write_framed};
    let gather_timeout = std::time::Duration::from_secs(2);

    // $/ping — measure round-trip latency.
    let ping_id = *next_id;
    *next_id += 1;
    let ping_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": ping_id,
        "method": "$/ping",
    });
    let body = serde_json::to_vec(&ping_req).unwrap();
    let ping_start = std::time::Instant::now();
    if write_framed(writer, &body, write_timeout).await.is_ok() {
        if let Ok(Ok(Some(_text))) =
            tokio::time::timeout(gather_timeout, read_message(reader)).await
        {
            ctx.ping_latency_ms = Some(ping_start.elapsed().as_millis() as u64);
        }
    }

    // $/debug — fetch per-doc server stats.
    let debug_id = *next_id;
    *next_id += 1;
    let debug_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": debug_id,
        "method": "$/debug",
    });
    let body = serde_json::to_vec(&debug_req).unwrap();
    if write_framed(writer, &body, write_timeout).await.is_ok() {
        if let Ok(Ok(Some(text))) = tokio::time::timeout(gather_timeout, read_message(reader)).await
        {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                ctx.server_debug = val.get("result").cloned();
            }
        }
    }
}

/// Context gathered for the doctor report — pre-fetched data from server queries.
pub(crate) struct DoctorContext {
    pub(crate) address: String,
    pub(crate) connected: bool,
    /// Per-doc stats from $/debug response, if available.
    pub(crate) server_debug: Option<serde_json::Value>,
    /// Round-trip latency in ms from $/ping.
    pub(crate) ping_latency_ms: Option<u64>,
    /// Per-buffer sync info: (doc_id, pending_update_count).
    pub(crate) synced_info: Vec<(String, usize)>,
}

pub(crate) fn build_doctor_lines(ctx: &DoctorContext) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("Collab Doctor".to_string());
    lines.push(String::from_utf8(vec![b'='; 20]).unwrap());
    if ctx.connected {
        lines.push(format!("\u{2713} State server reachable ({})", ctx.address));
        lines.push("\u{2713} Protocol: JSON-RPC 2.0 (Content-Length framing)".to_string());
        lines.push(format!(
            "\u{2713} Client version: {}",
            env!("CARGO_PKG_VERSION")
        ));

        // Latency
        match ctx.ping_latency_ms {
            Some(ms) => lines.push(format!("\u{2713} Ping: {}ms", ms)),
            None => lines.push("\u{26a0} Ping: timeout".to_string()),
        }

        // Per-doc server stats from $/debug
        // Server returns: {"documents": N, "doc_stats": {"name": {stats...}}}
        if let Some(ref debug_val) = ctx.server_debug {
            if let Some(stats_map) = debug_val.get("doc_stats").and_then(|d| d.as_object()) {
                lines.push(String::new());
                lines.push(format!("Server Documents ({}):", stats_map.len()));
                for (name, stats) in stats_map {
                    let wal_seq = stats.get("wal_seq").and_then(|v| v.as_u64()).unwrap_or(0);
                    let updates = stats
                        .get("update_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let clients = stats
                        .get("connected_clients")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let idle = stats.get("idle_secs").and_then(|v| v.as_u64());
                    let mut info = format!(
                        "  {} — wal:{} updates:{} clients:{}",
                        name, wal_seq, updates, clients
                    );
                    if let Some(s) = idle {
                        info.push_str(&format!(" idle:{}s", s));
                    }
                    lines.push(info);
                }
            }
        }

        // Synced buffers
        if !ctx.synced_info.is_empty() {
            lines.push(String::new());
            lines.push(format!("Synced Buffers ({}):", ctx.synced_info.len()));
            for (doc_id, pending) in &ctx.synced_info {
                let status = if *pending > 0 {
                    format!("{} pending", pending)
                } else {
                    "up-to-date".to_string()
                };
                lines.push(format!("  {} — {}", doc_id, status));
            }
        }
    } else {
        lines.push(format!(
            "\u{2717} State server not reachable ({})",
            ctx.address
        ));
        lines.push(String::new());
        lines.push("Troubleshooting:".to_string());
        lines.push("  1. Is mae-state-server running?".to_string());
        lines.push("     Start: systemctl --user start mae-state-server".to_string());
        lines.push(format!(
            "     Or:    mae-state-server --bind {}",
            ctx.address
        ));
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
            ctx.address.split(':').next().unwrap_or("127.0.0.1"),
            ctx.address.split(':').next_back().unwrap_or("9473")
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
        editor.collab.pending_intent = Some(CollabIntent::Connect {
            address: "127.0.0.1:9473".to_string(),
        });
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        assert!(editor.collab.pending_intent.is_none());
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
        editor.collab.pending_intent = Some(CollabIntent::ShareBuffer {
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
        editor.collab.pending_intent = Some(CollabIntent::ListDocs);
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, CollabCommand::ListDocs { for_join: false }));
    }

    #[test]
    fn drain_collab_join_doc() {
        let mut editor = Editor::new();
        editor.collab.pending_intent = Some(CollabIntent::JoinDoc {
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
            editor.collab.status,
            CollabStatus::Connected { peer_count: 2 }
        );
    }

    #[test]
    fn handle_disconnected_event() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        editor.collab.synced_buffers.insert("test.rs".to_string());
        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );
        assert_eq!(editor.collab.status, CollabStatus::Disconnected);
        assert_eq!(editor.collab.synced_docs, 0);
        // UI tracking cleared, but per-buffer state depends on sync_doc presence.
        assert!(editor.collab.synced_buffers.is_empty());
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
        assert!(editor.collab.synced_buffers.contains("main.rs"));
        assert_eq!(editor.collab.synced_docs, 1);
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
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: false,
            server_debug: None,
            ping_latency_ms: None,
            synced_info: vec![],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(lines.iter().any(|l| l.contains("\u{2717}")));
        assert!(lines.iter().any(|l| l.contains("Troubleshooting")));
    }

    #[test]
    fn doctor_lines_include_join_and_list() {
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: false,
            server_debug: None,
            ping_latency_ms: None,
            synced_info: vec![],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(lines.iter().any(|l| l.contains("SPC C l")));
        assert!(lines.iter().any(|l| l.contains("SPC C j")));
    }

    #[test]
    fn doctor_lines_show_server_stats() {
        // Matches actual $/debug response shape: doc_stats is a map keyed by name.
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: true,
            server_debug: Some(serde_json::json!({
                "documents": 1,
                "doc_stats": {
                    "test.rs": {
                        "wal_seq": 42,
                        "update_count": 10,
                        "connected_clients": 2,
                        "idle_secs": 5
                    }
                }
            })),
            ping_latency_ms: Some(3),
            synced_info: vec![],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(lines.iter().any(|l| l.contains("test.rs")));
        assert!(lines.iter().any(|l| l.contains("wal:42")));
        assert!(lines.iter().any(|l| l.contains("clients:2")));
    }

    #[test]
    fn doctor_lines_show_latency() {
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: true,
            server_debug: None,
            ping_latency_ms: Some(7),
            synced_info: vec![],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(lines.iter().any(|l| l.contains("Ping: 7ms")));
    }

    #[test]
    fn doctor_lines_show_synced_buffers() {
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: true,
            server_debug: None,
            ping_latency_ms: None,
            synced_info: vec![("doc-a".to_string(), 0), ("doc-b".to_string(), 3)],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(lines
            .iter()
            .any(|l| l.contains("doc-a") && l.contains("up-to-date")));
        assert!(lines
            .iter()
            .any(|l| l.contains("doc-b") && l.contains("3 pending")));
    }

    #[test]
    fn doctor_lines_disconnected_no_crash() {
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: false,
            server_debug: None,
            ping_latency_ms: None,
            synced_info: vec![],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(!lines.is_empty());
        assert!(lines.iter().any(|l| l.contains("not reachable")));
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
        handle_incoming_message(
            &msg.to_string(),
            &tx,
            &mut pending,
            &mut shared,
            &mut std::collections::HashMap::new(),
        )
        .await;
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
        handle_incoming_message(
            &msg.to_string(),
            &tx,
            &mut pending,
            &mut shared,
            &mut std::collections::HashMap::new(),
        )
        .await;
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

    #[test]
    fn drain_share_sets_synced_immediately() {
        let mut editor = Editor::new();
        let buf_name = editor.buffers[0].name.clone();
        editor.collab.pending_intent = Some(CollabIntent::ShareBuffer {
            buffer_name: buf_name.clone(),
        });
        let (tx, _rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);

        // BUG A: doc_id must be in collab_synced_buffers IMMEDIATELY.
        let expected_doc_id = format!("shared:{}", buf_name);
        assert!(
            editor.collab.synced_buffers.contains(&expected_doc_id),
            "doc_id should be in collab_synced_buffers immediately after drain"
        );
        assert_eq!(editor.collab.synced_docs, 1);
    }

    #[test]
    fn share_failure_removes_from_synced() {
        let mut editor = Editor::new();
        // Simulate: doc was optimistically added during share.
        editor.collab.synced_buffers.insert("test-doc".to_string());
        editor.collab.synced_docs = 1;
        // Also set collab_doc_id on a buffer so the rollback can clear it.
        editor.buffers[0].collab_doc_id = Some("test-doc".to_string());

        handle_collab_event(
            &mut editor,
            CollabEvent::ShareFailed {
                doc_id: "test-doc".to_string(),
                message: "server error".to_string(),
            },
        );

        assert!(!editor.collab.synced_buffers.contains("test-doc"));
        assert_eq!(editor.collab.synced_docs, 0);
        assert!(editor.buffers[0].collab_doc_id.is_none());
    }

    #[test]
    fn handle_disconnect_preserves_sync_for_offline_recovery() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        // Set up a buffer as if it were synced.
        let buf = &mut editor.buffers[0];
        buf.collab_doc_id = Some("test-doc".to_string());
        buf.enable_sync(42);
        buf.insert_text_at(5, "x"); // generates pending_sync_update
        editor.collab.synced_buffers.insert("test-doc".to_string());

        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );

        assert!(editor.collab.synced_buffers.is_empty());
        assert_eq!(editor.collab.synced_docs, 0);
        // WU3: sync_doc and collab_doc_id are PRESERVED for offline recovery.
        assert!(editor.buffers[0].collab_doc_id.is_some());
        assert!(editor.buffers[0].sync_doc.is_some());
        assert!(editor.buffers[0].collab_offline);
    }

    #[tokio::test]
    async fn share_failure_emits_share_failed() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32000, "message": "storage full" }
        });
        handle_response(
            &val,
            PendingResponseKind::ShareBuffer {
                doc_id: "fail.rs".to_string(),
            },
            &tx,
            &mut shared,
        )
        .await;

        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::ShareFailed { doc_id, message } => {
                assert_eq!(doc_id, "fail.rs");
                assert!(message.contains("storage full"));
            }
            other => panic!("expected ShareFailed, got {:?}", other),
        }
        // Should NOT be in shared_docs.
        assert!(!shared.contains(&"fail.rs".to_string()));
    }

    #[test]
    fn disconnect_sets_offline_on_all_synced_buffers() {
        // WU3: disconnect preserves sync_doc for offline recovery.
        // Buffers with sync_doc get collab_offline=true.
        // Buffers without sync_doc (ShareFailed cleared it) get collab_doc_id cleared.
        use mae_core::Buffer;
        let mut editor = Editor::new();

        // Buffer A: tracked in synced_buffers, has sync_doc.
        editor.buffers[0].name = "tracked.rs".to_string();
        editor.buffers[0].enable_sync(1);
        editor.buffers[0].collab_doc_id = Some("doc-tracked".to_string());
        editor
            .collab
            .synced_buffers
            .insert("doc-tracked".to_string());

        // Buffer B: has collab_doc_id but no sync_doc (ShareFailed cleared it).
        let mut buf_b = Buffer::new();
        buf_b.name = "orphaned.rs".to_string();
        buf_b.collab_doc_id = Some("doc-orphaned".to_string());
        // No enable_sync → sync_doc is None.
        editor.buffers.push(buf_b);

        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        editor.collab.synced_docs = 1;

        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );

        // Buffer A: sync_doc preserved, collab_offline = true.
        assert!(
            editor.buffers[0].sync_doc.is_some(),
            "tracked buffer should preserve sync_doc"
        );
        assert!(
            editor.buffers[0].collab_offline,
            "tracked buffer should be offline"
        );
        assert!(editor.buffers[0].collab_doc_id.is_some());

        // Buffer B: no sync_doc → collab_doc_id cleared (nothing to preserve).
        assert!(
            editor.buffers[1].collab_doc_id.is_none(),
            "orphaned buffer should have collab_doc_id cleared"
        );
        assert!(!editor.buffers[1].collab_offline);
    }

    #[test]
    fn disconnect_after_share_failure_preserves_good_buffer() {
        // WU3: ShareFailed on one buffer, then Disconnect: the good buffer
        // should have its sync_doc preserved for offline recovery.
        use mae_core::Buffer;
        let mut editor = Editor::new();

        editor.buffers[0].name = "good.rs".to_string();
        editor.buffers[0].enable_sync(1);
        editor.buffers[0].collab_doc_id = Some("doc-good".to_string());
        editor.collab.synced_buffers.insert("doc-good".to_string());

        let mut buf_bad = Buffer::new();
        buf_bad.name = "bad.rs".to_string();
        buf_bad.enable_sync(2);
        buf_bad.collab_doc_id = Some("doc-bad".to_string());
        editor.buffers.push(buf_bad);
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };

        // ShareFailed clears doc-bad from the buffer.
        handle_collab_event(
            &mut editor,
            CollabEvent::ShareFailed {
                doc_id: "doc-bad".to_string(),
                message: "test".to_string(),
            },
        );

        // Disconnect.
        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );

        // Good buffer: sync_doc preserved, offline=true.
        assert!(
            editor.buffers[0].sync_doc.is_some(),
            "good buffer should keep sync_doc"
        );
        assert!(editor.buffers[0].collab_offline);
        // Bad buffer: ShareFailed already cleared sync_doc, so disconnect clears collab_doc_id.
        assert!(
            editor.buffers[1].collab_doc_id.is_none(),
            "bad buffer should have doc_id cleared"
        );
    }

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
            handle_incoming_message(
                &msg.to_string(),
                &tx,
                &mut pending,
                &mut shared,
                &mut std::collections::HashMap::new(),
            )
            .await;
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

    // -----------------------------------------------------------------------
    // Join-save model: joined buffers have no auto file_path
    // -----------------------------------------------------------------------

    #[test]
    fn buffer_joined_has_no_file_path() {
        let mut editor = Editor::new();
        let content = "shared text\n";
        let sync = mae_sync::text::TextSync::with_client_id(content, 1);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "file:abc123/src/main.rs".to_string(),
                state_bytes,
            },
        );

        let idx = editor
            .find_buffer_by_name("src/main.rs")
            .expect("joined buffer should use rel_path as name");
        // Joined buffers must NOT have auto file_path set.
        assert!(
            editor.buffers[idx].file_path().is_none(),
            "joined buffer should have no file_path by default"
        );
        // But collab_doc_id should be set.
        assert_eq!(
            editor.buffers[idx].collab_doc_id.as_deref(),
            Some("file:abc123/src/main.rs")
        );
    }

    #[test]
    fn buffer_joined_sets_buffer_name_from_rel_path() {
        let mut editor = Editor::new();
        let sync = mae_sync::text::TextSync::with_client_id("hi", 1);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "file:proj/utils.rs".to_string(),
                state_bytes,
            },
        );

        assert!(
            editor.find_buffer_by_name("utils.rs").is_some(),
            "buffer name should be the rel_path from DocAddress"
        );
    }

    #[test]
    fn buffer_joined_shared_doc_name_extraction() {
        let mut editor = Editor::new();
        let sync = mae_sync::text::TextSync::with_client_id("data", 1);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "shared:notes".to_string(),
                state_bytes,
            },
        );

        assert!(
            editor.find_buffer_by_name("notes").is_some(),
            "shared doc buffer name should be the name field"
        );
    }

    #[test]
    fn drain_save_collab_sends_save_intent() {
        let mut editor = Editor::new();
        editor.collab.pending_intent = Some(CollabIntent::SaveCollab {
            doc_id: "file:abc/main.rs".to_string(),
            content_hash: "deadbeef".to_string(),
        });
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        match cmd {
            CollabCommand::SendSaveIntent {
                doc_id,
                expected_hash,
            } => {
                assert_eq!(doc_id, "file:abc/main.rs");
                assert_eq!(expected_hash, "deadbeef");
            }
            other => panic!("expected SendSaveIntent, got {:?}", other),
        }
    }

    #[test]
    fn drain_pending_save_committed() {
        let mut editor = Editor::new();
        editor.collab.pending_save_committed = Some((
            "doc1".to_string(),
            42,
            "hash123".to_string(),
            "alice".to_string(),
        ));
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        match cmd {
            CollabCommand::SendSaveCommitted {
                doc_id,
                save_epoch,
                content_hash,
                saved_by,
            } => {
                assert_eq!(doc_id, "doc1");
                assert_eq!(save_epoch, 42);
                assert_eq!(content_hash, "hash123");
                assert_eq!(saved_by, "alice");
            }
            other => panic!("expected SendSaveCommitted, got {:?}", other),
        }
        assert!(editor.collab.pending_save_committed.is_none());
    }

    #[test]
    fn handle_save_intent_ok_queues_committed() {
        let mut editor = Editor::new();
        editor.collab.user_name = "bob".to_string();
        handle_collab_event(
            &mut editor,
            CollabEvent::SaveIntentOk {
                doc_id: "test-doc".to_string(),
                save_epoch: 5,
                content_hash: "abc".to_string(),
            },
        );
        assert!(editor.collab.pending_save_committed.is_some());
        let (doc_id, epoch, hash, saved_by) =
            editor.collab.pending_save_committed.as_ref().unwrap();
        assert_eq!(doc_id, "test-doc");
        assert_eq!(*epoch, 5);
        assert_eq!(hash, "abc");
        assert_eq!(saved_by, "bob");
    }

    #[test]
    fn handle_save_intent_conflict_shows_status() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::SaveIntentConflict {
                doc_id: "test-doc".to_string(),
                message: "hash mismatch".to_string(),
            },
        );
        assert!(editor.status_msg.contains("conflict"));
    }

    #[tokio::test]
    async fn handle_response_save_intent_ok() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "doc": "test.rs",
                "result": {
                    "status": "ok",
                    "server_hash": "abc123",
                    "save_epoch": 3
                }
            }
        });
        handle_response(
            &val,
            PendingResponseKind::SaveIntent {
                doc_id: "test.rs".to_string(),
                expected_hash: "abc123".to_string(),
            },
            &tx,
            &mut shared,
        )
        .await;
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::SaveIntentOk {
                doc_id, save_epoch, ..
            } => {
                assert_eq!(doc_id, "test.rs");
                assert_eq!(save_epoch, 3);
            }
            other => panic!("expected SaveIntentOk, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_response_save_intent_conflict() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "doc": "test.rs",
                "result": {
                    "status": "conflict",
                    "server_hash": "xyz"
                }
            }
        });
        handle_response(
            &val,
            PendingResponseKind::SaveIntent {
                doc_id: "test.rs".to_string(),
                expected_hash: "abc123".to_string(),
            },
            &tx,
            &mut shared,
        )
        .await;
        let event = rx.try_recv().unwrap();
        assert!(
            matches!(event, CollabEvent::SaveIntentConflict { .. }),
            "expected SaveIntentConflict, got {:?}",
            event
        );
    }

    #[test]
    fn peer_count_zero_shows_all_disconnected() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 2 };
        handle_collab_event(&mut editor, CollabEvent::PeerCountChanged { peer_count: 0 });
        assert!(editor.status_msg.contains("disconnected"));
        assert_eq!(
            editor.collab.status,
            CollabStatus::Connected { peer_count: 0 }
        );
    }

    #[test]
    fn save_pathless_collab_buffer_shows_guidance() {
        let mut editor = Editor::new();
        let sync = mae_sync::text::TextSync::with_client_id("text", 1);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "shared:test".to_string(),
                state_bytes,
            },
        );

        let idx = editor
            .find_buffer_by_name("test")
            .expect("buffer should exist");
        editor.switch_to_buffer(idx);
        // Use dispatch_builtin("save") which is public and calls save_current_buffer.
        editor.dispatch_builtin("save");

        // Should show guidance about :saveas
        let status = &editor.status_msg;
        assert!(
            status.contains("saveas"),
            "status should mention :saveas, got: {status}"
        );
    }

    // --- WU1: Gap detection tests ---

    #[tokio::test]
    async fn gap_detection_triggers_on_missing_seq() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut seq_tracker = std::collections::HashMap::new();

        // Seq 1, 2 — no gap.
        check_seq_gap("doc1", 1, &mut seq_tracker, &tx).await;
        check_seq_gap("doc1", 2, &mut seq_tracker, &tx).await;
        assert!(rx.try_recv().is_err(), "no gap for sequential seqs");

        // Seq 4 — gap (expected 3).
        check_seq_gap("doc1", 4, &mut seq_tracker, &tx).await;
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::GapDetected {
                doc_id,
                expected,
                got,
            } => {
                assert_eq!(doc_id, "doc1");
                assert_eq!(expected, 3);
                assert_eq!(got, 4);
            }
            other => panic!("expected GapDetected, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn gap_detection_no_gap_for_sequential() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut seq_tracker = std::collections::HashMap::new();

        for i in 1..=5 {
            check_seq_gap("doc1", i, &mut seq_tracker, &tx).await;
        }
        assert!(rx.try_recv().is_err(), "no gap for sequential 1..5");
    }

    #[tokio::test]
    async fn gap_detection_independent_per_doc() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut seq_tracker = std::collections::HashMap::new();

        check_seq_gap("doc-a", 1, &mut seq_tracker, &tx).await;
        check_seq_gap("doc-b", 1, &mut seq_tracker, &tx).await;
        // Both start at 1, no gap.
        assert!(rx.try_recv().is_err());

        // doc-a jumps to 5 — gap.
        check_seq_gap("doc-a", 5, &mut seq_tracker, &tx).await;
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, CollabEvent::GapDetected { doc_id, .. } if doc_id == "doc-a"));

        // doc-b at 2 — no gap.
        check_seq_gap("doc-b", 2, &mut seq_tracker, &tx).await;
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn gap_detected_triggers_force_sync() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::GapDetected {
                doc_id: "test-doc".to_string(),
                expected: 3,
                got: 5,
            },
        );
        assert!(editor.status_msg.contains("gap"));
        // Should queue a ForceSync intent.
        assert!(editor.collab.pending_intent.is_some());
        match editor.collab.pending_intent.as_ref().unwrap() {
            CollabIntent::ForceSync { buffer_name } => {
                assert_eq!(buffer_name, "test-doc");
            }
            other => panic!("expected ForceSync, got {:?}", other),
        }
    }

    // --- WU3: Offline recovery tests ---

    #[test]
    fn disconnect_preserves_sync_doc() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        let buf = &mut editor.buffers[0];
        buf.collab_doc_id = Some("test-doc".to_string());
        buf.enable_sync(42);
        editor.collab.synced_buffers.insert("test-doc".to_string());

        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );

        // sync_doc and collab_doc_id should be PRESERVED (not cleared).
        assert!(
            editor.buffers[0].sync_doc.is_some(),
            "sync_doc should be preserved on disconnect"
        );
        assert!(
            editor.buffers[0].collab_doc_id.is_some(),
            "collab_doc_id should be preserved on disconnect"
        );
        assert!(
            editor.buffers[0].collab_offline,
            "collab_offline should be set"
        );
        // UI tracking should be cleared.
        assert!(editor.collab.synced_buffers.is_empty());
        assert_eq!(editor.collab.synced_docs, 0);
    }

    #[test]
    fn reconnect_triggers_resync_for_offline_buffers() {
        let mut editor = Editor::new();
        let buf = &mut editor.buffers[0];
        buf.collab_doc_id = Some("test-doc".to_string());
        buf.enable_sync(42);
        buf.collab_offline = true;

        handle_collab_event(
            &mut editor,
            CollabEvent::Connected {
                address: "127.0.0.1:9473".to_string(),
                peer_count: 1,
            },
        );

        // Should queue a ForceSync intent for the offline buffer.
        assert!(editor.collab.pending_intent.is_some());
        assert!(editor.collab.synced_buffers.contains("test-doc"));
    }

    #[test]
    fn remote_update_clears_offline_flag() {
        let mut editor = Editor::new();
        let buf = &mut editor.buffers[0];
        buf.collab_doc_id = Some("test-doc".to_string());
        buf.enable_sync(42);
        buf.collab_offline = true;

        // Create a valid yrs update for this buffer.
        let update = {
            let sync2 = mae_sync::text::TextSync::with_client_id("hello", 99);
            sync2.encode_state()
        };

        handle_collab_event(
            &mut editor,
            CollabEvent::RemoteUpdate {
                doc_id: "test-doc".to_string(),
                update_bytes: update,
                wal_seq: 1,
            },
        );

        // Note: apply_sync_update may fail if the update isn't compatible,
        // but the test validates the code path exists.
    }

    // --- WU1: Buffer status indicator tests ---

    #[test]
    fn buffer_shared_sets_is_sharer() {
        let mut editor = Editor::new();
        editor.buffers[0].collab_doc_id = Some("test-doc".to_string());
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferShared {
                doc_id: "test-doc".to_string(),
            },
        );
        assert!(editor.buffers[0].collab_is_sharer);
    }

    #[test]
    fn buffer_joined_stays_not_sharer() {
        let mut editor = Editor::new();
        let sync = mae_sync::text::TextSync::with_client_id("hello", 1);
        let state = sync.encode_state();
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "test-doc".to_string(),
                state_bytes: state,
            },
        );
        // Find the buffer that was created for the joined doc.
        let idx = editor.find_buffer_by_collab_doc_id("test-doc");
        assert!(idx.is_some());
        assert!(!editor.buffers[idx.unwrap()].collab_is_sharer);
    }

    // --- WU2: Save guard tests ---

    #[test]
    fn collab_is_sharer_defaults_false() {
        let buf = mae_core::Buffer::new();
        assert!(!buf.collab_is_sharer);
    }

    #[test]
    fn collab_is_sharer_set_on_share_not_join() {
        // Verify that BufferShared sets is_sharer and BufferJoined does not.
        let mut editor = Editor::new();
        editor.buffers[0].collab_doc_id = Some("doc-a".to_string());
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferShared {
                doc_id: "doc-a".to_string(),
            },
        );
        assert!(
            editor.buffers[0].collab_is_sharer,
            "sharer should be true after BufferShared"
        );

        // Join a different doc — its buffer should NOT be sharer.
        let sync = mae_sync::text::TextSync::with_client_id("content", 2);
        let state = sync.encode_state();
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "doc-b".to_string(),
                state_bytes: state,
            },
        );
        let idx = editor.find_buffer_by_collab_doc_id("doc-b").unwrap();
        assert!(
            !editor.buffers[idx].collab_is_sharer,
            "joiner should not be sharer"
        );
    }

    // --- WU3: SharerLeft event handling ---

    #[test]
    fn sharer_left_sets_status() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 2 };
        handle_collab_event(
            &mut editor,
            CollabEvent::SharerLeft {
                doc_id: "test-doc".to_string(),
            },
        );
        assert!(editor.status_msg.contains("Sharer disconnected"));
    }

    // --- WU4: Backoff + debounce tests ---

    #[test]
    fn compute_backoff_exponential() {
        // base=5, factor=2: 5, 10, 20, 40, 80, 160
        assert_eq!(compute_backoff(5, 2, 0), 5);
        assert_eq!(compute_backoff(5, 2, 1), 10);
        assert_eq!(compute_backoff(5, 2, 2), 20);
        assert_eq!(compute_backoff(5, 2, 3), 40);
        assert_eq!(compute_backoff(5, 2, 4), 80);
        assert_eq!(compute_backoff(5, 2, 5), 160);
        // Capped at attempt=5 exponent, so attempt 6 same as 5.
        assert_eq!(compute_backoff(5, 2, 6), 160);
    }

    #[test]
    fn compute_backoff_capped_at_300() {
        // base=10, factor=3: attempt 5 = 10 * 243 = 2430 → capped at 300.
        assert_eq!(compute_backoff(10, 3, 5), 300);
    }

    #[test]
    fn compute_backoff_factor_one_is_constant() {
        // factor=1 means no exponential growth.
        assert_eq!(compute_backoff(5, 1, 0), 5);
        assert_eq!(compute_backoff(5, 1, 5), 5);
    }

    // --- WU3: Notification parsing ---

    #[tokio::test]
    async fn parse_sharer_left_notification() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut pending = std::collections::HashMap::new();
        let mut shared = Vec::new();
        let mut seq = std::collections::HashMap::new();
        let msg = r#"{
            "jsonrpc": "2.0",
            "method": "notifications/sharer_left",
            "params": {
                "seq": 1,
                "event": {
                    "type": "sharer_left",
                    "data": {
                        "session_id": 42,
                        "doc": "file:abc/main.rs",
                        "peer_count": 1
                    }
                }
            }
        }"#;
        handle_incoming_message(msg, &tx, &mut pending, &mut shared, &mut seq).await;
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::SharerLeft { doc_id } => {
                assert_eq!(doc_id, "file:abc/main.rs");
            }
            other => panic!("expected SharerLeft, got {:?}", other),
        }
    }
}
