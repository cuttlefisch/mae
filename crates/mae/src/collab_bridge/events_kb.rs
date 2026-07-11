//! KB-sharing `CollabEvent` arms, the KB-node-update drain section, and
//! `CollabIntent` translations, split out of `collab_bridge/mod.rs`'s
//! `handle_collab_event` / `drain_collab_intents` (pure code motion — see
//! that module's doc comment). Covers: KB share/join/leave lifecycle, remote
//! node updates (ADR-020 queue→send→confirm→ack), the ADR-023 epoch-fence
//! rejection + ADR-024 adopt/re-author flow, and KB membership/policy intents.

use super::*;

pub(super) fn handle_kb_shared_event(
    editor: &mut Editor,
    kb_id: String,
    node_count: usize,
    collection_state: Vec<u8>,
) {
    info!(kb = %kb_id, node_count, "KB shared successfully");
    // OQ1 / introspection: seed the owner's local collection replica from
    // the daemon's authoritative collection state so the owner can see its
    // own KB's members/roles/policy in `kb_sharing_snapshot`. C1's
    // `handle_kbc_membership_broadcast` then advances this replica on every
    // membership change. Re-derive the owner's epoch from the same doc.
    if !collection_state.is_empty() {
        if let Ok(coll) = mae_sync::kb::KbCollectionDoc::from_bytes(&collection_state) {
            if !editor.collab.local_fingerprint.is_empty() {
                let epoch = coll.epoch_of(&editor.collab.local_fingerprint);
                if epoch == 0 {
                    editor.collab.kb_epochs.remove(&kb_id);
                } else {
                    editor.collab.kb_epochs.insert(kb_id.clone(), epoch);
                }
            }
        }
        editor
            .collab
            .kb_collection_state
            .insert(kb_id.clone(), collection_state);
    }
    // Track which nodes are shared for continuous sync.
    // Get node IDs from the primary KB (or the named instance).
    let node_ids: HashSet<String> = if kb_id == mae_core::KB_DEFAULT_NAME || kb_id == "primary" {
        if let Some(q) = editor.kb.query_layer() {
            q.list_ids(None).into_iter().collect()
        } else {
            editor.kb.primary.list_ids(None).into_iter().collect()
        }
    } else {
        // `instances` is keyed by UUID, but `kb_id` is the human name
        // (e.g. "collabtest"). Resolve name→uuid via the registry first,
        // else `shared_kbs` gets an EMPTY set and later edits to the
        // shared KB never match → no kb/node_update is broadcast (I-9).
        let uuid = editor
            .kb
            .registry
            .find(&kb_id)
            .map(|inst| inst.uuid.clone());
        let kb = uuid
            .and_then(|u| editor.kb.instances.get(&u))
            .or_else(|| editor.kb.instances.get(&kb_id));
        match kb {
            Some(kb) => kb.list_ids(None).into_iter().collect(),
            None => HashSet::new(),
        }
    };
    editor.collab.shared_kbs.insert(kb_id.clone(), node_ids);

    // Phase D (ADR-029): was this a daemon-host share (auto-hosting the
    // primary) rather than a user/peer share? Consume the in-flight marker.
    // Host shares are runtime-only — they must NOT stamp the durable
    // `primary_shared` marker (which would imply peer-share and persist into a
    // later daemon-less launch). The collection + node docs are already on the
    // daemon (the point of hosting); we only refresh the runtime gate.
    if editor.collab.daemon_host_pending.remove(&kb_id) {
        editor.refresh_daemon_host_state();
        tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_count, "host: primary hosted on daemon CRDT (runtime, no durable marker)");
        editor.set_status(format!(
            "KB '{}' hosted on daemon ({} nodes)",
            kb_id, node_count
        ));
    } else {
        // ADR-019: stamp the DURABLE share marker so "this KB syncs" survives
        // editor restart / reconnect (the transient `shared_kbs` set above is
        // only a cache; the emit gate now reads these markers). Persisted to
        // the XDG-first registry. Only on a confirmed share.
        if let Some(dir) = editor.mae_data_dir() {
            let (registry, (), saved) = mae_kb::federation::KbRegistry::update(&dir, |reg| {
                let now = mae_kb::data_dir::chrono_now_iso();
                if kb_id == mae_core::KB_DEFAULT_NAME || kb_id == "primary" {
                    reg.primary_shared = true;
                    reg.primary_collab_id = Some(kb_id.clone());
                } else if let Some(inst) = reg.find_mut(&kb_id) {
                    inst.shared = true;
                    inst.collab_id = Some(kb_id.clone());
                    inst.last_sync = Some(now);
                }
            });
            if let Err(e) = saved {
                warn!(kb = %kb_id, error = %e, "failed to persist shared-KB registry marker");
            }
            editor.kb.registry = registry;
            editor.kb.last_local_registry_write = Some(std::time::Instant::now());
        }
        tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_count, "share: durable marker stamped");
        editor.set_status(format!("KB '{}' shared ({} nodes)", kb_id, node_count));
    }
    refresh_collab_status_if_open(editor);
}

