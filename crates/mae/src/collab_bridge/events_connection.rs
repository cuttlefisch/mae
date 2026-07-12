//! Connection-lifecycle `CollabEvent` arms and `CollabIntent` translations,
//! split out of `collab_bridge/mod.rs`'s `handle_collab_event` /
//! `drain_collab_intents` matches (pure code motion — see that module's doc
//! comment). Covers: host-key TOFU prompts, connect/disconnect, peer-count
//! changes, server start/fail, generic errors, and the `*Collab Status*` /
//! `*Collab Doctor*` report buffers.

use super::*;

pub(super) fn handle_host_key_prompt_event(
    editor: &mut Editor,
    addr: String,
    fingerprint: String,
    reply: std::sync::mpsc::Sender<bool>,
) {
    tracing::debug!(target: "collab", addr = %addr, "host-key TOFU prompt raised (awaiting trust decision)");
    // ADR-017 TOFU, now via the ADR-024 bus: a BlockingReply notification
    // routes to a modal; the y/n answer in apply_mini_dialog sends back on
    // `reply` (unblocking the connection task). One consumer of the generic
    // mechanism — no bespoke `pending_host_key_reply` field anymore.
    editor.notify(
        mae_core::notifications::Notification::action_required(
            "collab",
            format!("Trust daemon at {addr}?"),
        )
        .body(format!("{fingerprint}  (first connect — accept & pin?)"))
        // B-22c: explicit bus actions so the prompt is answerable via
        // `notify_resolve` / the `*Notifications*` row (headless/agent parity,
        // and a working answer path while the GUI modal paint is fixed), in
        // addition to the modal y/n keypress — both send on the reply channel.
        .action(
            "Accept & pin",
            mae_core::notifications::NotifCommand::Reply(true),
        )
        .action(
            "Reject",
            mae_core::notifications::NotifCommand::Reply(false),
        )
        .blocking(mae_core::notifications::NotifReply::Bool(reply)),
    );
    editor.mark_full_redraw();
}

pub(super) fn handle_connected_event(editor: &mut Editor, address: String, peer_count: usize) {
    info!(address = %address, peers = peer_count, "collab connected");
    editor.collab.status = CollabStatus::Connected { peer_count };
    editor.set_status(format!("Connected to {} ({} peers)", address, peer_count));
    // Proactively surface the daemon state (ADR-035 PR C-b).
    editor.notify_daemon_connected(peer_count);
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

    // ADR-019: KBs get the same reconnect care as buffers. Rebuild the
    // gate cache from durable markers, then re-subscribe every durably-
    // shared INSTANCE so remote edits resume flowing — guests re-join,
    // owners re-share, primary is skipped (see kb_resubscribe_intents).
    // Idempotent via subscribed_kbs; queued one-per-tick.
    editor.reconstruct_kb_sync_gate();
    let mut resubscribed = 0;
    for intent in editor.kb_resubscribe_intents() {
        let key = match &intent {
            CollabIntent::JoinKb { kb_id, .. } => kb_id.clone(),
            CollabIntent::ShareKb { kb_name, .. } => kb_name.clone(),
            _ => continue,
        };
        if editor.collab.subscribed_kbs.insert(key) {
            editor.collab.reconnect_intents.push_back(intent);
            resubscribed += 1;
        }
    }
    if resubscribed > 0 {
        info!(count = resubscribed, "reconnect: re-subscribing shared KBs");
    }

    // Phase D (ADR-029): if opted in, host the primary KB on the daemon as a
    // "shared-with-self" collection (kbc:default). refresh_daemon_host_state()
    // flips the runtime gate now that we're Connected; if hosting and not
    // already in flight this connection, enqueue ONE host-only share — the
    // idempotent kb/share imports the primary into the daemon's CRDT, which the
    // projector materializes. host_only routing (no durable marker) is keyed by
    // daemon_host_pending, consumed on the KbShared confirm.
    editor.refresh_daemon_host_state();
    if editor.kb.daemon_hosts_primary() {
        let kb_id = mae_core::KB_DEFAULT_NAME.to_string();
        if editor.collab.daemon_host_pending.insert(kb_id.clone()) {
            let node_ids: Vec<String> = editor.kb.primary.list_ids(None);
            info!(
                kb = %kb_id,
                nodes = node_ids.len(),
                "Phase D: hosting primary KB on daemon (host-only share)"
            );
            editor
                .collab
                .reconnect_intents
                .push_back(CollabIntent::ShareKb {
                    kb_name: kb_id,
                    node_ids,
                });
        }
    }

    editor.mark_full_redraw();
}

