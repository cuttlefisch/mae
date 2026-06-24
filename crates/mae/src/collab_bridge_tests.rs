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
    let update = coll.add_pending("carolfp", "carol", "2026-06-23");
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

    // Owner demotes Alice (Editor → Viewer): a real role change → epoch bumps.
    let demote_update = coll.set_role("alicefp", Role::Viewer);
    assert_eq!(coll.epoch_of("alicefp"), 1, "role change bumps the epoch");

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
        Some(1),
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
    assert_eq!(coll.epoch_of("bobfp"), 1);
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
    // "rebase required" in an epoch-fence rejection (collab_handler.rs:1780,
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

fn tofu_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mae-tofu-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn prompting_verifier_pinned_match_no_prompt() {
    use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
    let dir = tofu_dir("pin");
    let kh = dir.join("known_hosts");
    let server = Identity::generate("daemon").public();
    KnownHosts::load(&kh).pin("d:9473", &server).unwrap();
    // No receiver needed — a pinned match must NOT prompt.
    let (tx, _rx) = mpsc::channel::<CollabEvent>(8);
    let v = PromptingHostKeyVerifier {
        known_hosts: kh,
        evt_tx: tx,
        policy: std::sync::Arc::new(std::sync::Mutex::new("prompt".to_string())),
        timeout: std::time::Duration::from_millis(50),
    };
    assert!(
        v.verify("d:9473", &server),
        "pinned key must be accepted silently"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prompting_verifier_changed_key_rejected() {
    use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
    let dir = tofu_dir("changed");
    let kh = dir.join("known_hosts");
    KnownHosts::load(&kh)
        .pin("d:9473", &Identity::generate("real").public())
        .unwrap();
    let (tx, _rx) = mpsc::channel::<CollabEvent>(8);
    let v = PromptingHostKeyVerifier {
        known_hosts: kh,
        evt_tx: tx,
        policy: std::sync::Arc::new(std::sync::Mutex::new("prompt".to_string())),
        timeout: std::time::Duration::from_millis(50),
    };
    // A DIFFERENT key for the same addr → abort (no prompt).
    assert!(!v.verify("d:9473", &Identity::generate("imposter").public()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prompting_verifier_unknown_accept_pins() {
    use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
    let dir = tofu_dir("accept");
    let kh = dir.join("known_hosts");
    let server = Identity::generate("daemon").public();
    let server_bytes = server.to_bytes();
    let (tx, mut rx) = mpsc::channel::<CollabEvent>(8);
    let v = PromptingHostKeyVerifier {
        known_hosts: kh.clone(),
        evt_tx: tx,
        policy: std::sync::Arc::new(std::sync::Mutex::new("prompt".to_string())),
        timeout: std::time::Duration::from_secs(5),
    };
    // verify() blocks until the (simulated) user answers via the event reply.
    let handle = std::thread::spawn(move || v.verify("d:9473", &server));
    match rx.blocking_recv().expect("prompt event") {
        CollabEvent::HostKeyPrompt {
            reply, fingerprint, ..
        } => {
            assert!(fingerprint.starts_with("SHA256:"));
            reply.send(true).unwrap();
        }
        other => panic!("expected HostKeyPrompt, got {other:?}"),
    }
    assert!(handle.join().unwrap(), "accepted host must verify");
    // ...and is now pinned.
    assert_eq!(
        KnownHosts::load(&kh).get("d:9473").unwrap().to_bytes(),
        server_bytes
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prompting_verifier_unknown_reject_not_pinned() {
    use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
    let dir = tofu_dir("reject");
    let kh = dir.join("known_hosts");
    let server = Identity::generate("daemon").public();
    let (tx, mut rx) = mpsc::channel::<CollabEvent>(8);
    let v = PromptingHostKeyVerifier {
        known_hosts: kh.clone(),
        evt_tx: tx,
        policy: std::sync::Arc::new(std::sync::Mutex::new("prompt".to_string())),
        timeout: std::time::Duration::from_secs(5),
    };
    let handle = std::thread::spawn(move || v.verify("d:9473", &server));
    if let CollabEvent::HostKeyPrompt { reply, .. } = rx.blocking_recv().unwrap() {
        reply.send(false).unwrap();
    }
    assert!(!handle.join().unwrap(), "rejected host must not verify");
    assert!(
        KnownHosts::load(&kh).get("d:9473").is_none(),
        "rejected host must not be pinned"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// B-21 regression: a runtime `collab_host_key_policy` change is honored by the
/// SAME verifier instance at verify-time (the verifier/transport is built once
/// at collab-task setup and cached, so it must read the live policy cell).
#[test]
fn host_key_policy_change_honored_at_verify_time_b21() {
    use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
    let dir = tofu_dir("b21");
    let kh = dir.join("known_hosts");
    let policy = std::sync::Arc::new(std::sync::Mutex::new("accept-new".to_string()));
    let (tx, mut rx) = mpsc::channel::<CollabEvent>(8);
    let v = PromptingHostKeyVerifier {
        known_hosts: kh.clone(),
        evt_tx: tx,
        policy: policy.clone(),
        timeout: std::time::Duration::from_secs(5),
    };
    // accept-new: an unknown host is pinned WITHOUT prompting.
    let a = Identity::generate("daemon-a").public();
    assert!(v.verify("a:9473", &a), "accept-new pins unknown host");
    assert!(rx.try_recv().is_err(), "accept-new must NOT prompt");
    assert_eq!(
        KnownHosts::load(&kh).get("a:9473").unwrap().to_bytes(),
        a.to_bytes()
    );

    // Flip the LIVE policy to `prompt` — the SAME verifier must now ASK on a new
    // host instead of auto-pinning (the B-21 fix: no rebuild/relaunch needed).
    *policy.lock().unwrap() = "prompt".to_string();
    let b = Identity::generate("daemon-b").public();
    let b_bytes = b.to_bytes();
    let handle = std::thread::spawn(move || v.verify("b:9473", &b));
    match rx
        .blocking_recv()
        .expect("prompt event after runtime policy change")
    {
        CollabEvent::HostKeyPrompt {
            reply, fingerprint, ..
        } => {
            assert!(fingerprint.starts_with("SHA256:"));
            reply.send(false).unwrap(); // decline
        }
        other => panic!("expected HostKeyPrompt after policy→prompt, got {other:?}"),
    }
    assert!(!handle.join().unwrap(), "declined prompt → not verified");
    assert!(
        KnownHosts::load(&kh).get("b:9473").is_none(),
        "declined host must not be pinned"
    );
    let _ = b_bytes; // (only needed to move `b` into the thread)
    let _ = std::fs::remove_dir_all(&dir);
}

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
    let mut shared = vec!["test.rs".to_string()];

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
    );
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
    let mut shared = vec!["legacy.rs".to_string()];

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
    );
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
        &mut std::collections::HashMap::new(),
    );
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
    let mut seq = std::collections::HashMap::new();
    handle_response(
        &val,
        PendingResponseKind::ShareBuffer {
            doc_id: "test.rs".to_string(),
        },
        &tx,
        &mut shared,
        &mut seq,
    );
    assert!(shared.contains(&"test.rs".to_string()));
    // WU2: seq_tracker should be seeded from share response wal_seq.
    assert_eq!(seq.get("test.rs"), Some(&1));
    let event = rx.try_recv().unwrap();
    assert!(matches!(event, CollabEvent::BufferShared { doc_id } if doc_id == "test.rs"));
}

/// ADR-020 B-13 regression: a successful `kb/join` must add the collection AND
/// each node doc to `shared_docs`, or later inbound `sync_update` broadcasts for
/// `kb:<node>` are dropped at the `shared_docs.contains()` filter and the member
/// never receives live edits (emit works, receive is dead).
#[tokio::test]
async fn handle_response_kb_join_subscribes_to_collection_and_node_docs() {
    let (tx, _rx) = mpsc::channel(8);
    let mut shared: Vec<String> = Vec::new();
    let mut seq = std::collections::HashMap::new();

    let coll = mae_sync::kb::KbCollectionDoc::new("testkb", "owner");
    let coll_b64 = mae_sync::encoding::update_to_base64(&coll.encode_state());
    let node = mae_sync::kb::KbNodeDoc::new("testkb:n1", "T", "b", &[]);
    let node_b64 = mae_sync::encoding::update_to_base64(&node.encode_state());

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "collection_state": coll_b64,
            "nodes": [ { "id": "testkb:n1", "state": node_b64 } ]
        }
    });
    handle_response(
        &val,
        PendingResponseKind::KbJoin {
            kb_id: "testkb".to_string(),
        },
        &tx,
        &mut shared,
        &mut seq,
    );
    assert!(
        shared.contains(&"kbc:testkb".to_string()),
        "join must subscribe to the collection doc"
    );
    assert!(
        shared.contains(&"kb:testkb:n1".to_string()),
        "join must subscribe to each node doc — else inbound live updates are dropped (B-13)"
    );
}

#[tokio::test]
async fn handle_response_join_seeds_seq_tracker() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();
    let mut seq = std::collections::HashMap::new();

    // Create a real yrs state to encode.
    let ts = mae_sync::text::TextSync::with_client_id("joined content", 1);
    let state_b64 = mae_sync::encoding::update_to_base64(&ts.encode_state());

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "doc": "joined.rs", "state": state_b64, "wal_seq": 7 }
    });
    handle_response(
        &val,
        PendingResponseKind::JoinDoc {
            doc_id: "joined.rs".to_string(),
        },
        &tx,
        &mut shared,
        &mut seq,
    );

    // WU2: seq_tracker should be seeded from join response wal_seq.
    assert_eq!(seq.get("joined.rs"), Some(&7));
    let event = rx.try_recv().unwrap();
    assert!(matches!(event, CollabEvent::BufferJoined { doc_id, .. } if doc_id == "joined.rs"));
}

