//! Buffer/doc-sync `CollabEvent` arms and `CollabIntent` translations, split
//! out of `collab_bridge/mod.rs`'s `handle_collab_event` /
//! `drain_collab_intents` matches (pure code motion — see that module's doc
//! comment). Covers: remote CRDT updates, WAL gap resync, share/join
//! lifecycle, save-intent protocol, awareness (cursor/presence), and the
//! local self-protection blocklist.

use super::*;

pub(super) fn handle_remote_update_event(
    editor: &mut Editor,
    doc_id: String,
    update_bytes: Vec<u8>,
    wal_seq: u64,
) {
    // C1: a `kbc:` broadcast is a KB collection-doc (membership/epoch)
    // delta, NOT buffer text. Apply it to our local collection replica and
    // relearn our authorization epoch live — no manual reconnect. It never
    // maps to a buffer, so intercept it before the buffer lookup.
    if let Some(kb_id) = doc_id.strip_prefix("kbc:") {
        handle_kbc_membership_broadcast(editor, kb_id, &update_bytes);
    } else if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc_id) {
        // Capture cursor char offsets for all windows viewing this buffer
        // so we can restore them after the rope rebuild.
        let window_cursors: Vec<(mae_core::WindowId, usize)> = editor
            .window_mgr
            .iter_windows()
            .filter(|w| w.buffer_idx == idx)
            .map(|w| {
                (
                    w.id,
                    editor.buffers[idx].char_offset_at(w.cursor_row, w.cursor_col),
                )
            })
            .collect();
        let old_offsets: Vec<usize> = window_cursors.iter().map(|(_, o)| *o).collect();

        // The edit's exact position/delta comes from yrs's own transaction
        // delta (via apply_sync_update_with_cursors -> TextSync::
        // apply_update_with_edit) — not an inferred before/after rope diff,
        // which used to silently miss same-length replacements entirely.
        match editor.buffers[idx].apply_sync_update_with_cursors(&update_bytes, &old_offsets) {
            Ok(adjusted_offsets) => {
                let new_len = editor.buffers[idx].rope().len_chars();
                let text_preview: String = editor.buffers[idx].rope().chars().take(200).collect();

                info!(
                    doc = %doc_id,
                    wal_seq,
                    update_len = update_bytes.len(),
                    buf_idx = idx,
                    buf_name = %editor.buffers[idx].name,
                    new_len,
                    text_preview = %text_preview,
                    "applied remote sync update"
                );

                for ((win_id, _), adjusted) in window_cursors.iter().zip(adjusted_offsets) {
                    if let Some(win) = editor.window_mgr.window_mut(*win_id) {
                        let (row, col) = editor.buffers[idx].row_col_from_offset(adjusted);
                        win.cursor_row = row;
                        win.cursor_col = col;
                        win.clamp_cursor(&editor.buffers[idx]);
                    }
                }
                // Clear offline flag on successful remote update.
                editor.buffers[idx].collab_offline = false;
                editor.fire_hook("sync-update");
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

pub(super) fn handle_gap_detected_event(
    editor: &mut Editor,
    doc_id: String,
    expected: u64,
    got: u64,
) {
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

pub(super) fn handle_buffer_shared_event(editor: &mut Editor, doc_id: String) {
    info!(doc = %doc_id, "buffer shared (server confirmed)");
    // Doc was already added optimistically in drain_collab_intents (BUG A fix).
    // This insert is idempotent — ensures consistency if event ordering varies.
    editor.collab.synced_buffers.insert(doc_id.clone());
    editor.collab.synced_docs = editor.collab.synced_buffers.len();
    // Mark as server-confirmed (not just optimistically requested).
    editor.collab.confirmed_shares.insert(doc_id.clone());
    // Mark this buffer as the sharer (authoritative saver).
    if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc_id) {
        editor.buffers[idx].collab_is_sharer = true;
    }
    editor.set_status(format!("Shared: {}", doc_id));
    editor.mark_full_redraw();
}

pub(super) fn handle_blocklist_updated_event(
    editor: &mut Editor,
    blocklist: std::collections::HashMap<String, Vec<String>>,
) {
    // ADR-039 A2 (#162): cache the daemon's authoritative local blocklist and
    // repaint the *KB Sharing* Blocked view. Display-only — the daemon enforces.
    debug!(kbs = blocklist.len(), "local blocklist updated");
    editor.collab.kb_blocklists = blocklist;
    editor.refresh_kb_sharing_buffer();
}

pub(super) fn handle_doc_list_event(editor: &mut Editor, documents: Vec<String>, for_join: bool) {
    debug!(count = documents.len(), for_join, "doc list received");
    if for_join {
        // Open a palette picker with the document names.
        if documents.is_empty() {
            editor.set_status("No documents on server");
        } else {
            let names: Vec<&str> = documents.iter().map(|s| s.as_str()).collect();
            let palette = mae_core::command_palette::CommandPalette::for_collab_join(&names);
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
            lines.push("Use :collab-join <name> or SPC C j to join a document.".to_string());
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

pub(super) fn handle_buffer_joined_event(
    editor: &mut Editor,
    doc_id: String,
    state_bytes: Vec<u8>,
) {
    info!(doc = %doc_id, state_bytes = state_bytes.len(), "buffer joined event received");
    // Parse DocAddress from doc_id for structured addressing.
    let doc_addr = mae_sync::DocAddress::parse(&doc_id);
    // Use a display-friendly name for the buffer.
    let buf_name = match &doc_addr {
        Some(mae_sync::DocAddress::File { rel_path, .. }) => rel_path.clone(),
        Some(mae_sync::DocAddress::Shared { name }) => name.clone(),
        Some(mae_sync::DocAddress::KbNode { node_id }) => node_id.clone(),
        Some(mae_sync::DocAddress::KbCollection { kb_id }) => {
            format!("[kbc:{kb_id}]")
        }
        None => doc_id.clone(),
    };
    // Check if a buffer with this collab_doc_id already exists (e.g.,
    // the user shared a buffer and then also joined it, or a ForceSync
    // response arrived). Using the doc_id match prevents creating a
    // duplicate buffer with a different name but the same CRDT doc,
    // which causes remote updates to be applied to the wrong sync_doc.
    let existing_by_doc_id = editor.find_buffer_by_collab_doc_id(&doc_id);
    let already_existed =
        existing_by_doc_id.is_some() || editor.find_buffer_by_name(&buf_name).is_some();
    let idx = if let Some(i) = existing_by_doc_id {
        info!(doc = %doc_id, buf_idx = i, buf_name = %editor.buffers[i].name,
            "join: reusing existing buffer with same collab_doc_id");
        i
    } else {
        editor.find_or_create_buffer(&buf_name, || {
            let mut buf = mae_core::Buffer::new();
            buf.name = buf_name.clone();
            buf.kind = mae_core::BufferKind::Text;
            buf
        })
    };
    // Snapshot project root before mutable borrow of buffer.
    let project_root = editor.active_project_root().map(|p| p.to_path_buf());
    let client_id = compute_client_id(idx);
    let load_ok = {
        let buf = &mut editor.buffers[idx];
        if already_existed && buf.sync_doc.is_some() {
            // Existing synced buffer (ForceSync resync): merge state via
            // apply_update to preserve undo/redo history. yrs handles
            // already-applied operations idempotently via vector clocks.
            info!(doc = %doc_id, "resync: merging state into existing buffer (preserving undo history)");
            match buf.apply_sync_update(&state_bytes) {
                Ok(()) => Ok(()),
                Err(e) => {
                    warn!(doc = %doc_id, error = %e, "resync merge failed, falling back to full load");
                    buf.load_sync_state(&state_bytes, client_id).map(|()| {
                        buf.doc_address = doc_addr.clone();
                    })
                }
            }
        } else {
            // New buffer (explicit join): full state load.
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
        }
    };
    match load_ok {
        Ok(()) => {
            let text_preview: String = editor.buffers[idx].text().chars().take(200).collect();
            info!(doc = %doc_id, buf_idx = idx, text_len = editor.buffers[idx].text().len(),
                text_preview = %text_preview, "buffer joined: sync state loaded");
            // Store doc_id on buffer only after successful load — prevents
            // RemoteUpdate from targeting a buffer with no valid sync_doc.
            editor.buffers[idx].collab_doc_id = Some(doc_id.clone());
            // Detect language from doc_id for syntax highlighting.
            {
                let content = editor.buffers[idx].text();
                let path_hint = std::path::Path::new(&doc_id);
                if let Some(lang) = mae_core::syntax::language_for_buffer(path_hint, &content) {
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
            // Mark as server-confirmed.
            editor.collab.confirmed_shares.insert(doc_id.clone());
            // Only switch active buffer for newly created buffers (explicit join).
            // For existing buffers (ForceSync resync), don't steal focus.
            if !already_existed {
                editor.switch_to_buffer(idx);
                editor.set_status(format!("Joined: {}", doc_id));
            } else {
                info!(doc = %doc_id, buf_idx = idx, "buffer resync complete (no focus switch)");
            }
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
                        editor.mini_dialog =
                            Some(mae_core::command_palette::MiniDialogState::confirm(
                                format!("Map to local project file {}? (y/n)", display),
                                mae_core::command_palette::MiniDialogContext::CollabResolvePath {
                                    buf_idx: idx,
                                    resolved_path,
                                },
                            ));
                    }
                }
            }
        }
        Err(e) => {
            editor.set_status(format!("Failed to join {}: {}", doc_id, e));
        }
    }
}

pub(super) fn handle_share_failed_event(editor: &mut Editor, doc_id: String, message: String) {
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

pub(super) fn handle_save_intent_ok_event(
    editor: &mut Editor,
    doc_id: String,
    save_epoch: u64,
    content_hash: String,
) {
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

pub(super) fn handle_save_intent_conflict_event(
    editor: &mut Editor,
    doc_id: String,
    message: String,
) {
    warn!(doc = %doc_id, "save intent conflict: {}", message);
    editor.set_status(format!(
        "Save conflict on {} — sync first (:collab-sync)",
        doc_id
    ));
    editor.mark_full_redraw();
}

pub(super) fn handle_sharer_left_event(editor: &mut Editor, doc_id: String) {
    warn!(doc = %doc_id, "sharer disconnected");
    editor.set_status(format!("Sharer disconnected for {}", doc_id));
    editor.mark_full_redraw();
}

pub(super) fn handle_peer_saved_event(editor: &mut Editor, doc: String, saved_by: String) {
    debug!(doc = %doc, saved_by = %saved_by, "peer saved");
    editor.set_status(format!("[{}] saved by {}", doc, saved_by));
    // Mark the local buffer clean if we have it (content matches what was saved).
    if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc) {
        editor.buffers[idx].modified = false;
    }
    editor.mark_full_redraw();
}

pub(super) fn handle_awareness_update_event(
    editor: &mut Editor,
    client_id: u64,
    doc_id: String,
    state: mae_sync::awareness::AwarenessState,
) {
    let color_index = mae_core::render_common::collab_colors::collab_color_index(client_id);
    debug!(
        client_id,
        doc = %doc_id,
        user = %state.user_name,
        row = state.cursor_row,
        col = state.cursor_col,
        "awareness update received"
    );
    editor
        .collab
        .remote_users
        .update(client_id, doc_id, state, color_index);
    editor.mark_full_redraw();
}

// --- CollabIntent -> CollabCommand translation (doc/buffer intents) ---

/// Translate a doc/buffer `CollabIntent` into the `CollabCommand` sent to the
/// background task. Returns `None` when the arm has no command to send (e.g.
/// buffer not found, or the intent is fully handled locally like
/// `DiscoverPeers`) — the caller must skip the send in that case, matching
/// the original arm's early `return`.
pub(super) fn doc_intent_to_command(
    editor: &mut Editor,
    intent: CollabIntent,
) -> Option<CollabCommand> {
    match intent {
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
                    let client_id = compute_client_id(idx);
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
                Some(CollabCommand::ShareBuffer {
                    doc_id,
                    state_bytes,
                })
            } else {
                None // Buffer not found
            }
        }
        CollabIntent::ForceSync { buffer_name } => Some(CollabCommand::ForceSync {
            doc_id: buffer_name,
        }),
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
            Some(CollabCommand::Doctor { synced_info })
        }
        CollabIntent::SaveCollab {
            doc_id,
            content_hash,
        } => Some(CollabCommand::SendSaveIntent {
            doc_id,
            expected_hash: content_hash,
        }),
        CollabIntent::ListDocs => Some(CollabCommand::ListDocs { for_join: false }),
        CollabIntent::ListDocsForJoin => Some(CollabCommand::ListDocs { for_join: true }),
        CollabIntent::JoinDoc { doc_id } => Some(CollabCommand::JoinDoc { doc_id }),
        CollabIntent::DiscoverPeers => {
            // Read the BACKGROUND browse's current snapshot — never sleep on the
            // main thread (#1). The browse runs persistently (started on connect /
            // server-start / first discover), so results accumulate over time.
            ensure_mdns_browsing();
            let peers = mdns_discovered_peers();
            if peers.is_empty() {
                editor.set_status(
                    "Discovering MAE peers… run :collab-discover again in a moment.".to_string(),
                );
            } else {
                let mut lines = vec![
                    "Discovered MAE Peers".to_string(),
                    "====================".to_string(),
                    String::new(),
                ];
                for p in &peers {
                    lines.push(format!(
                        "  {} — {} (v{}, {} KBs)",
                        p.user_name, p.address, p.version, p.kb_count
                    ));
                }
                lines.push(String::new());
                lines.push("Use :collab-connect <address> to connect to a peer.".to_string());
                let content = lines.join("\n");
                let idx = editor.find_or_create_buffer("*Collab Discover*", || {
                    let mut buf = mae_core::buffer::Buffer::new();
                    buf.name = "*Collab Discover*".to_string();
                    buf
                });
                editor.buffers[idx].replace_contents(&content);
                editor.switch_to_buffer(idx);
                editor.set_status(format!("Found {} peer(s)", peers.len()));
            }
            None
        }
        other => unreachable!("doc_intent_to_command called with non-doc intent: {other:?}"),
    }
}
