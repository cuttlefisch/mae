//! KB content doc-request handlers (`kb/share`, `kb/node_fetch`,
//! `kb/node_update`, `kb/collection_op`, `kb/collection_node_add`/
//! `kb/collection_node_remove`), split out of `collab_handler/mod.rs`'s
//! `handle_doc_request_inner` match (pure code motion — see that module's
//! doc comment).

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_kb_share(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
    session_docs: &mut HashSet<String>,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'kb_id' field".to_string()),
            );
        }
    };
    let name = params["name"].as_str().unwrap_or("").to_string();
    // ADR-018: the client-supplied `creator` is only a DISPLAY label hint —
    // never authoritative. The daemon derives the OWNER from the verified
    // cert principal (key fingerprint) and ignores any claimed identity, so
    // there is no "creator mismatch" failure (the old I-7 reject is gone).
    let creator_hint = params["creator"].as_str().unwrap_or("").to_string();
    let owner_label = auth_label.unwrap_or(creator_hint.as_str()).to_string();
    info!(
        session = session_id, kb_id = %kb_id, name = %name,
        owner = ?auth_principal, owner_label = %owner_label, "kb/share"
    );

    // Decode the collection doc.
    let collection_b64 = match params["collection_state"].as_str() {
        Some(s) => s,
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'collection_state' field".to_string()),
            );
        }
    };
    let collection_bytes = match base64_to_update(collection_b64) {
        Ok(b) => b,
        Err(e) => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error(format!("invalid collection_state base64: {e}")),
            );
        }
    };
    // ADR-018 strict binding: bind the OWNER = the AUTHENTICATED principal
    // (key fingerprint), overriding whatever the client baked in. A
    // self-claimed creator is simply ignored — never rejected. `none` mode
    // (no principal) keeps the client collection as-is (loopback trust).
    let collection_bytes = match auth_principal {
        Some(principal) => match KbCollectionDoc::from_bytes(&collection_bytes) {
            Ok(mut coll) => {
                coll.set_owner(principal, &owner_label);
                coll.encode_state()
            }
            Err(e) => {
                warn!(
                    kb_id = %kb_id,
                    error = %e,
                    "kb/share: owner-binding decode failed; using client collection unbound"
                );
                collection_bytes
            }
        },
        None => collection_bytes,
    };
    let collection_doc = format!("kbc:{kb_id}");
    // ADR-020 B-12: an owner reconnect/re-share must NOT clobber the daemon's
    // authoritative collection. It holds the durable membership — approved
    // members, roles, pending requests (set via kb/approve_member /
    // kb/add_member) — that the owner's LOCAL collection copy does not carry,
    // plus the collection's CRDT lineage. `share_doc` is destructive
    // (delete+replace), so re-sharing the owner-only copy silently revoked
    // every trusted member. PRESERVE an existing collection; create it only
    // on the FIRST share. Membership changes flow through the dedicated
    // member/approve/policy methods, never through re-share.
    if !doc_store.has_doc(&collection_doc).await {
        if let Err(e) = doc_store
            .share_doc(&collection_doc, &collection_bytes)
            .await
        {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("failed to share collection doc: {e}")),
            );
        }
    } else {
        info!(
            session = session_id,
            kb_id = %kb_id,
            "kb/share: collection exists — preserving daemon-side membership (B-12)"
        );
    }
    session_docs.insert(collection_doc.clone());
    // ADR-020 liveness: count the sharer as a connected client so the
    // collection doc isn't idle-evicted while the owner stays connected.
    let _ = doc_store.track_client_connect(&collection_doc).await;
    broadcaster
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .subscribe_doc(session_id, &collection_doc);

    // Transport exposure (ADR-018/025): the `transport` param (hub|p2p|both,
    // default hub) WIDENS the KB's policy from its stored value, so a first
    // p2p-share is P2p-only while `kb-share` + `kb-share-p2p` becomes Both
    // (unlike membership, transport is owner-set metadata — safe on
    // re-share). This is how `kb-share-p2p` *establishes* a mesh-exposed KB.
    let requested_transport = params["transport"]
        .as_str()
        .and_then(mae_sync::kb::TransportPolicy::parse)
        .unwrap_or(mae_sync::kb::TransportPolicy::Hub);
    if let Ok(mut coll) = load_collection(doc_store, &kb_id).await {
        let raw = coll.transport_policy_raw();
        let new = raw.map_or(requested_transport, |c| c.union(requested_transport));
        if Some(new) != raw {
            let update = coll.set_transport_policy(new);
            if let Err(e) = persist_and_broadcast_collection(
                doc_store,
                broadcaster,
                session_id,
                &kb_id,
                &update,
            )
            .await
            {
                warn!(kb_id = %kb_id, error = %e, "kb/share: failed to set transport policy");
            } else {
                info!(kb_id = %kb_id, transport = %new.as_str(), "kb/share: transport exposure set");
            }
        }
    }

    // Store each node doc.
    let nodes = params["nodes"].as_array();
    let mut node_count: u64 = 0;
    if let Some(node_arr) = nodes {
        for node in node_arr {
            let node_id = match node["id"].as_str() {
                Some(s) => s,
                None => continue,
            };
            let state_b64 = match node["state"].as_str() {
                Some(s) => s,
                None => continue,
            };
            let state_bytes = match base64_to_update(state_b64) {
                Ok(b) => b,
                Err(e) => {
                    warn!(session = session_id, node_id, error = %e, "kb/share: skipping node with invalid base64");
                    continue;
                }
            };
            let node_doc = format!("kb:{node_id}");
            // ADR-020 B-12: MERGE onto an existing node instead of
            // delete+replace, so an owner re-share does not reset the node's
            // CRDT lineage or clobber peer edits the owner hasn't seen yet.
            // Post-B-16 the owner's lineage is stable + persisted, so this is
            // a clean idempotent merge. Create fresh only on the first share.
            let node_res = if doc_store.has_doc(&node_doc).await {
                doc_store
                    .apply_update(&node_doc, &state_bytes, None)
                    .await
                    .map(|_| ())
            } else {
                doc_store
                    .share_doc(&node_doc, &state_bytes)
                    .await
                    .map(|_| ())
            };
            if let Err(e) = node_res {
                warn!(session = session_id, node_id, error = %e, "kb/share: failed to share node doc");
                continue;
            }
            session_docs.insert(node_doc.clone());
            let _ = doc_store.track_client_connect(&node_doc).await;
            broadcaster
                .lock()
                .unwrap()
                .subscribe_doc(session_id, &node_doc);
            node_count += 1;
        }
    }

    // Store KB metadata.
    let meta = serde_json::json!({
        "kb_id": kb_id,
        "name": name,
        "owner": auth_principal.unwrap_or(""),
        "owner_label": owner_label,
        "node_count": node_count,
        "shared_by": session_id,
    });
    doc_store.set_kb_meta(&kb_id, meta).await;

    info!(session = session_id, kb_id = %kb_id, node_count, "kb/share: complete");
    // Return the AUTHORITATIVE collection state (post-set_owner, or the
    // preserved one on re-share — B-12) so the owner can seed a local
    // collection replica and introspect its own KB's members/roles/policy
    // on the same CRDT lineage future `kbc:` broadcasts advance (C1).
    let owner_collection = doc_store
        .encode_state_and_sv(&collection_doc)
        .await
        .map(|(state, _sv)| update_to_base64(&state))
        .unwrap_or_default();
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "kb_id": kb_id,
            "shared": true,
            "node_count": node_count,
            "collection_state": owner_collection,
        }),
    )
}