pub(super) fn handle_kb_joined_event(
    editor: &mut Editor,
    kb_id: String,
    collection_state: Vec<u8>,
    nodes: Vec<JoinedNode>,
) {
    let node_count = nodes.len();
    info!(kb = %kb_id, node_count, collection_bytes = collection_state.len(), "KB joined — reconciling into local store");

    // ADR-019 + ADR-020 + ADR-022: register the joined KB as a FIRST-CLASS
    // federated instance (durable markers, addressable, in kb_instances) and
    // RECONCILE each node's CRDT state (state-vector diff + local-ahead push)
    // rather than overwrite — so a member's offline/local edits survive a
    // (re)join even if their pending-queue row was lost in a crash. Per-node
    // decode/reconcile errors are tolerated (skipped + warned) inside the helper.
    let joined_node_ids: HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
    editor.kb_register_joined_instance(&kb_id, nodes);
    // Keep the transient index in sync as a cache.
    editor
        .collab
        .shared_kbs
        .insert(kb_id.clone(), joined_node_ids);

    // ADR-023: learn THIS peer's current authorization epoch for the KB from
    // the (full) collection snapshot, so subsequent node edits are authored
    // under the current-epoch client_id the daemon's fence expects. This is
    // also the post-rebase relearn point: a "rebase required" rejection
    // triggers a re-join, which re-delivers collection_state with the bumped
    // epoch. Absent/0 ⇒ fresh grant ⇒ the legacy base client_id (no change).
    if !editor.collab.local_fingerprint.is_empty() {
        if let Ok(coll) = mae_sync::kb::KbCollectionDoc::from_bytes(&collection_state) {
            let epoch = coll.epoch_of(&editor.collab.local_fingerprint);
            if epoch == 0 {
                editor.collab.kb_epochs.remove(&kb_id);
            } else {
                editor.collab.kb_epochs.insert(kb_id.clone(), epoch);
            }
            debug!(kb = %kb_id, epoch, "learned KB authorization epoch (ADR-023)");
        }
    }

    // C1: seed the local collection replica from the full join snapshot so
    // subsequent live `kbc:` membership broadcasts can be applied as deltas
    // and the epoch relearned without a reconnect.
    editor
        .collab
        .kb_collection_state
        .insert(kb_id.clone(), collection_state);

    info!(kb = %kb_id, node_count, "KB join complete (merged)");
    editor.set_status(format!("Joined KB '{}' ({} nodes)", kb_id, node_count));
    refresh_collab_status_if_open(editor);
    editor.mark_full_redraw();
}

pub(super) fn handle_kb_left_event(editor: &mut Editor, kb_id: String) {
    info!(kb = %kb_id, "left shared KB — local copy preserved");
    // Local KB nodes persist after leaving (local-first principle).
    // Only stop receiving further updates.
    editor.collab.shared_kbs.remove(&kb_id);
    // C1: drop the collection replica + learned epoch — we no longer track
    // this KB's membership (a later rejoin re-seeds from a fresh snapshot).
    editor.collab.kb_collection_state.remove(&kb_id);
    editor.collab.kb_epochs.remove(&kb_id);
    editor.set_status(format!("Left KB '{}' (local copy preserved)", kb_id));
    refresh_collab_status_if_open(editor);
    editor.mark_full_redraw();
}

pub(super) fn handle_kb_node_update_event(
    editor: &mut Editor,
    kb_id: String,
    node_id: String,
    update_bytes: Vec<u8>,
) {
    debug!(
        kb = %kb_id,
        node = %node_id,
        update_len = update_bytes.len(),
        "remote KB node update — applying"
    );
    // ADR-019: route to the OWNING KB (instance or primary), not always
    // primary. The node-id namespace prefix hints the target instance
    // for a node not yet present locally.
    let hint = node_id.split(':').next().filter(|p| !p.is_empty());
    match editor.kb_apply_remote_update(&node_id, &update_bytes, hint) {
        Ok(changed) => {
            if changed {
                debug!(kb = %kb_id, node = %node_id, "KB node content changed by remote update");
                editor.mark_full_redraw();
            }
        }
        Err(e) => {
            warn!(kb = %kb_id, node = %node_id, error = %e, "failed to apply remote KB node update");
        }
    }
}