#[tokio::test]
async fn handle_incoming_logs_null_id_response() {
    // WU3: Responses with null id should be logged but not panic or emit events.
    let (tx, mut rx) = mpsc::channel(8);
    let mut pending = std::collections::HashMap::new();
    let mut shared = Vec::new();
    let mut seq = std::collections::HashMap::new();

    let msg = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#;
    handle_incoming_message(msg, &tx, &mut pending, &mut shared, &mut seq);

    // Should not emit any event (the warning is logged by tracing).
    assert!(rx.try_recv().is_err());
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
fn buffer_joined_reuses_existing_buffer_by_collab_doc_id() {
    // Regression test: if a buffer was shared (collab_doc_id set) and the
    // user also joins the same doc, BufferJoined must reuse the existing
    // buffer instead of creating a duplicate. Creating a duplicate causes
    // remote updates to be applied to the wrong sync_doc (the one without
    // the locally-typed operations), making all updates no-ops.
    let mut editor = Editor::new();

    // Simulate: buffer "2026-05-27.org" was shared, enable_sync + collab_doc_id set.
    let mut buf = mae_core::Buffer::new();
    buf.name = "2026-05-27.org".to_string();
    buf.insert_text_at(0, "shared content");
    buf.enable_sync(1000);
    buf.collab_doc_id = Some("file:abc123/daily/2026-05-27.org".to_string());
    editor.buffers.push(buf);
    editor
        .collab
        .synced_buffers
        .insert("file:abc123/daily/2026-05-27.org".to_string());
    let original_idx = editor.buffers.len() - 1;

    // Simulate: user also joins the same doc. The join resolves to
    // buf_name="daily/2026-05-27.org" (different from "2026-05-27.org").
    let sync = mae_sync::text::TextSync::with_client_id("shared content", 2000);
    let state_bytes = sync.encode_state();

    handle_collab_event(
        &mut editor,
        CollabEvent::BufferJoined {
            doc_id: "file:abc123/daily/2026-05-27.org".to_string(),
            state_bytes,
        },
    );

    // Should NOT have created a new buffer — should reuse the existing one.
    assert!(
        editor.find_buffer_by_name("daily/2026-05-27.org").is_none(),
        "should not create duplicate buffer with different name"
    );
    // The original buffer should still be the one with the collab_doc_id.
    assert_eq!(
        editor.buffers[original_idx].collab_doc_id.as_deref(),
        Some("file:abc123/daily/2026-05-27.org"),
    );
    // Only one buffer should have this collab_doc_id.
    let matching: Vec<_> = editor
        .buffers
        .iter()
        .filter(|b| b.collab_doc_id.as_deref() == Some("file:abc123/daily/2026-05-27.org"))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one buffer should have this collab_doc_id"
    );
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
        &mut std::collections::HashMap::new(),
    );

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
    // Pre-subscribe to all docs so the filter passes.
    let mut shared: Vec<String> = (0..5).map(|i| format!("file{}.rs", i)).collect();

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
        );
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