// ADR-024 R1: fetch a single node's authoritative state so a fenced member
// can ADOPT it (drop its stale-epoch divergence, ADR-023) + re-author. The
// editor's local doc still carries the rejected op, so it cannot self-adopt.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_kb_node_fetch(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_principal: Option<&str>,
    session_docs: &mut HashSet<String>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'kb_id' field".to_string()),
            );
        }
    };
    let node_id = match params["node_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'node_id' field".to_string()),
            );
        }
    };
    info!(session = session_id, kb_id = %kb_id, node_id = %node_id, "kb/node_fetch");

    // Read access suffices (members only — a non-member is denied).
    match kb_access(doc_store, &kb_id, auth_principal, KbOp::Read, transport).await {
        Ok(AccessDecision::Allow) | Ok(AccessDecision::AllowAutoJoin) => {}
        Ok(AccessDecision::Deny(msg)) | Err(msg) => {
            warn!(session = session_id, kb_id = %kb_id, reason = %msg, "kb/node_fetch denied");
            return JsonRpcResponse::error(id, McpError::internal_error(msg));
        }
        Ok(_) => {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("cannot read KB '{kb_id}'")),
            );
        }
    }

    let node_doc = format!("kb:{node_id}");
    match doc_store.encode_state_and_sv(&node_doc).await {
        Ok((state, sv)) => {
            // Keep the member live-subscribed to this node going forward.
            if session_docs.insert(node_doc.clone()) {
                let _ = doc_store.track_client_connect(&node_doc).await;
                broadcaster
                    .lock()
                    .unwrap()
                    .subscribe_doc(session_id, &node_doc);
            }
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "kb_id": kb_id,
                    "node_id": node_id,
                    "state": update_to_base64(&state),
                    "sv": update_to_base64(&sv),
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            id,
            McpError::internal_error(format!("node fetch failed for '{node_id}': {e}")),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_kb_node_update(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_principal: Option<&str>,
    session_docs: &mut HashSet<String>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'kb_id' field".to_string()),
            );
        }
    };
    let node_id = match params["node_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'node_id' field".to_string()),
            );
        }
    };
    let update_b64 = match params["update"].as_str() {
        Some(s) => s,
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'update' field".to_string()),
            );
        }
    };
    let update_bytes = match base64_to_update(update_b64) {
        Ok(b) => b,
        Err(e) => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error(format!("invalid base64: {e}")),
            );
        }
    };
    // ADR-037 (#171) RESEAL-AS-REPLACE: when E2E is enabled on a
    // previously-plaintext KB, the owner reseals each node as a FRESH op-set and
    // sets `reseal:true` so we PURGE the pre-enable plaintext lineage — `share_doc`
    // atomically deletes the `kb:{node}` snapshot+WAL and recreates it from this
    // ciphertext-only op (vs `apply_update`, which would stack the op-set on top of
    // the readable plaintext). It is OWNER-gated (Manage) and bypasses the epoch
    // fence (a full-doc replace, not a merge against the daemon's node SV).
    let reseal = params["reseal"].as_bool().unwrap_or(false);
    // ADR-020 traceability: log on ENTRY so a received kb/node_update is
    // greppable on the daemon (distinguishes "never arrived" from "rejected").
    info!(
        session = session_id,
        kb_id = %kb_id,
        node_id = %node_id,
        update_len = update_bytes.len(),
        "kb/node_update: received"
    );
    let max_update = doc_store.max_update_size();
    if update_bytes.len() > max_update {
        return JsonRpcResponse::error(
            id,
            McpError::parse_error(format!(
                "update too large: {} bytes (max {})",
                update_bytes.len(),
                max_update
            )),
        );
    }
    info!(session = session_id, kb_id = %kb_id, node_id = %node_id, update_len = update_bytes.len(), "kb/node_update");

    // ADR-018: editing a node requires editor/owner role. Viewers and
    // non-members are denied (least privilege). A RESEAL replaces the whole node
    // doc, so it requires Manage (owner-only) — a mere Editor cannot purge/replace
    // another's node lineage.
    let required_op = if reseal { KbOp::Manage } else { KbOp::Edit };
    // Perf: the `kbc:{kb_id}` collection doc is NOT mutated by this handler
    // arm (only the `kb:{node}` doc is), so load it ONCE and share the
    // snapshot across all four gates below instead of re-encoding+decoding
    // it 4× (kb_access / resolve_content_anchor / verify_content_op /
    // enforce_epoch_fence). `None` (a failed load) makes each gate load
    // itself, reproducing its exact original semantics.
    let pre_coll = load_collection(doc_store, &kb_id).await.ok();
    match kb_access_with_coll(
        doc_store,
        &kb_id,
        auth_principal,
        required_op,
        transport,
        pre_coll.as_ref(),
    )
    .await
    {
        Ok(AccessDecision::Allow) => {}
        Ok(AccessDecision::Deny(msg)) | Err(msg) => {
            warn!(session = session_id, kb_id = %kb_id, reseal, reason = %msg, "kb/node_update denied");
            return JsonRpcResponse::error(id, McpError::internal_error(msg));
        }
        Ok(_) => {
            warn!(session = session_id, kb_id = %kb_id, "kb/node_update denied");
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("cannot edit KB '{kb_id}'")),
            );
        }
    }

    let node_doc = format!("kb:{node_id}");

    // ADR-036 §D3 SIGNED CONTENT OP VERIFICATION. If the op carries a signed
    // authorship header, the content AUTHOR (from the *signed header*, not the
    // connection principal) must be a current Editor+ member at the op's
    // epoch, and the signature must bind that author to this exact
    // (kb_id, node_id, payload). This is what makes a *relayed* edit
    // peer-verifiable: a hostile relay can neither forge an edit nor
    // mis-attribute one (the node_id is signed, so a header claiming a
    // different node than it carries fails `verify_signed`). Resolved via
    // `resolve_content_anchor`, which anchors on the registered op-log key
    // for a JOINED KB AND on this daemon's own signer key for an OWNED KB —
    // #255: an owned KB that is ALSO mesh-shared must attach the header to
    // its OWNER's signed ops, or the owner's edit reaches a mesh-joined
    // member stripped of its signature and is rejected by the member's
    // require-signed relay gate (`verify_relayed_content_op`, which already
    // uses the SAME resolver — the send side was the lone asymmetry). An
    // unsigned op (no header) falls through to the legacy epoch fence. When
    // verified, the authorship header is carried into the broadcast (and thus
    // the dialer's relay to peers) so a downstream peer re-verifies the same
    // op — `verify_content_op` is the single check shared with the mesh relay
    // path (principle #8).
    // A JOINED KB (an external `kb_anchor` is registered) has NO authority
    // beyond its signed op-log, so a signed op is verified STRICTLY and a bad
    // one is hard-rejected. An OWNED KB is additionally authenticated by the
    // trusted local connection (the sender already passed `kb_access` as the
    // owner), so `resolve_content_anchor` falls back to the owner's own key and
    // we attach the header for mesh relay WHEN the op verifies — but on a verify
    // miss (e.g. the genesis owner-self-admit isn't seeded yet, so the owner
    // isn't yet a *derived* member) we fall through to the legacy epoch fence
    // rather than reject a trusted local edit. #255: without this attach, an
    // owned-AND-mesh-shared KB relays the owner's signed edit STRIPPED of its
    // header, and the mesh-joined member rejects it as unsigned.
    let mut content_header: Option<serde_json::Value> = None;
    let joined_anchor = doc_store.kb_anchor(&kb_id).await; // Some ⇒ joined ⇒ strict
    let content_anchor = match joined_anchor {
        Some(a) => Some(a),
        None => resolve_content_anchor_with_coll(doc_store, &kb_id, pre_coll.as_ref()).await, // owned fallback (lenient)
    };
    if let Some(anchor) = content_anchor {
        if let Some(signed) = SignedContentOp::from_params(params, update_bytes.clone()) {
            match verify_content_op_with_coll(
                doc_store,
                &kb_id,
                &anchor,
                &signed,
                pre_coll.as_ref(),
            )
            .await
            {
                Ok(()) => {
                    info!(
                        session = session_id, kb_id = %kb_id, node_id = %node_id,
                        author = %signed.op.author,
                        "kb/node_update: signed content op verified (ADR-036)"
                    );
                    content_header = Some(signed.header_params());
                }
                Err(e) if joined_anchor.is_some() => {
                    warn!(
                        session = session_id, kb_id = %kb_id, node_id = %node_id,
                        author = %signed.op.author, reason = %e,
                        "kb/node_update: SIGNED CONTENT OP REJECTED (ADR-036)"
                    );
                    return JsonRpcResponse::error(id, McpError::internal_error(e));
                }
                Err(e) => {
                    // Owned KB, trusted local connection: don't hard-reject a
                    // not-yet-verifiable signed op — fall through to the legacy
                    // epoch fence with no header (pre-#255 behavior preserved).
                    debug!(
                        session = session_id, kb_id = %kb_id, node_id = %node_id,
                        reason = %e,
                        "kb/node_update: owned-KB signed op not yet verifiable; legacy gate"
                    );
                }
            }
        }
    }

    // ADR-023 (B-19) EPOCH FENCE — the security core. `kb_access` above
    // confirmed the sender's *current* role permits editing, but a granted
    // member must author under their *current-epoch* client_id. Decode the
    // update's NEW ops (those beyond the daemon's authoritative node SV) and
    // reject any authored under a stale-epoch client_id. That is precisely a
    // member's pre-grant divergent lineage (e.g. viewer-era edits) trying to
    // cascade after a grant — denied here, at the daemon, independent of any
    // (possibly malicious) client behaviour. Only enforced in key-auth mode:
    // `none`/loopback has no per-identity principal to derive an epoch from.
    // A reseal REPLACES the doc (fresh op-set at clock 0) — there is no prior-SV
    // merge to fence, and the owner's op is authored under its current epoch
    // anyway, so the fence is skipped for it (owner-gated above).
    if !reseal {
        if let Some(principal) = auth_principal {
            if let Err(reason) = enforce_epoch_fence_with_coll(
                doc_store,
                &kb_id,
                &node_id,
                &node_doc,
                &update_bytes,
                principal,
                pre_coll.as_ref(),
            )
            .await
            {
                warn!(
                    session = session_id, kb_id = %kb_id, node_id = %node_id, %reason,
                    "kb/node_update: epoch fence rejected (B-19)"
                );
                return JsonRpcResponse::error(id, McpError::internal_error(reason));
            }
        }
    }

    // ADR-020 liveness: an editing session keeps its node doc alive (count
    // it as a connected client on first touch) so the doc the member is
    // actively editing isn't idle-evicted out from under them.
    if session_docs.insert(node_doc.clone()) {
        let _ = doc_store.track_client_connect(&node_doc).await;
        broadcaster
            .lock()
            .unwrap()
            .subscribe_doc(session_id, &node_doc);
    }
    // #171: a reseal PURGES + replaces (delete plaintext snapshot+WAL, recreate
    // from this ciphertext-only op); an ordinary edit MERGES into the CRDT.
    let applied = if reseal {
        doc_store.share_doc(&node_doc, &update_bytes).await
    } else {
        doc_store.apply_update(&node_doc, &update_bytes, None).await
    };
    match applied {
        Ok(result) => {
            // Broadcast to other subscribers of the collection.
            {
                let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
                bc.broadcast_except(
                    &EditorEvent::SyncUpdate {
                        buffer_name: node_doc.clone(),
                        update_base64: update_to_base64(&result.update),
                        wal_seq: result.wal_seq,
                        // Relay the verified authorship header so a peer daemon
                        // re-verifies this op (ADR-036 mesh propagation).
                        content_header,
                    },
                    session_id,
                );
            }
            info!(session = session_id, kb_id = %kb_id, node_id = %node_id, wal_seq = result.wal_seq, reseal, "kb/node_update: applied");
            JsonRpcResponse::success(id, serde_json::json!({ "applied": true }))
        }
        Err(e) => JsonRpcResponse::error(
            id,
            McpError::internal_error(format!("failed to apply node update: {e}")),
        ),
    }
}