pub(super) fn handle_disconnected_event(editor: &mut Editor, reason: String) {
    info!(reason = %reason, "collab disconnected");
    editor.collab.status = CollabStatus::Disconnected;
    // Proactively surface the loss BEFORE the host/share state is torn down
    // below, so `has_active_shares()` still sees that we were syncing and
    // raises the sticky "deferred, not lost" badge (ADR-035 PR C-b).
    editor.notify_daemon_disconnected(&reason);
    // Phase D3b: snapshot the mirror back to the local store BEFORE dropping
    // the host flag, so this session's edits (whose per-edit write-through was
    // retired) land in the daemon-less fallback. Cheap (only touched nodes).
    if editor.kb.daemon_hosts_primary() {
        editor.kb_snapshot_primary_to_store();
    }
    // Phase D: hosting requires a live write channel — drop the runtime flag
    // and clear in-flight host shares so the next connect re-hosts cleanly.
    editor.collab.daemon_host_pending.clear();
    editor.refresh_daemon_host_state();
    // Re-subscribe on the next connect (ADR-019).
    editor.collab.subscribed_kbs.clear();
    // ADR-020: any kb/node_update whose apply-confirmation was still in flight
    // will never be answered now — release the marks so the durable rows
    // re-drain on reconnect (the rows themselves are untouched in SQLite).
    editor.collab.inflight_kb_updates.clear();
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
    // Log pending updates that will be orphaned by clearing synced_buffers.
    let orphaned_updates: usize = editor
        .buffers
        .iter()
        .filter(|b| !b.pending_sync_updates.is_empty())
        .map(|b| b.pending_sync_updates.len())
        .sum();
    if orphaned_updates > 0 {
        warn!(
            orphaned_updates,
            synced_buffers_before = ?editor.collab.synced_buffers,
            "DISCONNECT: clearing synced_buffers with pending updates — these will be LOST"
        );
    }
    editor.collab.synced_buffers.clear();
    editor.collab.confirmed_shares.clear();
    editor.mark_full_redraw();
}

pub(super) fn handle_status_report_event(editor: &mut Editor, lines: Vec<String>) {
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

pub(super) fn handle_doctor_report_event(editor: &mut Editor, lines: Vec<String>) {
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

pub(super) fn handle_server_started_event(editor: &mut Editor, pid: u32) {
    info!(pid = pid, "state server started");
    // Register mDNS service for peer discovery.
    let port = editor
        .collab
        .server_address
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(mae_core::DEFAULT_COLLAB_PORT);
    let user = if editor.collab.user_name.is_empty() {
        "mae-user"
    } else {
        &editor.collab.user_name
    };
    match crate::mdns_discovery::MdnsManager::new() {
        Ok(mut mgr) => {
            let kb_count = editor.collab.shared_kbs.len() as u32;
            if let Err(e) = mgr.register(user, port, kb_count) {
                warn!(error = %e, "mDNS registration failed");
            } else {
                info!(port, user, "mDNS service registered");
            }
            // Also browse so this host discovers peers (and `collab-discover`
            // reads results without blocking the main thread, #1).
            if let Err(e) = mgr.start_browse() {
                debug!(error = %e, "mDNS browse start failed");
            }
            // Store in module-level static so it lives for the editor's lifetime
            // and auto-unregisters on drop (no memory leak).
            if let Ok(mut guard) = MDNS_MANAGER.lock() {
                *guard = Some(mgr);
            }
        }
        Err(e) => {
            debug!(error = %e, "mDNS unavailable — peer discovery disabled");
        }
    }
    editor.set_status(format!("State server started (PID {})", pid));
    editor.mark_full_redraw();
}

pub(super) fn handle_server_failed_event(editor: &mut Editor, error: String) {
    error!(error = %error, "state server failed to start");
    editor.set_status(format!("State server failed: {}", error));
    editor.mark_full_redraw();
}

pub(super) fn handle_error_event(editor: &mut Editor, message: String) {
    warn!(error = %message, "collab error");
    editor.set_status(format!("Collab: {}", message));
    editor.mark_full_redraw();
}

pub(super) fn handle_peer_count_changed_event(editor: &mut Editor, peer_count: usize) {
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

// --- CollabIntent -> CollabCommand translation (connection-lifecycle intents) ---

/// Translate a connection-lifecycle `CollabIntent` into the `CollabCommand` sent
/// to the background task. All arms in this group are infallible (no early
/// return), unlike the doc/KB groups.
pub(super) fn connection_intent_to_command(
    _editor: &mut Editor,
    intent: CollabIntent,
) -> CollabCommand {
    match intent {
        CollabIntent::StartServer => CollabCommand::StartServer,
        CollabIntent::Connect { address } => {
            // Start discovering LAN peers in the background so `collab-discover`
            // has results ready (non-blocking — just spawns a browse thread).
            ensure_mdns_browsing();
            CollabCommand::Connect { address }
        }
        CollabIntent::Disconnect => CollabCommand::Disconnect,
        CollabIntent::RotateIdentity => CollabCommand::RotateIdentity,
        CollabIntent::RegisterRecoveryKey => CollabCommand::RegisterRecoveryKey,
        CollabIntent::RecoverIdentity {
            recovery_path,
            old_fp,
        } => CollabCommand::RecoverIdentity {
            recovery_path,
            old_fp,
        },
        CollabIntent::ShowStatus => CollabCommand::ShowStatus,
        other => unreachable!(
            "connection_intent_to_command called with non-connection intent: {other:?}"
        ),
    }
}