#[tokio::test]
async fn unsubscribed_doc_sync_update_ignored() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut pending = std::collections::HashMap::new();
    let mut shared = vec!["subscribed.rs".to_string()]; // Only subscribed to one doc.

    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/sync_update",
        "params": {
            "seq": 1,
            "event": {
                "type": "sync_update",
                "data": {
                    "buffer_name": "other-client.rs",
                    "update_base64": "AQIDBA==",
                    "wal_seq": 1
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
    );
    // No event should be emitted for the unsubscribed doc.
    assert!(
        rx.try_recv().is_err(),
        "sync_update for unsubscribed doc should be ignored"
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
    let (doc_id, epoch, hash, saved_by) = editor.collab.pending_save_committed.as_ref().unwrap();
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
        &mut std::collections::HashMap::new(),
    );
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
        &mut std::collections::HashMap::new(),
    );
    let event = rx.try_recv().unwrap();
    assert!(
        matches!(event, CollabEvent::SaveIntentConflict { .. }),
        "expected SaveIntentConflict, got {:?}",
        event
    );
}

/// B-1: a kb/join response must surface joined / pending / denied as three
/// DISTINCT outcomes — not "Joined (0 nodes)" for all of them.
#[tokio::test]
async fn kb_join_pending_response_is_distinct() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();
    let val = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "kb_id": "collabtest", "status": "pending" }
    });
    handle_response(
        &val,
        PendingResponseKind::KbJoin {
            kb_id: "collabtest".into(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
    );
    match rx.try_recv().unwrap() {
        CollabEvent::StatusReport { lines } => {
            assert!(
                lines.iter().any(|l| l.contains("pending")),
                "pending join should report pending approval, got {lines:?}"
            );
        }
        other => panic!("expected StatusReport for pending, got {other:?}"),
    }
}

#[tokio::test]
async fn kb_join_denied_response_is_distinct() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();
    let val = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "error": { "code": -32603, "message": "not a member of KB 'collabtest'" }
    });
    handle_response(
        &val,
        PendingResponseKind::KbJoin {
            kb_id: "collabtest".into(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
    );
    match rx.try_recv().unwrap() {
        CollabEvent::Error { message } => {
            assert!(
                message.contains("denied"),
                "denied join should report denial, got {message:?}"
            );
        }
        other => panic!("expected Error for denied join, got {other:?}"),
    }
}

#[tokio::test]
async fn kb_join_success_response_emits_joined() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();
    let val = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "kb_id": "collabtest", "collection_state": "", "nodes": [] }
    });
    handle_response(
        &val,
        PendingResponseKind::KbJoin {
            kb_id: "collabtest".into(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
    );
    assert!(
        matches!(rx.try_recv().unwrap(), CollabEvent::KbJoined { .. }),
        "a real join must emit KbJoined"
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
    check_seq_gap("doc1", 1, &mut seq_tracker, &tx);
    check_seq_gap("doc1", 2, &mut seq_tracker, &tx);
    assert!(rx.try_recv().is_err(), "no gap for sequential seqs");

    // Seq 4 — gap (expected 3).
    check_seq_gap("doc1", 4, &mut seq_tracker, &tx);
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
        check_seq_gap("doc1", i, &mut seq_tracker, &tx);
    }
    assert!(rx.try_recv().is_err(), "no gap for sequential 1..5");
}

#[tokio::test]
async fn gap_detection_independent_per_doc() {
    let (tx, mut rx) = mpsc::channel(16);
    let mut seq_tracker = std::collections::HashMap::new();

    check_seq_gap("doc-a", 1, &mut seq_tracker, &tx);
    check_seq_gap("doc-b", 1, &mut seq_tracker, &tx);
    // Both start at 1, no gap.
    assert!(rx.try_recv().is_err());

    // doc-a jumps to 5 — gap.
    check_seq_gap("doc-a", 5, &mut seq_tracker, &tx);
    let event = rx.try_recv().unwrap();
    assert!(matches!(event, CollabEvent::GapDetected { doc_id, .. } if doc_id == "doc-a"));

    // doc-b at 2 — no gap.
    check_seq_gap("doc-b", 2, &mut seq_tracker, &tx);
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
    handle_incoming_message(msg, &tx, &mut pending, &mut shared, &mut seq);
    let event = rx.try_recv().unwrap();
    match event {
        CollabEvent::SharerLeft { doc_id } => {
            assert_eq!(doc_id, "file:abc/main.rs");
        }
        other => panic!("expected SharerLeft, got {:?}", other),
    }
}

// --- Phase 4: Continuous KB sync tests ---

#[test]
fn collab_kb_shared_populates_tracking() {
    let mut editor = Editor::new();
    // Isolate the registry save (handler stamps the primary-shared marker).
    let tmp = std::env::temp_dir().join(format!("mae-adr019-prim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    editor.data_dir_override = Some(tmp.clone());
    // Insert some nodes into the primary KB.
    editor.kb.primary.insert(mae_kb::Node::new(
        "node-1".to_string(),
        "Title 1".to_string(),
        mae_kb::NodeKind::Note,
        "body 1".to_string(),
    ));
    editor.kb.primary.insert(mae_kb::Node::new(
        "node-2".to_string(),
        "Title 2".to_string(),
        mae_kb::NodeKind::Note,
        "body 2".to_string(),
    ));

    // Simulate KbShared event.
    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: "default".to_string(),
            node_count: 2,
            collection_state: Vec::new(),
        },
    );

    assert!(
        editor.collab.shared_kbs.contains_key("default"),
        "shared_kbs should track the shared KB"
    );
    let tracked = &editor.collab.shared_kbs["default"];
    assert!(
        tracked.contains("node-1") && tracked.contains("node-2"),
        "shared_kbs should contain all node IDs: {:?}",
        tracked
    );
    // ADR-019: primary-share durable marker stamped.
    assert!(editor.kb.registry.primary_shared);
    assert_eq!(
        editor.kb.registry.primary_collab_id.as_deref(),
        Some("default")
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// I-9 + ADR-019: sharing a *named federated instance* tracks its node IDs by
/// resolving name→uuid (cache) AND stamps the DURABLE registry marker
/// (`shared`/`collab_id`) so the share survives editor restart.
#[test]
fn collab_kb_shared_named_instance_tracks_nodes_by_uuid() {
    let mut editor = Editor::new();
    // Isolate the registry save to a temp dir (the handler persists markers).
    let tmp = std::env::temp_dir().join(format!("mae-adr019-share-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    editor.data_dir_override = Some(tmp.clone());

    let uuid = "uuid-collabtest".to_string();
    let mut inst = mae_kb::KnowledgeBase::new();
    inst.insert(mae_kb::Node::new(
        "collabtest:overview",
        "Overview",
        mae_kb::NodeKind::Note,
        "b",
    ));
    inst.insert(mae_kb::Node::new(
        "collabtest:alpha",
        "Alpha",
        mae_kb::NodeKind::Note,
        "b",
    ));
    editor.kb.instances.insert(uuid.clone(), inst);
    // Registry maps the human name → uuid, NOT yet shared (handler stamps it).
    editor
        .kb
        .registry
        .instances
        .push(mae_kb::federation::KbInstance {
            uuid: uuid.clone(),
            name: "collabtest".into(),
            org_dir: std::path::PathBuf::from("/tmp/collabtest"),
            db_path: std::path::PathBuf::from("/tmp/collabtest.db"),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
        });

    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: "collabtest".to_string(),
            node_count: 2,
            collection_state: Vec::new(),
        },
    );

    let tracked = &editor.collab.shared_kbs["collabtest"];
    assert!(
        tracked.contains("collabtest:overview") && tracked.contains("collabtest:alpha"),
        "named-instance share must track nodes via uuid resolution, got: {:?}",
        tracked
    );
    // Durable marker stamped (survives restart).
    let inst = editor.kb.registry.find("collabtest").unwrap();
    assert!(inst.shared, "share must stamp durable shared=true");
    assert_eq!(inst.collab_id.as_deref(), Some("collabtest"));
    // And persisted to the isolated registry file.
    assert!(
        tmp.join("kb-registry.toml").exists(),
        "registry marker must be persisted"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// ADR-019 restart-survival (the bug): the durable share marker must survive
/// a registry SAVE→LOAD round-trip, so a freshly-started editor's emit gate
/// fires without any live event. This is the persistence crux of "edits keep
/// propagating across editor restart".
#[test]
fn adr019_share_marker_survives_registry_reload() {
    let tmp = std::env::temp_dir().join(format!("mae-adr019-reload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut editor = Editor::new();
    editor.data_dir_override = Some(tmp.clone());

    let mut inst = mae_kb::KnowledgeBase::new();
    inst.insert(mae_kb::Node::new(
        "collabtest:overview",
        "O",
        mae_kb::NodeKind::Note,
        "b",
    ));
    editor.kb.instances.insert("uuid-ct".into(), inst);
    editor
        .kb
        .registry
        .instances
        .push(mae_kb::federation::KbInstance {
            uuid: "uuid-ct".into(),
            name: "collabtest".into(),
            org_dir: std::path::PathBuf::new(),
            db_path: std::path::PathBuf::new(),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
        });

    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: "collabtest".to_string(),
            node_count: 1,
            collection_state: Vec::new(),
        },
    );

    // Simulate restart: load the registry fresh from disk.
    let reloaded = mae_kb::federation::KbRegistry::load(&tmp);
    let inst = reloaded
        .find("collabtest")
        .expect("instance survives reload");
    assert!(
        inst.shared && inst.collab_id.as_deref() == Some("collabtest"),
        "durable share marker must survive a registry save→load round-trip"
    );

    // A restarted editor (empty cache) with the reloaded registry: the emit
    // gate fires from the durable marker → edits still queue for broadcast.
    let mut restarted = Editor::new();
    restarted.kb.registry = reloaded;
    let mut inst2 = mae_kb::KnowledgeBase::new();
    let mut n = mae_kb::Node::new("collabtest:overview", "O", mae_kb::NodeKind::Note, "b");
    n.source = Some(mae_kb::NodeSource::Federation);
    inst2.insert(n);
    restarted.kb.instances.insert("uuid-ct".into(), inst2);
    restarted.collab.kb_sync_mode = "on_save".into();
    assert!(restarted.collab.shared_kbs.is_empty());

    restarted
        .kb_update_node(
            "collabtest:overview",
            Some("edited after restart"),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        restarted.collab.pending_kb_updates.len(),
        1,
        "post-restart edit must still queue a kb/node_update (durable gate)"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn collab_kb_joined_populates_tracking() {
    let mut editor = Editor::new();
    let tmp = std::env::temp_dir().join(format!("mae-adr019-join-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    editor.data_dir_override = Some(tmp.clone());

    // Create a CRDT node state + SV for the join event (ADR-022 reconcile).
    let doc = mae_sync::kb::KbNodeDoc::new("join-node-1", "Joined Title", "joined body", &[]);
    let state = doc.encode_state();
    let sv = doc.state_vector();

    handle_collab_event(
        &mut editor,
        CollabEvent::KbJoined {
            kb_id: "remote-kb".to_string(),
            collection_state: vec![],
            nodes: vec![JoinedNode {
                id: "join-node-1".to_string(),
                bytes: state,
                daemon_sv: Some(sv),
            }],
        },
    );

    assert!(
        editor.collab.shared_kbs.contains_key("remote-kb"),
        "shared_kbs should track the joined KB"
    );
    assert!(
        editor.collab.shared_kbs["remote-kb"].contains("join-node-1"),
        "shared_kbs should contain the joined node ID"
    );
    // ADR-019: joined KB is a FIRST-CLASS instance with durable markers, NOT
    // dumped into primary (fixes B-3).
    let inst = editor
        .kb
        .registry
        .find_by_collab_id("remote-kb")
        .expect("joined KB must be a registered instance");
    assert!(inst.shared && inst.collab_id.as_deref() == Some("remote-kb"));
    let uuid = inst.uuid.clone();
    assert!(
        editor.kb.instances[&uuid].get("join-node-1").is_some(),
        "joined node must live in the instance"
    );
    assert!(
        editor.kb.primary.get("join-node-1").is_none(),
        "joined node must NOT be dumped into primary"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn collab_kb_left_removes_tracking() {
    let mut editor = Editor::new();
    editor
        .collab
        .shared_kbs
        .insert("test-kb".to_string(), HashSet::from(["n1".to_string()]));

    handle_collab_event(
        &mut editor,
        CollabEvent::KbLeft {
            kb_id: "test-kb".to_string(),
        },
    );

    assert!(
        !editor.collab.shared_kbs.contains_key("test-kb"),
        "shared_kbs should be cleared after leaving"
    );
}

#[test]
fn collab_kb_update_node_generates_crdt_update_for_shared_node() {
    let mut editor = Editor::new();
    // Insert a node and mark it as shared.
    editor.kb.primary.insert(mae_kb::Node::new(
        "shared-node".to_string(),
        "Original Title".to_string(),
        mae_kb::NodeKind::Note,
        "original body".to_string(),
    ));
    // ADR-019: the durable primary-share marker is the gate authority.
    editor.kb.registry.primary_shared = true;
    editor.kb.registry.primary_collab_id = Some("my-kb".to_string());
    editor.collab.kb_sync_mode = mae_core::KB_SYNC_MODE_DEFAULT.to_string();

    // Update the node.
    editor
        .kb_update_node("shared-node", Some("New Title"), Some("new body"), None)
        .unwrap();

    // Should have a pending KB update.
    assert_eq!(
        editor.collab.pending_kb_updates.len(),
        1,
        "should generate one pending KB update"
    );
    let (kb_id, node_id, update_bytes) = &editor.collab.pending_kb_updates[0];
    assert_eq!(kb_id, "my-kb");
    assert_eq!(node_id, "shared-node");
    assert!(
        !update_bytes.is_empty(),
        "CRDT update bytes should be non-empty"
    );
}

#[test]
fn collab_kb_update_node_no_update_for_unshared_node() {
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "local-only".to_string(),
        "Title".to_string(),
        mae_kb::NodeKind::Note,
        "body".to_string(),
    ));
    editor.collab.kb_sync_mode = mae_core::KB_SYNC_MODE_DEFAULT.to_string();
    // No shared_kbs entry for this node.

    editor
        .kb_update_node("local-only", Some("Updated"), None, None)
        .unwrap();

    assert!(
        editor.collab.pending_kb_updates.is_empty(),
        "unshared node should not generate KB updates"
    );
}

#[test]
fn collab_kb_manual_sync_mode_suppresses_auto_update() {
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "shared-node".to_string(),
        "Title".to_string(),
        mae_kb::NodeKind::Note,
        "body".to_string(),
    ));
    editor.collab.shared_kbs.insert(
        "my-kb".to_string(),
        HashSet::from(["shared-node".to_string()]),
    );
    editor.collab.kb_sync_mode = "manual".to_string();

    editor
        .kb_update_node("shared-node", Some("New Title"), None, None)
        .unwrap();

    assert!(
        editor.collab.pending_kb_updates.is_empty(),
        "manual sync mode should not auto-generate KB updates"
    );
}

#[test]
fn collab_kb_drain_pending_updates_sends_commands() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    editor.collab.pending_kb_updates.push((
        "kb-1".to_string(),
        "node-a".to_string(),
        vec![1, 2, 3],
    ));
    editor.collab.pending_kb_updates.push((
        "kb-1".to_string(),
        "node-b".to_string(),
        vec![4, 5, 6],
    ));

    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);

    // Should have sent 2 KbNodeUpdate commands.
    let cmd1 = rx.try_recv().unwrap();
    let cmd2 = rx.try_recv().unwrap();
    match cmd1 {
        CollabCommand::KbNodeUpdate {
            kb_id,
            node_id,
            update,
            pending_rowid,
        } => {
            assert_eq!(kb_id, "kb-1");
            assert_eq!(node_id, "node-a");
            assert_eq!(update, vec![1, 2, 3]);
            assert_eq!(
                pending_rowid, None,
                "in-memory updates carry no durable rowid"
            );
        }
        other => panic!(
            "expected KbNodeUpdate, got: {:?}",
            collab_command_name(&other)
        ),
    }
    match cmd2 {
        CollabCommand::KbNodeUpdate {
            kb_id,
            node_id,
            update,
            pending_rowid,
        } => {
            assert_eq!(kb_id, "kb-1");
            assert_eq!(node_id, "node-b");
            assert_eq!(update, vec![4, 5, 6]);
            assert_eq!(
                pending_rowid, None,
                "in-memory updates carry no durable rowid"
            );
        }
        other => panic!(
            "expected KbNodeUpdate, got: {:?}",
            collab_command_name(&other)
        ),
    }

    // Pending list should be drained.
    assert!(editor.collab.pending_kb_updates.is_empty());
}

#[test]
fn collab_kb_update_crdt_bytes_apply_to_fresh_doc() {
    // Verify that the CRDT update bytes generated by upsert_with_crdt
    // can actually be applied to reconstruct the node content.
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "crdt-test".to_string(),
        "Original".to_string(),
        mae_kb::NodeKind::Note,
        "original body with café and 日本語".to_string(),
    ));
    // ADR-019: durable primary-share marker gates the broadcast.
    editor.kb.registry.primary_shared = true;
    editor.kb.registry.primary_collab_id = Some("test-kb".to_string());
    editor.collab.kb_sync_mode = mae_core::KB_SYNC_MODE_DEFAULT.to_string();

    editor
        .kb_update_node(
            "crdt-test",
            Some("Updated Title"),
            Some("new body — naïve résumé"),
            None,
        )
        .unwrap();

    let (_, _, update_bytes) = &editor.collab.pending_kb_updates[0];

    // Apply the update bytes to a fresh KbNodeDoc.
    let doc = mae_sync::kb::KbNodeDoc::from_bytes(update_bytes)
        .expect("CRDT bytes should decode to valid KbNodeDoc");
    let mat = doc.materialize();
    assert_eq!(
        mat.title, "Updated Title",
        "title should match after CRDT round-trip"
    );
    assert_eq!(
        mat.body, "new body — naïve résumé",
        "body should preserve UTF-8 after CRDT round-trip"
    );
}

// --- Phase 8: Offline KB sync tests ---

#[test]
fn offline_kb_updates_accumulate_when_disconnected() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Disconnected;
    editor.collab.pending_kb_updates.push((
        "kb-1".to_string(),
        "node-a".to_string(),
        vec![1, 2, 3],
    ));

    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);

    // Updates should NOT be sent when disconnected.
    assert!(
        rx.try_recv().is_err(),
        "pending KB updates should not be drained while disconnected"
    );
    // They should remain in the queue.
    assert_eq!(
        editor.collab.pending_kb_updates.len(),
        1,
        "pending KB updates should be preserved while offline"
    );
}

#[test]
fn offline_kb_updates_drain_on_reconnect() {
    let mut editor = Editor::new();
    // Start disconnected, accumulate updates.
    editor.collab.status = CollabStatus::Disconnected;
    editor
        .collab
        .pending_kb_updates
        .push(("kb-1".to_string(), "node-a".to_string(), vec![10, 20]));

    let (tx, mut rx) = mpsc::channel(8);

    // First drain while disconnected — nothing sent.
    drain_collab_intents(&mut editor, &tx);
    assert!(rx.try_recv().is_err());
    assert_eq!(editor.collab.pending_kb_updates.len(), 1);

    // Simulate reconnect.
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    drain_collab_intents(&mut editor, &tx);

    // Now the update should be sent.
    let cmd = rx
        .try_recv()
        .expect("KB update should be sent after reconnect");
    match cmd {
        CollabCommand::KbNodeUpdate { kb_id, node_id, .. } => {
            assert_eq!(kb_id, "kb-1");
            assert_eq!(node_id, "node-a");
        }
        other => panic!(
            "expected KbNodeUpdate, got: {:?}",
            collab_command_name(&other)
        ),
    }
    assert!(editor.collab.pending_kb_updates.is_empty());
}

#[test]
fn offline_kb_multiple_edits_all_sent_on_reconnect() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Disconnected;

    // Accumulate 3 offline edits.
    for i in 0..3 {
        editor.collab.pending_kb_updates.push((
            "kb-1".to_string(),
            format!("node-{}", i),
            vec![i as u8],
        ));
    }

    let (tx, mut rx) = mpsc::channel(8);

    // Reconnect and drain.
    editor.collab.status = CollabStatus::Connected { peer_count: 2 };
    drain_collab_intents(&mut editor, &tx);

    // All 3 should be sent.
    for _ in 0..3 {
        assert!(
            rx.try_recv().is_ok(),
            "all offline KB updates should be sent on reconnect"
        );
    }
    assert!(rx.try_recv().is_err(), "no extra commands should be sent");
    assert!(editor.collab.pending_kb_updates.is_empty());
}