pub(super) async fn handle_kb_collection_op(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_principal: Option<&str>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    // ADR-037 Phase 3a: the editor's outbound collection-write path. The owner
    // authors a signed collection delta LOCALLY (a signed membership op, an
    // encryption-flag flip, …) and ships the opaque bytes here. The daemon stays
    // KEY-BLIND: it never inspects op semantics nor holds a content key — it only
    // confirms owner authority (Manage) and stores + rebroadcasts the
    // owner-signed bytes. `append_signed_op` (which produced them) does not
    // validate; every peer DERIVES validity from the signed log. This is the
    // only way to author signed membership for an EDITOR-owned KB while keeping
    // the daemon unable to read content (the owner, not the daemon, holds the key).
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'kb_id' field".to_string()),
            );
        }
    };
    let update = match params["update"].as_str() {
        Some(s) => match base64_to_update(s) {
            Ok(b) => b,
            Err(e) => {
                return JsonRpcResponse::error(
                    id,
                    McpError::parse_error(format!("invalid 'update' base64: {e}")),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'update' field".to_string()),
            );
        }
    };
    let max_update = doc_store.max_update_size();
    if update.len() > max_update {
        return JsonRpcResponse::error(
            id,
            McpError::internal_error(format!("collection update exceeds {max_update} bytes")),
        );
    }
    // ADR-018: collection ops are owner-only (Manage) — with ONE narrow
    // exception (ADR-040 PR2c/PR3): a non-owner member managing their OWN identity
    // (self-rotation, recovery-key registration, or recovery rotation). Probe Manage
    // first; if it denies, accept the update IFF it is exactly such a self-service op
    // (`verify_member_self_service_update`) and nothing else.
    let manage = kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await;
    // The `(successor, predecessor)` pairs of an accepted rotation/recovery — used to
    // mirror each successor into the roster below. On the OWNER path (#265) these are
    // the owner's own self-`Rebind`s; on the member path they are the verified member
    // self-service rebinds; a bare recovery-key registration contributes none.
    let rebind_pairs: Vec<(String, String)> = match manage {
        Ok(AccessDecision::Allow) => {
            // #265: an OWNER's self-rotation reaches HERE (Manage = Allow), and its
            // `Rebind` must ALSO be mirrored into the roster — otherwise on an
            // un-anchored (owned/hub) KB the owner is locked out of their own KB after
            // reconnecting under the new key (`role_of(new_fp) = None`). The member
            // branch below already does this for non-owner rotations; do the same for
            // the owner. (Scans only the caller's own self-`Rebind`s — see the helper.)
            owner_self_rebind_pairs(doc_store, &kb_id, auth_principal.unwrap_or(""), &update).await
        }
        other => {
            match verify_member_self_service_update(doc_store, &kb_id, auth_principal, &update)
                .await
            {
                Ok(pairs) => {
                    info!(
                        session = session_id,
                        kb_id = %kb_id,
                        principal = auth_principal.unwrap_or("?"),
                        rebinds = pairs.len(),
                        "kb/collection_op: accepted a member self-service identity op (ADR-040 PR2c/PR3)"
                    );
                    pairs
                }
                Err(rebind_reason) => {
                    let base = match other {
                        Ok(AccessDecision::Deny(m)) | Err(m) => m,
                        _ => format!("not authorized to manage KB '{kb_id}'"),
                    };
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!(
                            "{base} (and not a member self-service identity op: {rebind_reason})"
                        )),
                    );
                }
            }
        }
    };
    // #156 F5: the owner sets `scrub` on the enable-time manifest-title-blank op so
    // the daemon force-COMPACTS the `kbc:` doc after applying — re-snapshotting the
    // title-blanked state and trimming + TRUNCATE-checkpointing the WAL (secure_delete
    // zeroes the freed pages), so the pre-enable cleartext title cannot linger in the
    // `kbc:` WAL at rest. (The manifest doc is mutated in place, not replaced like a
    // node doc under #171.) Additive flag — an old daemon ignores it.
    let scrub = params["scrub"].as_bool().unwrap_or(false);
    match persist_and_broadcast_collection(doc_store, broadcaster, session_id, &kb_id, &update)
        .await
    {
        Ok(wal_seq) => {
            if scrub {
                let coll_doc = format!("kbc:{kb_id}");
                if let Err(e) = doc_store.compact_doc(&coll_doc).await {
                    warn!(session = session_id, kb_id = %kb_id, error = %e, "kb/collection_op: scrub compaction failed (title may linger in the WAL until next compaction)");
                }
            }
            // ADR-040 PR2c/PR3: a member self-rotation or recovery rotation only appends
            // the `Rebind` to the op-log (a bare recovery-key registration appends none).
            // On a roster-model (owned/un-anchored) daemon, mirror each successor into the
            // `member_roles` roster with the PREDECESSOR's role so
            // it gains access here too — parallel to how `kb/add_member` mirrors
            // roster↔op-log. Additive: the predecessor is left in place (no lockout of
            // the rotating session); the user removes the old key explicitly. Derive
            // peers already alias + retire via the PR2a post-pass. Key-blind (the
            // roster holds no secrets).
            if !rebind_pairs.is_empty() {
                if let Ok(mut coll) = load_collection(doc_store, &kb_id).await {
                    for (successor, predecessor) in &rebind_pairs {
                        let role = coll.role_of(predecessor).unwrap_or(SyncRole::Editor);
                        let u = coll.upsert_member(successor, successor, role);
                        if let Err(e) = persist_and_broadcast_collection(
                            doc_store,
                            broadcaster,
                            session_id,
                            &kb_id,
                            &u,
                        )
                        .await
                        {
                            warn!(session = session_id, kb_id = %kb_id, error = %e, "kb/collection_op: failed to mirror rotated successor into the roster");
                        }
                    }
                }
            }
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "applied": true, "wal_seq": wal_seq, "scrubbed": scrub }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
    }
}