pub(super) fn handle_kb_update_requeue_event(
    editor: &mut Editor,
    kb_id: String,
    node_id: String,
    update: Vec<u8>,
    pending_rowid: Option<i64>,
) {
    // ADR-020 durability: the bg task couldn't put this kb/node_update on the
    // wire — never silently lost (B-8). A store-backed row still EXISTS (the
    // drain is non-destructive and acks only on daemon-confirm), so we must
    // NOT re-persist it (that would duplicate the row) — just release the
    // in-flight mark so the next drain retries the same row. A transient
    // in-memory update has no durable row, so re-push it to the in-mem queue.
    match pending_rowid {
        Some(rowid) => {
            editor.collab.inflight_kb_updates.remove(&rowid);
            tracing::debug!(target: "kb_sync", kb = %kb_id, node = %node_id, rowid, "requeue: released in-flight durable row for retry");
        }
        None => {
            editor
                .collab
                .pending_kb_updates
                .push((kb_id, node_id, update));
        }
    }
}

pub(super) fn handle_kb_update_acked_event(editor: &mut Editor, rowid: i64) {
    // ADR-020 queue→send→confirm→**ack**: the daemon confirmed apply, so now
    // remove the durable row and clear the in-flight mark.
    editor.collab.inflight_kb_updates.remove(&rowid);
    if let Some(ref store) = editor.kb.store {
        if let Err(e) = store.ack_pending_update(rowid) {
            warn!(target: "kb_sync", rowid, error = %e, "ack: failed to remove confirmed pending kb update");
        } else {
            tracing::debug!(target: "kb_sync", rowid, "ack: durable pending kb update confirmed + removed");
        }
    }
}

pub(super) fn handle_kb_update_failed_event(
    editor: &mut Editor,
    kb_id: String,
    node_id: String,
    rowid: Option<i64>,
    message: String,
) {
    // The daemon rejected the update (e.g. access denied / malformed). Surface
    // loudly and drop the durable row — retrying the identical update would not
    // succeed. (A future ReplicationFailed status surfaces this in the UI.)
    //
    // ADR-023: a "rebase required" rejection is the epoch fence firing — this
    // op was authored under a stale (pre-grant) authorization epoch and can
    // NEVER be accepted as-is (that is the whole point: a pre-grant divergent
    // lineage must not cascade). The security guarantee holds regardless of
    // what we do here (the daemon enforced it); we drop the row so it isn't
    // retried forever, relearn our epoch on the next (re)join, and tell the
    // user their pre-grant edit was not synced and must be re-applied. (The
    // graceful auto-adopt + re-author under the current epoch is tracked as a
    // follow-up; it needs a targeted node-adopt primitive.)
    let is_rebase = is_epoch_fence_rejection(&message);
    if is_rebase {
        warn!(target: "kb_sync", kb = %kb_id, node = %node_id, error = %message, "kb/node_update fenced (stale-epoch) — pre-grant edit not synced (B-19)");
        use mae_core::notifications::{NotifCommand, Notification};
        // P4: `collab_fence_resolution = auto` resolves the fence in the
        // background — adopt the authoritative state + re-author the local
        // edit under the current epoch (the keep-mine path), no prompt.
        // Default `prompt` keeps the user in the loop (#10).
        if editor.collab.fence_resolution == "auto" {
            if editor.notify_collab_keep_mine(&kb_id, &node_id) {
                editor.notify(Notification::info(
                    "collab",
                    format!(
                        "KB '{kb_id}': access changed — re-applying your edit to {node_id} under updated authorization"
                    ),
                ));
            }
            if let Some(rowid) = rowid {
                editor.collab.inflight_kb_updates.remove(&rowid);
                if let Some(ref store) = editor.kb.store {
                    let _ = store.ack_pending_update(rowid);
                }
            }
            editor.mark_full_redraw();
            return;
        }
        // ADR-024 R2: raise an actionable, non-clobberable notification (badge
        // + *Notifications* row) instead of a status line that gets drowned out.
        // The actions invoke the R1 adopt-and-re-author round-trip.
        editor.notify(
            Notification::action_required(
                "collab",
                format!("KB '{kb_id}': your edit to {node_id} wasn't synced"),
            )
            .key(format!("collab:fence:{kb_id}:{node_id}"))
            .body(
                "You edited this node before your access to the KB changed, so the \
                 server rejected the edit. Choose what to do: Accept-remote replaces \
                 your copy with the current shared version (your edit is lost); \
                 Keep-mine re-applies your edit on top of the current version; Stash \
                 externally saves your version to a separate file first.",
            )
            .action(
                "Accept-remote (clobber local)",
                NotifCommand::AdoptRemote {
                    kb_id: kb_id.clone(),
                    node_id: node_id.clone(),
                },
            )
            .action(
                "Keep-mine (re-author)",
                NotifCommand::KeepMine {
                    kb_id: kb_id.clone(),
                    node_id: node_id.clone(),
                },
            )
            .action(
                "Stash externally",
                NotifCommand::StashExternally {
                    kb_id: kb_id.clone(),
                    node_id: node_id.clone(),
                },
            ),
        );
    } else {
        warn!(target: "kb_sync", kb = %kb_id, node = %node_id, error = %message, "kb/node_update failed — dropping");
        editor.set_status(format!("KB sync rejected for {node_id}: {message}"));
    }
    if let Some(rowid) = rowid {
        editor.collab.inflight_kb_updates.remove(&rowid);
        if let Some(ref store) = editor.kb.store {
            let _ = store.ack_pending_update(rowid);
        }
    }
}