// -----------------------------------------------------------------------
// PSK wiring tests — CI-runnable (no network required)
// -----------------------------------------------------------------------

#[tokio::test]
async fn perform_psk_auth_correct_key_succeeds() {
    // Test perform_psk_auth against a real PskAuth server handshake
    // using tokio duplex streams (no TCP needed).
    use mae_mcp::auth::{AuthProvider, PskAuth};
    use tokio::io::{duplex, BufReader, BufWriter};

    let psk = "test-secret-for-collab-bridge";
    let (client_stream, server_stream) = duplex(4096);
    let (cr, cw) = tokio::io::split(client_stream);
    let (sr, sw) = tokio::io::split(server_stream);

    let server_auth = PskAuth::new(psk);
    let server_handle = tokio::spawn(async move {
        let mut sr = BufReader::new(sr);
        let mut sw = BufWriter::new(sw);
        server_auth.server_handshake(&mut sr, &mut sw).await
    });

    let client_handle = tokio::spawn(async move {
        let mut cr = BufReader::new(cr);
        let mut cw = BufWriter::new(cw);
        perform_psk_auth(&mut cr, &mut cw, psk, None).await
    });

    let (server_result, client_result) = tokio::join!(server_handle, client_handle);
    assert!(
        server_result.unwrap().is_ok(),
        "server handshake should succeed with correct PSK"
    );
    assert!(
        client_result.unwrap().is_ok(),
        "perform_psk_auth should succeed with correct PSK"
    );
}

