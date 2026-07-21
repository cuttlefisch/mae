//! Split from the monolithic `collab_bridge_tests.rs`: epoch fencing, join requests, node-adopted conflict resolution, doctor/build-sha, transport credentials.

use super::*;

#[test]
fn fence_auto_resolution_reauthors_in_background_without_prompt() {
    // P4: collab_fence_resolution = "auto" resolves a fenced edit silently —
    // captures the local edit for re-author (keep-mine) and queues the adopt,
    // with NO action-required prompt (an Info notice instead).
    let mut editor = Editor::new();
    editor.collab.local_fingerprint = "mefp".to_string();
    editor.collab.fence_resolution = "auto".to_string();
    editor.kb.primary.insert(mae_kb::Node::new(
        "concept:beta",
        "My Title",
        mae_kb::NodeKind::Note,
        "my body",
    ));

    handle_collab_event(
        &mut editor,
        CollabEvent::KbUpdateFailed {
            kb_id: "team".to_string(),
            node_id: "concept:beta".to_string(),
            rowid: None,
            message: "rebase required: node 'concept:beta' carries a stale-epoch op".to_string(),
        },
    );

    // Keep-mine captured the edit for re-author + queued the adopt round-trip.
    assert!(editor
        .collab
        .pending_reauthor
        .contains_key(&("team".to_string(), "concept:beta".to_string())));
    assert!(matches!(
        editor.collab.pending_intent,
        Some(mae_core::CollabIntent::KbAdoptNode { .. })
    ));
    // No action-required prompt was raised (auto = no user interruption).
    assert_eq!(editor.notifications.outstanding_count(), 0);
}
#[test]
fn fence_prompt_raises_action_required_by_default() {
    let mut editor = Editor::new();
    editor.collab.local_fingerprint = "mefp".to_string();
    // default fence_resolution == "prompt"
    handle_collab_event(
        &mut editor,
        CollabEvent::KbUpdateFailed {
            kb_id: "team".to_string(),
            node_id: "concept:beta".to_string(),
            rowid: None,
            message: "rebase required: stale".to_string(),
        },
    );
    assert_eq!(editor.notifications.outstanding_count(), 1);
    assert!(editor.collab.pending_reauthor.is_empty());
}
#[test]
fn owner_notified_of_new_pending_join_request() {
    // P4: when the owner's replica advances with a NEW pending request, raise
    // an action-required notification so the owner isn't blind.
    use mae_sync::kb::KbCollectionDoc;
    let mut editor = Editor::new();
    editor.collab.local_fingerprint = "alicefp".to_string();
    let mut coll = KbCollectionDoc::new_owned("Team", "alicefp", "alice");
    editor
        .collab
        .kb_collection_state
        .insert("team".to_string(), coll.encode_state());

    // A peer's join request arrives as a kbc: broadcast.
    let update = coll.add_pending("carolfp", "carol", "2026-06-23", None, None);
    handle_collab_event(
        &mut editor,
        CollabEvent::RemoteUpdate {
            doc_id: "kbc:team".to_string(),
            update_bytes: update,
            wal_seq: 1,
        },
    );

    assert!(
        editor
            .notifications
            .active_sorted()
            .iter()
            .any(|n| n.source == "collab" && n.title.contains("join request")),
        "owner is notified of the pending request"
    );
}
#[test]
fn kb_shared_seeds_owner_replica_for_introspection() {
    // OQ1: on KbShared the owner seeds a local collection replica from the
    // daemon's authoritative collection, so `kb_sharing_snapshot` can list its
    // OWN KB's members (the owner is otherwise blind to its own KB).
    use mae_sync::kb::KbCollectionDoc;
    let mut editor = Editor::new();
    editor.collab.local_fingerprint = "alicefp".to_string();

    let coll = KbCollectionDoc::new_owned("Team", "alicefp", "alice");
    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: "team".to_string(),
            node_count: 0,
            collection_state: coll.encode_state(),
        },
    );

    assert!(
        editor.collab.kb_collection_state.contains_key("team"),
        "owner replica seeded on share"
    );
    let snap = editor.kb_sharing_snapshot();
    let kb = snap
        .kbs
        .iter()
        .find(|k| k.id == "team")
        .expect("kb present");
    assert_eq!(kb.role_of_me.as_deref(), Some("owner"));
    assert!(kb.is_owner);
    assert!(
        kb.members.iter().any(|m| m.is_me && m.role == "owner"),
        "owner sees itself as a member of its own KB"
    );
}
#[test]
fn kbc_broadcast_relearns_epoch_without_reconnect() {
    // C1: a live `kbc:` collection-doc broadcast that changes THIS peer's role
    // must update its learned authorization epoch in-place — no reconnect/
    // re-join — so the next node edit authors under the rotated client_id.
    use mae_sync::kb::{KbCollectionDoc, Role};

    let mut editor = Editor::new();
    editor.collab.local_fingerprint = "alicefp".to_string();

    // Owner shares; Alice is granted (epoch 0 — a fresh grant doesn't bump).
    let mut coll = KbCollectionDoc::new_owned("teamkb", "ownerfp", "Owner");
    let _ = coll.upsert_member("alicefp", "Alice", Role::Editor);
    // Seed Alice's local replica from the join snapshot.
    editor
        .collab
        .kb_collection_state
        .insert("teamkb".to_string(), coll.encode_state());
    assert_eq!(coll.epoch_of("alicefp"), 0, "fresh grant is epoch 0");

    // Owner demotes Alice (Editor → Viewer): a real role change → epoch bumps to
    // an unpredictable token (#72), so capture it rather than assume prev+1.
    let demote_update = coll.set_role("alicefp", Role::Viewer);
    let alice_epoch = coll.epoch_of("alicefp");
    assert_ne!(
        alice_epoch, 0,
        "role change bumps the epoch off the sentinel"
    );

    // The daemon broadcasts the delta as a `kbc:` RemoteUpdate.
    handle_collab_event(
        &mut editor,
        CollabEvent::RemoteUpdate {
            doc_id: "kbc:teamkb".to_string(),
            update_bytes: demote_update,
            wal_seq: 1,
        },
    );

    assert_eq!(
        editor.collab.kb_epochs.get("teamkb").copied(),
        Some(alice_epoch),
        "epoch relearned live from the broadcast — no reconnect needed"
    );
    assert!(
        editor
            .notifications
            .feed()
            .any(|n| n.source == "collab" && n.title.contains("access changed")),
        "the user is informed their access changed"
    );
}
#[test]
fn kbc_broadcast_cannot_self_elevate_other_members_change_is_ignored() {
    // C1 security non-regression: the relearn derives epoch ONLY from the
    // daemon-authored collection doc. A broadcast that changes a DIFFERENT
    // member must not touch THIS peer's epoch — a client cannot synthesize an
    // epoch bump for itself. (The daemon remains authoritative and fences a
    // client that authors under a stale epoch regardless — see the daemon's
    // viewer_era_* / stale_epoch_continuation_* tests, untouched by C1.)
    use mae_sync::kb::{KbCollectionDoc, Role};

    let mut editor = Editor::new();
    editor.collab.local_fingerprint = "alicefp".to_string();

    let mut coll = KbCollectionDoc::new_owned("teamkb", "ownerfp", "Owner");
    let _ = coll.upsert_member("alicefp", "Alice", Role::Editor);
    let _ = coll.upsert_member("bobfp", "Bob", Role::Editor);
    editor
        .collab
        .kb_collection_state
        .insert("teamkb".to_string(), coll.encode_state());

    // Owner changes BOB's role (not Alice's).
    let bob_update = coll.set_role("bobfp", Role::Viewer);
    assert_ne!(
        coll.epoch_of("bobfp"),
        0,
        "Bob's role change advances his epoch"
    );
    assert_eq!(coll.epoch_of("alicefp"), 0, "Alice's epoch is unchanged");

    handle_collab_event(
        &mut editor,
        CollabEvent::RemoteUpdate {
            doc_id: "kbc:teamkb".to_string(),
            update_bytes: bob_update,
            wal_seq: 1,
        },
    );

    // Alice's epoch stays at 0 — she gained nothing from Bob's change.
    assert!(
        editor.collab.kb_epochs.get("teamkb").copied().unwrap_or(0) == 0,
        "a change to another member must not alter this peer's epoch"
    );
}
#[test]
fn kbc_broadcast_for_unjoined_kb_is_ignored() {
    // C1: a `kbc:` broadcast for a KB we hold no replica of (never joined, or
    // already left) is safely ignored — a bare delta can't apply to nothing.
    let mut editor = Editor::new();
    editor.collab.local_fingerprint = "alicefp".to_string();
    handle_collab_event(
        &mut editor,
        CollabEvent::RemoteUpdate {
            doc_id: "kbc:ghost-kb".to_string(),
            update_bytes: vec![1, 2, 3],
            wal_seq: 1,
        },
    );
    assert!(editor.collab.kb_epochs.is_empty());
    assert!(editor.collab.kb_collection_state.is_empty());
}
#[test]
fn kb_node_adopted_keep_mine_reauthors_over_authoritative() {
    // A2b: the bridge half of the R1 adopt-and-re-author round-trip. After a
    // fence, keep-mine captured the user's fields into `pending_reauthor`; when
    // the daemon's authoritative node state arrives (KbNodeAdopted, the reply
    // to kb/node_fetch), `handle_collab_event` must (1) replace the local node
    // with the authoritative state, then (2) re-apply the kept edit on top so
    // it converges as a fresh, authorized op — and consume the pending entry.
    let mut editor = Editor::new();
    // Local node carries the pre-fence (stale) content.
    editor.kb.primary.insert(mae_kb::Node::new(
        "concept:beta",
        "STALE LOCAL",
        mae_kb::NodeKind::Note,
        "stale local body",
    ));
    // Keep-mine already captured the user's edit for re-author.
    editor.collab.pending_reauthor.insert(
        ("teamkb".to_string(), "concept:beta".to_string()),
        mae_core::editor::ReauthorFields {
            title: "MY KEPT TITLE".to_string(),
            body: "my kept body".to_string(),
            tags: vec!["mine".to_string()],
        },
    );

    // The daemon's authoritative state for the node (a different lineage/value).
    let mut remote = mae_kb::KnowledgeBase::new();
    let state_bytes = remote
        .upsert_with_crdt(
            mae_kb::Node::new(
                "concept:beta",
                "AUTHORITATIVE",
                mae_kb::NodeKind::Note,
                "authoritative body",
            ),
            7,
        )
        .unwrap();

    handle_collab_event(
        &mut editor,
        CollabEvent::KbNodeAdopted {
            kb_id: "teamkb".to_string(),
            node_id: "concept:beta".to_string(),
            state_bytes,
        },
    );

    // The pending re-author entry is consumed.
    assert!(
        editor.collab.pending_reauthor.is_empty(),
        "pending_reauthor must be consumed after adopt"
    );
    // The node materializes to the kept edit, re-authored over the
    // authoritative base (not the stale local, not the bare authoritative).
    let node = editor
        .kb
        .primary
        .get("concept:beta")
        .expect("node present after adopt");
    assert_eq!(node.title, "MY KEPT TITLE");
    assert_eq!(node.body, "my kept body");
    assert_eq!(node.tags, vec!["mine".to_string()]);
}
#[test]
fn kb_node_adopted_accept_remote_takes_authoritative_value() {
    // A2b (accept-remote): with NO pending_reauthor entry, KbNodeAdopted
    // replaces the local node with the authoritative state and discards the
    // local edit — no re-author.
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "concept:beta",
        "STALE LOCAL",
        mae_kb::NodeKind::Note,
        "stale local body",
    ));

    let mut remote = mae_kb::KnowledgeBase::new();
    let state_bytes = remote
        .upsert_with_crdt(
            mae_kb::Node::new(
                "concept:beta",
                "AUTHORITATIVE",
                mae_kb::NodeKind::Note,
                "authoritative body",
            ),
            7,
        )
        .unwrap();

    handle_collab_event(
        &mut editor,
        CollabEvent::KbNodeAdopted {
            kb_id: "teamkb".to_string(),
            node_id: "concept:beta".to_string(),
            state_bytes,
        },
    );

    let node = editor
        .kb
        .primary
        .get("concept:beta")
        .expect("node present after adopt");
    assert_eq!(
        node.title, "AUTHORITATIVE",
        "accept-remote takes the daemon value"
    );
    assert_eq!(node.body, "authoritative body");
}
#[test]
fn build_sha_is_populated() {
    // C3 smoke test: build.rs must embed a non-empty build identifier.
    assert!(!crate::BUILD_SHA.is_empty());
}
#[test]
fn doctor_reports_build_and_warns_on_mismatch() {
    // C3: collab-doctor surfaces the daemon's build and flags an
    // editor↔daemon build mismatch — the "same commit?" check the live
    // two-machine test ran by hand.
    let ctx_match = DoctorContext {
        address: "127.0.0.1:9473".to_string(),
        connected: true,
        server_debug: Some(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "build": crate::BUILD_SHA,
        })),
        ping_latency_ms: Some(1),
        synced_info: vec![],
    };
    let lines = build_doctor_lines(&ctx_match).join("\n");
    assert!(
        lines.contains("Server version:"),
        "doctor must report the server version/build:\n{lines}"
    );
    assert!(
        !lines.contains("Build mismatch"),
        "matching builds must not warn:\n{lines}"
    );

    let ctx_mismatch = DoctorContext {
        server_debug: Some(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "build": "deadbeefcafe",
        })),
        ..ctx_match
    };
    let lines = build_doctor_lines(&ctx_mismatch).join("\n");
    assert!(
        lines.contains("Build mismatch"),
        "a differing daemon build must warn:\n{lines}"
    );
}
#[test]
fn resolve_transport_reads_credentials_live_no_cache() {
    // C2 (collab test-gap plan): the connect transport is resolved from the
    // LIVE editor options (the OptionRegistry-backed `collab.*` fields), with
    // no read-site cache — so `(set-option!)` then a (re)resolve picks up the
    // new value with no tick in between. (The transport is built once at
    // collab-task setup and cached for the task's lifetime; the security-
    // critical runtime-changeable field — host-key policy — is kept live via
    // `host_key_policy_live`. A full per-connect transport rebuild on a
    // runtime auth_mode/tls change is a separate, deferred follow-up.)
    let (tx, _rx) = mpsc::channel::<CollabEvent>(8);
    let mut editor = Editor::new();
    // PSK mode keeps this filesystem-free (no identity load).
    editor.set_option("collab_auth_mode", "psk").unwrap();

    editor.set_option("collab_psk", "first").unwrap();
    let t1 = resolve_client_transport(&editor, &tx);
    assert_eq!(
        t1.plain_psk(),
        Some("first"),
        "transport must reflect the freshly-set PSK"
    );

    editor.set_option("collab_psk", "second").unwrap();
    let t2 = resolve_client_transport(&editor, &tx);
    assert_eq!(
        t2.plain_psk(),
        Some("second"),
        "a re-resolve reads the live PSK — no read-site cache (no apply-drain wait)"
    );

    // Switching auth_mode away from key/psk is likewise read live: "none"
    // still resolves to a Plain transport built from current options.
    editor.set_option("collab_auth_mode", "none").unwrap();
    let t3 = resolve_client_transport(&editor, &tx);
    assert!(
        t3.plain_psk().is_some(),
        "auth_mode is read live at resolve time"
    );
}
#[test]
fn epoch_fence_rejection_classified_from_daemon_message() {
    // Editor↔daemon contract (B-19 regression guard): the daemon embeds
    // "rebase required" in an epoch-fence rejection (collab_handler/mod.rs:1041,
    // node-specific detail appended). The editor MUST still classify such a
    // message as a fence so it raises the actionable ADR-024 notification
    // rather than a generic status line. If the daemon reword breaks this,
    // BOTH the daemon's producer-side tests and this consumer-side test fail,
    // forcing the marker to be updated in lockstep.
    let daemon_msg = "rebase required: node 'concept:beta' carries an op from a stale epoch";
    assert!(
        is_epoch_fence_rejection(daemon_msg),
        "daemon fence message must classify as an epoch fence"
    );
    // The bare marker (and embedded anywhere) classifies.
    assert!(is_epoch_fence_rejection(EPOCH_FENCE_MARKER));
    assert!(is_epoch_fence_rejection(&format!(
        "error: {EPOCH_FENCE_MARKER} (node x)"
    )));
    // Unrelated rejections must NOT be mistaken for a fence (else a real sync
    // error would be silently swallowed into the adopt/keep-mine UX).
    assert!(!is_epoch_fence_rejection(
        "kb sync rejected: node not found"
    ));
    assert!(!is_epoch_fence_rejection("connection reset"));
    assert!(!is_epoch_fence_rejection(""));
}