// Phase D1.1 (ADR-029): add/remove a node in a KB's collection manifest
// (`kbc:{kb_id}`). The projector materializes manifest membership into cozo, so
// this is how a *created* node joins the daemon's projection and a *deleted* one
// leaves it (the node doc itself rides `kb/node_update`). Daemon computes the
// collection update server-side (mirrors `kb/add_member`); authorized at Editor
// level (KbOp::Edit) like a node edit, and broadcast to other subscribers.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_kb_collection_node_add_remove(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_principal: Option<&str>,
    transport: Transport,
    method: &str,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let add = method == "kb/collection_node_add";
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'kb_id' field".to_string()),
            );
        }
    };
    let node_id = match params["node_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'node_id' field".to_string()),
            );
        }
    };
    let title = params["title"].as_str().unwrap_or("");
    match kb_access(doc_store, &kb_id, auth_principal, KbOp::Edit, transport).await {
        Ok(AccessDecision::Allow) => {}
        Ok(AccessDecision::Deny(m)) | Err(m) => {
            return JsonRpcResponse::error(id, McpError::internal_error(m));
        }
        Ok(_) => {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("not authorized to edit KB '{kb_id}'")),
            );
        }
    }
    let mut coll = match load_collection(doc_store, &kb_id).await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
    };
    let update = if add {
        coll.add_node(&node_id, title)
    } else {
        coll.remove_node(&node_id)
    };
    match persist_and_broadcast_collection(doc_store, broadcaster, session_id, &kb_id, &update)
        .await
    {
        Ok(_) => {
            info!(session = session_id, kb_id = %kb_id, node_id = %node_id, add, "kb collection-manifest change");
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "kb_id": kb_id, "node_id": node_id, "added": add }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
    }
}