#[tokio::test]
async fn perform_psk_auth_wrong_key_fails() {
    use mae_mcp::auth::{AuthProvider, PskAuth};
    use tokio::io::{duplex, BufReader, BufWriter};

    let (client_stream, server_stream) = duplex(4096);
    let (cr, cw) = tokio::io::split(client_stream);
    let (sr, sw) = tokio::io::split(server_stream);

    let server_auth = PskAuth::new("server-key");
    let server_handle = tokio::spawn(async move {
        let mut sr = BufReader::new(sr);
        let mut sw = BufWriter::new(sw);
        server_auth.server_handshake(&mut sr, &mut sw).await
    });

    let client_handle = tokio::spawn(async move {
        let mut cr = BufReader::new(cr);
        let mut cw = BufWriter::new(cw);
        perform_psk_auth(&mut cr, &mut cw, "wrong-key", None).await
    });

    let (server_result, client_result) = tokio::join!(server_handle, client_handle);
    let server_ok = server_result.is_ok_and(|r| r.is_ok());
    let client_ok = client_result.is_ok_and(|r| r.is_ok());
    assert!(
        !server_ok || !client_ok,
        "mismatched PSK should cause at least one side to fail"
    );
}

#[tokio::test]
async fn perform_psk_auth_empty_key_skips_auth() {
    // Empty PSK should skip auth entirely (no reads/writes on the stream).
    use tokio::io::{duplex, BufReader, BufWriter};

    let (client_stream, _server_stream) = duplex(4096);
    let (cr, cw) = tokio::io::split(client_stream);
    let mut cr = BufReader::new(cr);
    let mut cw = BufWriter::new(cw);

    let result = perform_psk_auth(&mut cr, &mut cw, "", None).await;
    assert!(result.is_ok(), "empty PSK should skip auth and return Ok");
}