pub(super) fn handle_kb_node_adopted_event(
    editor: &mut Editor,
    kb_id: String,
    node_id: String,
    state_bytes: Vec<u8>,
) {
    // ADR-024 R1: replace the local node with the daemon's authoritative
    // state (dropping the fenced stale-epoch op), then — if keep-mine
    // captured the user's edit — re-apply it under the current epoch so it
    // converges as a fresh, authorized op.
    match editor.kb_adopt_node(&node_id, &state_bytes) {
        Ok(_) => {
            let kept = editor
                .collab
                .pending_reauthor
                .remove(&(kb_id.clone(), node_id.clone()));
            if let Some(f) = kept {
                match editor.kb_update_node(&node_id, Some(&f.title), Some(&f.body), Some(f.tags)) {
                    Ok(()) => {
                        editor.notify(mae_core::notifications::Notification::success(
                            "collab",
                            format!("Re-applied your edit to {node_id} under current access"),
                        ));
                    }
                    Err(e) => editor.set_status(format!("Re-author failed for {node_id}: {e}")),
                }
            } else {
                editor.notify(mae_core::notifications::Notification::success(
                    "collab",
                    format!("Adopted authoritative {node_id} (local changes discarded)"),
                ));
            }
            editor.mark_full_redraw();
        }
        Err(e) => {
            editor
                .collab
                .pending_reauthor
                .remove(&(kb_id, node_id.clone()));
            editor.set_status(format!("Adopt failed for {node_id}: {e}"));
        }
    }
}

// --- Drain: pending KB node updates (called from drain_collab_intents) ---

