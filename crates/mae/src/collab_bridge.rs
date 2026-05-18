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
        initial_content: String,
    },
    ForceSync {
        doc_id: String,
    },
    ShowStatus,
    Doctor,
    StartServer,
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
            let content = editor
                .find_buffer_by_name(&buffer_name)
                .map(|idx| editor.buffers[idx].text())
                .unwrap_or_default();
            // Use buffer name as doc_id for MVP
            CollabCommand::ShareBuffer {
                doc_id: buffer_name,
                initial_content: content,
            }
        }
        CollabIntent::ForceSync { buffer_name } => CollabCommand::ForceSync {
            doc_id: buffer_name,
        },
        CollabIntent::Doctor => CollabCommand::Doctor,
    };

    let kind = collab_command_name(&cmd);
    if collab_tx.try_send(cmd).is_err() {
        warn!(
            kind,
            "collab command channel full or closed — intent dropped"
        );
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
            editor.collab_synced_docs = 0;
            editor.mark_full_redraw();
        }
        CollabEvent::RemoteUpdate {
            doc_id,
            update_bytes,
        } => {
            if let Some(idx) = editor.find_buffer_by_name(&doc_id) {
                match editor.buffers[idx].apply_sync_update(&update_bytes) {
                    Ok(()) => {
                        debug!(doc = %doc_id, "applied remote sync update");
                        editor.mark_full_redraw();
                    }
                    Err(e) => {
                        warn!(doc = %doc_id, error = %e, "failed to apply remote sync update");
                    }
                }
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
    use tokio::io::BufReader;
    use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::net::TcpStream;

    let mut reader: Option<BufReader<OwnedReadHalf>> = None;
    let mut writer: Option<OwnedWriteHalf> = None;
    let mut target_address: Option<String> = None;
    let mut shared_docs: Vec<String> = Vec::new();
    let mut reconnect_enabled = false;

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
                biased;

                Some(cmd) = cmd_rx.recv() => {
                    match cmd {
                        CollabCommand::Disconnect => {
                            tear_down(&mut reader, &mut writer);
                            reconnect_enabled = false;
                            shared_docs.clear();
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
                        CollabCommand::ShareBuffer { doc_id, initial_content } => {
                            if let Some(ref mut w) = writer {
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": 1,
                                    "method": "sync/full_state",
                                    "params": {
                                        "doc_id": doc_id,
                                        "content": initial_content,
                                    }
                                });
                                let body = serde_json::to_vec(&req).unwrap();
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    if !shared_docs.contains(&doc_id) {
                                        shared_docs.push(doc_id.clone());
                                    }
                                    let _ = evt_tx.send(CollabEvent::StatusReport {
                                        lines: vec![format!("Shared: {}", doc_id)],
                                    }).await;
                                } else {
                                    let _ = evt_tx.send(CollabEvent::Error {
                                        message: format!("Failed to share {}", doc_id),
                                    }).await;
                                }
                            }
                        }
                        CollabCommand::ForceSync { doc_id } => {
                            if let Some(ref mut w) = writer {
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": 2,
                                    "method": "sync/full_state",
                                    "params": { "doc_id": doc_id }
                                });
                                let body = serde_json::to_vec(&req).unwrap();
                                if write_framed(w, &body, write_timeout).await.is_err() {
                                    let _ = evt_tx.send(CollabEvent::Error {
                                        message: format!("Failed to sync {}", doc_id),
                                    }).await;
                                }
                            }
                        }
                        CollabCommand::Connect { address } => {
                            tear_down(&mut reader, &mut writer);
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
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                                if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
                                    match method {
                                        "sync/update" => {
                                            if let Some(params) = val.get("params") {
                                                let doc_id = params.get("doc_id")
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                                let update_b64 = params.get("update")
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("");
                                                if let Ok(bytes) = mae_sync::encoding::base64_to_update(update_b64) {
                                                    let _ = evt_tx.send(CollabEvent::RemoteUpdate {
                                                        doc_id,
                                                        update_bytes: bytes,
                                                    }).await;
                                                }
                                            }
                                        }
                                        _ => {
                                            debug!(method = method, "unhandled server notification");
                                        }
                                    }
                                }
                            }
                        }
                        Ok(None) | Err(_) => {
                            tear_down(&mut reader, &mut writer);
                            shared_docs.clear();
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
                                &mut shared_docs, write_timeout,
                            ).await;
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(reconnect_secs)) => {
                            if let Ok(mut stream) = TcpStream::connect(&addr_clone).await {
                                if send_initialize(&mut stream, write_timeout).await {
                                    install_connection(stream, &mut reader, &mut writer);
                                    let _ = evt_tx.send(CollabEvent::Connected {
                                        address: addr_clone,
                                        peer_count: 0,
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
                    write_timeout,
                )
                .await;
            }
        }
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
    write_timeout: std::time::Duration,
) {
    use tokio::io::BufReader;

    match cmd {
        CollabCommand::Connect { address } => {
            *target_address = Some(address.clone());
            match tokio::net::TcpStream::connect(&address).await {
                Ok(mut stream) => {
                    if send_initialize(&mut stream, write_timeout).await {
                        let (r, w) = stream.into_split();
                        *reader = Some(BufReader::new(r));
                        *writer = Some(w);
                        *reconnect_enabled = true;
                        let _ = evt_tx
                            .send(CollabEvent::Connected {
                                address,
                                peer_count: 0,
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
                            if send_initialize(&mut stream, write_timeout).await {
                                let (r, w) = stream.into_split();
                                *reader = Some(BufReader::new(r));
                                *writer = Some(w);
                                *reconnect_enabled = true;
                                if let Err(e) = evt_tx
                                    .send(CollabEvent::Connected {
                                        address: addr,
                                        peer_count: 0,
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
    }
}

/// Send JSON-RPC `initialize` handshake to the state server.
/// Returns true on success. Takes `&mut` because we need to write.
async fn send_initialize(stream: &mut tokio::net::TcpStream, timeout: std::time::Duration) -> bool {
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
    write_framed(stream, &body, timeout).await.is_ok()
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
    fn drain_collab_share_includes_content() {
        let mut editor = Editor::new();
        let buf_name = editor.buffers[0].name.clone();
        editor.pending_collab_intent = Some(CollabIntent::ShareBuffer {
            buffer_name: buf_name.clone(),
        });
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        match cmd {
            CollabCommand::ShareBuffer { doc_id, .. } => {
                assert_eq!(doc_id, buf_name);
            }
            other => panic!("expected ShareBuffer, got {:?}", other),
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
        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );
        assert_eq!(editor.collab_status, CollabStatus::Disconnected);
        assert_eq!(editor.collab_synced_docs, 0);
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
}