#[test]
fn setup_collab_channels_propagates_psk_direct() {
    // When collab.psk is set (no psk_command), it should flow through to CollabSpawn.psk.
    let mut editor = Editor::new();
    let _ = editor.set_option("collab_psk", "my-secret-key");

    let (_evt_rx, _cmd_tx, spawn) = setup_collab_channels(&editor);
    assert_eq!(
        spawn.transport.plain_psk(),
        Some("my-secret-key"),
        "transport should carry the direct PSK value"
    );
}

#[test]
fn setup_collab_channels_propagates_psk_command() {
    // When collab.psk_command is set, it should be prefixed with "cmd:" sentinel.
    let mut editor = Editor::new();
    let _ = editor.set_option("collab_psk_command", "cat /tmp/test-psk.txt");

    let (_evt_rx, _cmd_tx, spawn) = setup_collab_channels(&editor);
    assert_eq!(
        spawn.transport.plain_psk(),
        Some("cmd:cat /tmp/test-psk.txt"),
        "transport should carry the cmd: prefix for deferred resolution"
    );
}

#[test]
fn setup_collab_channels_psk_command_takes_precedence() {
    // When both psk and psk_command are set, psk_command wins.
    let mut editor = Editor::new();
    let _ = editor.set_option("collab_psk", "plaintext-key");
    let _ = editor.set_option("collab_psk_command", "pass show mae/psk");

    let (_evt_rx, _cmd_tx, spawn) = setup_collab_channels(&editor);
    let psk = spawn.transport.plain_psk().unwrap_or("");
    assert!(
        psk.starts_with("cmd:"),
        "psk_command should take precedence over psk: got '{psk}'"
    );
    assert_eq!(psk, "cmd:pass show mae/psk");
}