/// Drain pending KB node updates (generated by `kb_update_node` for shared
/// nodes) and the collection-manifest ops queue, forwarding them to the
/// background task. Only sends while connected — updates accumulate while
/// offline. See ADR-020 (queue→send→confirm→ack durability) and Phase D1.1
/// (collection-manifest ops).
pub(super) fn drain_kb_node_updates(editor: &mut Editor, collab_tx: &mpsc::Sender<CollabCommand>) {
    if !matches!(editor.collab.status, CollabStatus::Connected { .. }) {
        return;
    }
    // Durable path: SQLite-persisted updates (survives crashes). Non-destructive
    // read — the row is removed only on daemon-confirmed ack.
    let pending = editor
        .kb
        .store
        .as_ref()
        .and_then(|s| s.drain_pending_updates().ok())
        .unwrap_or_default();
    for pu in pending {
        // Skip rows already on the wire awaiting the daemon's apply-confirmation.
        if !editor.collab.inflight_kb_updates.insert(pu.rowid) {
            continue;
        }
        tracing::debug!(target: "kb_sync", kb_id = %pu.kb_id, node_id = %pu.node_id, rowid = pu.rowid, bytes = pu.update_bytes.len(), "drain: send kb/node_update (durable)");
        let epoch = editor.collab.kb_epochs.get(&pu.kb_id).copied().unwrap_or(0);
        // #168: stamp the authoritative E2e status from the cached collection so the
        // network task fails closed (never plaintext) if it can't seal.
        let e2e = editor
            .collab
            .kb_collection_state
            .get(&pu.kb_id)
            .is_some_and(|s| kb_collection_is_e2e(s));
        let cmd = CollabCommand::KbNodeUpdate {
            kb_id: pu.kb_id,
            node_id: pu.node_id,
            update: pu.update_bytes,
            epoch,
            e2e,
            pending_rowid: Some(pu.rowid),
        };
        if collab_tx.try_send(cmd).is_err() {
            // Couldn't hand off — release the mark so the next tick retries.
            editor.collab.inflight_kb_updates.remove(&pu.rowid);
            warn!("collab command channel full — persisted KB update retried next tick");
        }
    }

    // Fallback path: transient in-memory updates (only populated when there is
    // no durable store). Destructive take — requeued in-memory on send failure.
    let in_mem = editor.collab.pending_kb_updates.len();
    for (kb_id, node_id, update) in std::mem::take(&mut editor.collab.pending_kb_updates) {
        tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, bytes = update.len(), "drain: send kb/node_update (in-mem)");
        let epoch = editor.collab.kb_epochs.get(&kb_id).copied().unwrap_or(0);
        let e2e = editor
            .collab
            .kb_collection_state
            .get(&kb_id)
            .is_some_and(|s| kb_collection_is_e2e(s));
        let cmd = CollabCommand::KbNodeUpdate {
            kb_id,
            node_id,
            update,
            epoch,
            e2e,
            pending_rowid: None,
        };
        if collab_tx.try_send(cmd).is_err() {
            warn!("collab command channel full — KB node update dropped");
        }
    }
    if in_mem > 0 {
        tracing::debug!(target: "kb_sync", count = in_mem, "drain: flushed in-memory kb updates");
    }

    // Phase D1.1: drain collection-manifest ops (created/deleted nodes) →
    // kb/collection_node_*. Best-effort: only sent when connected; creates also
    // self-heal on the reconnect re-share (which rebuilds the full manifest).
    for (kb_id, node_id, title, add) in std::mem::take(&mut editor.collab.pending_kb_manifest) {
        tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, add, "drain: send kb/collection_node");
        // #156 F5: never write a cleartext node title into the manifest of an E2e KB —
        // the key-blind daemon/relay stores the manifest in the clear and would read it.
        // The real title lives encrypted inside the node op-set; an E2e manifest needs
        // only the node_id (the Blocked/list views fall back to it). Downgrade-resistant:
        // `kb_collection_is_e2e` reads the SIGNED op-log, not the relay-flippable flag.
        let title = if add
            && editor
                .collab
                .kb_collection_state
                .get(&kb_id)
                .is_some_and(|s| kb_collection_is_e2e(s))
        {
            String::new()
        } else {
            title
        };
        let cmd = CollabCommand::KbCollectionNode {
            kb_id,
            node_id,
            title,
            add,
        };
        if collab_tx.try_send(cmd).is_err() {
            warn!("collab command channel full — KB manifest op dropped");
        }
    }
}

// --- CollabIntent -> CollabCommand translation (KB-sharing intents) ---

/// Translate a KB-sharing `CollabIntent` into the `CollabCommand` sent to the
/// background task. Returns `None` when the arm bails out early (KB not
/// found, or share-encoding failed) — the caller must skip the send in that
/// case, matching the original arm's early `return`.
pub(super) fn kb_intent_to_command(
    editor: &mut Editor,
    intent: CollabIntent,
) -> Option<CollabCommand> {
    match intent {
        CollabIntent::ShareKb { kb_name, node_ids } => {
            // ADR-020 B-16: establish + persist a canonical CRDT lineage for every
            // shared node (incl. never-edited ones) BEFORE encoding the payload, so
            // the owner's local docs ARE the lineage peers adopt on join — otherwise
            // `to_collection` mints an ephemeral, non-persisted lineage and a peer's
            // later edit no-ops against the owner's divergent local doc (bob→alice).
            editor.kb_prepare_share_lineage(&kb_name, &node_ids);
            // Look up the KB instance: KB_DEFAULT_NAME/"primary" → editor.kb.primary,
            // otherwise resolve a registered instance. `editor.kb.instances` is keyed
            // by UUID, but callers pass a human name (e.g. ":kb-share collabtest"), so
            // map name→uuid via the registry first (find() accepts a name or a uuid).
            // Without this, every named-instance share failed with "KB not found".
            let kb = if kb_name == mae_core::KB_DEFAULT_NAME || kb_name == "primary" {
                Some(&editor.kb.primary)
            } else {
                let uuid = editor
                    .kb
                    .registry
                    .find(&kb_name)
                    .map(|inst| inst.uuid.clone());
                uuid.and_then(|u| editor.kb.instances.get(&u))
                    .or_else(|| editor.kb.instances.get(&kb_name))
            };
            let kb = match kb {
                Some(k) => k,
                None => {
                    editor.set_status(format!("KB '{}' not found", kb_name));
                    return None;
                }
            };
            let creator = editor.collab.user_name.clone();
            let kb_id = kb_name.clone();

            match kb.to_collection(&kb_name, &creator, &node_ids) {
                Ok((coll, node_states)) => {
                    let collection_state = coll.encode_state();
                    let node_count = node_states.len();
                    info!(
                        kb = %kb_id,
                        node_count,
                        collection_bytes = collection_state.len(),
                        "sharing KB: encoded collection + nodes"
                    );
                    Some(CollabCommand::ShareKb {
                        kb_id,
                        name: kb_name,
                        creator,
                        collection_state,
                        node_states,
                    })
                }
                Err(e) => {
                    error!(kb = %kb_name, error = %e, "failed to encode KB for sharing");
                    editor.set_status(format!("Failed to share KB: {}", e));
                    None
                }
            }
        }
        CollabIntent::JoinKb { kb_id, node_svs } => Some(CollabCommand::JoinKb { kb_id, node_svs }),
        CollabIntent::LeaveKb { kb_id } => Some(CollabCommand::LeaveKb { kb_id }),
        CollabIntent::KbAddMember {
            kb_id,
            member,
            role,
        } => Some(CollabCommand::KbMember {
            kb_id,
            member,
            role,
            add: true,
        }),
        CollabIntent::KbRemoveMember { kb_id, member } => {
            // The §D3 rotation (if E2e) is authored on the network task against ITS
            // `kb_collections` replica — not a main-thread snapshot — so no collection
            // bytes ride along (the kb/approve #179 fail-closed rule).
            Some(CollabCommand::KbMember {
                kb_id,
                member,
                role: String::new(),
                add: false,
            })
        }
        CollabIntent::KbApprove {
            kb_id,
            principal,
            role,
        } => Some(CollabCommand::KbApprove {
            kb_id,
            principal,
            role,
        }),
        CollabIntent::KbListPending { kb_id } => Some(CollabCommand::KbListPending { kb_id }),
        CollabIntent::KbSetPolicy { kb_id, policy } => {
            Some(CollabCommand::KbSetPolicy { kb_id, policy })
        }
        CollabIntent::KbSetBlock {
            kb_id,
            member,
            blocked,
        } => Some(CollabCommand::KbBlockPrincipal {
            kb_id,
            principal: member,
            block: blocked,
        }),
        CollabIntent::KbSetEncryption { kb_id, mode } => {
            // Carry the main thread's cached collection replica so the network task (which
            // holds the identity secret) can author the signed genesis + SetEncryption op.
            let collection_state = editor
                .collab
                .kb_collection_state
                .get(&kb_id)
                .cloned()
                .unwrap_or_default();
            // #171: also carry each shared node's plaintext state so the network task can
            // RE-SEAL them under the new content key (it holds the key + the secret).
            let node_states = editor.kb_share_node_states(&kb_id);
            Some(CollabCommand::KbSetEncryption {
                kb_id,
                mode,
                collection_state,
                node_states,
            })
        }
        CollabIntent::KbNodeUpdate {
            kb_id,
            node_id,
            update,
        } => Some(CollabCommand::KbNodeUpdate {
            kb_id,
            node_id,
            update,
            // This intent path is not a live producer (node updates flow via the
            // drain, which stamps the real epoch + e2e); 0 / false are the defaults.
            epoch: 0,
            e2e: false,
            pending_rowid: None,
        }),
        CollabIntent::KbAdoptNode { kb_id, node_id } => {
            Some(CollabCommand::KbAdoptNode { kb_id, node_id })
        }
        other => unreachable!("kb_intent_to_command called with non-KB intent: {other:?}"),
    }
}