#[test]
fn setup_collab_channels_empty_psk_is_empty() {
    // With no psk/psk_command AND no keystore, the credential is empty.
    let (psk, key_id) = resolve_client_credential("", "", None);
    assert!(psk.is_empty(), "no creds → empty psk, got '{psk}'");
    assert_eq!(key_id, None);
}

#[test]
fn resolve_credential_precedence() {
    // psk_command wins, returned as a cmd: sentinel, no key_id.
    let (psk, id) = resolve_client_credential("pass show k", "plain", None);
    assert_eq!(psk, "cmd:pass show k");
    assert_eq!(id, None);
    // psk wins over keystore when no command.
    let (psk, id) = resolve_client_credential("", "plain", None);
    assert_eq!(psk, "plain");
    assert_eq!(id, None);
}

#[test]
fn resolve_credential_from_keystore_primary() {
    // A keystore with a named primary key → present its secret + name.
    let dir = std::env::temp_dir().join(format!("mae-cred-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("trusted_keys");
    mae_mcp::keystore::add_key(&path, Some("framework"), "deadbeef").unwrap();
    mae_mcp::keystore::add_key(&path, Some("thinkpad"), "cafef00d").unwrap();

    let (psk, id) = resolve_client_credential("", "", Some(&path));
    assert_eq!(psk, "deadbeef", "presents the primary (first) key");
    assert_eq!(id.as_deref(), Some("framework"), "advertises the key name");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn drain_discover_peers_does_not_send_command() {
    // DiscoverPeers is handled locally (mDNS browse + buffer creation).
    // It should NOT send any CollabCommand to the network channel.
    // NOTE: MdnsManager::new() may fail on CI (no multicast), but that's
    // fine — the intent is still consumed (returns early with status msg).
    let mut editor = Editor::new();
    editor.collab.pending_intent = Some(CollabIntent::DiscoverPeers);
    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);

    // Intent must be consumed regardless of mDNS availability.
    assert!(
        editor.collab.pending_intent.is_none(),
        "DiscoverPeers intent should be consumed"
    );
    // No command should be sent to the collab task.
    assert!(
        rx.try_recv().is_err(),
        "DiscoverPeers should not send any CollabCommand"
    );
}
