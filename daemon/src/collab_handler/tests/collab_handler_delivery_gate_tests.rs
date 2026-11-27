//! The delivery gate (`delivery_gate`): a principal who loses read access to a
//! KB stops receiving its documents on the sessions already open, whichever
//! way the access was lost, and is told with `session/doc_closed`.

use super::*;
use mae_mcp::identity::Identity;

type Pipe = (
    tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
);

/// A real session loop for bearer `principal` on the shared `bc`, governed (as
/// the WebSocket route runs it) or not (as mTLS runs it).
fn session(store: &Arc<DocStore>, bc: &SharedBroadcaster, principal: &str, governed: bool) -> Pipe {
    let (client, server) = tokio::io::duplex(1 << 20);
    let (sr, sw) = tokio::io::split(server);
    let reader = tokio::io::BufReader::new(sr);
    let peer = mae_mcp::identity::PeerIdentity::bearer(principal);
    let (store, bc) = (Arc::clone(store), bc.clone());
    if governed {
        let deps = SessionDeps {
            doc_store: store,
            broadcaster: bc,
            start_time: std::time::Instant::now(),
            artifact_store: Arc::new(crate::artifact_store::NoArtifactStore),
            quota: Arc::new(crate::quota::NoQuota),
            kb_query_limits: crate::kb_query::KbQueryLimits::default(),
        };
        let (gov, _h) = governor::governor(0, u64::MAX);
        tokio::spawn(handle_client_governed(reader, sw, peer, deps, gov));
    } else {
        tokio::spawn(handle_client_authenticated(
            reader,
            sw,
            peer,
            store,
            bc,
            std::time::Instant::now(),
            Transport::Hub,
            Arc::new(crate::artifact_store::NoArtifactStore),
            Arc::new(crate::quota::NoQuota),
            crate::kb_query::KbQueryLimits::default(),
            None,
        ));
    }
    let (cr, cw) = tokio::io::split(client);
    (tokio::io::BufReader::new(cr), cw)
}

/// The next message the session sends, if one comes within `ms`.
async fn next(pipe: &mut Pipe, ms: u64) -> Option<serde_json::Value> {
    let t = std::time::Duration::from_millis(ms);
    let m = tokio::time::timeout(t, mae_mcp::read_message(&mut pipe.0)).await;
    m.ok()?.ok()?.and_then(|m| serde_json::from_str(&m).ok())
}

async fn send(pipe: &mut Pipe, msg: serde_json::Value) -> serde_json::Value {
    let t = std::time::Duration::from_secs(5);
    mae_mcp::write_framed(&mut pipe.1, msg.to_string().as_bytes(), t)
        .await
        .unwrap();
    next(pipe, 5_000).await.expect("a reply")
}

fn update(doc: &str) -> EditorEvent {
    EditorEvent::SyncUpdate {
        buffer_name: doc.to_string(),
        update_base64: "AA==".into(),
        wal_seq: 1,
        content_header: None,
    }
}

/// What the session got: "update" for a delivered sync update to `doc`,
/// "closed:<reason>" for a doc_closed notice naming `doc`, "nothing" for
/// silence, else the method.
async fn got(pipe: &mut Pipe, doc: &str) -> String {
    match next(pipe, 1_500).await {
        None => "nothing".into(),
        Some(m) => match m["method"].as_str() {
            Some("notifications/sync_update")
                if m["params"]["event"]["data"]["buffer_name"] == doc =>
            {
                "update".into()
            }
            Some("session/doc_closed") if m["params"]["doc"] == doc => {
                format!("closed:{}", m["params"]["reason"].as_str().unwrap_or(""))
            }
            other => format!("{other:?}: {m}"),
        },
    }
}

struct Kb {
    store: Arc<DocStore>,
    bc: SharedBroadcaster,
    owner: String,
    bob: String,
    carol: String,
}

/// KB `kbg` owned by the daemon's identity, bob and carol viewers, anchored
/// (so the blocklist governs, as it does for every signed-log KB).
async fn kb() -> Kb {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let id = Identity::generate("owner");
    let (owner, pk) = (id.fingerprint(), id.public().to_bytes());
    store.set_signer(Arc::new(id));
    let (bob, carol) = (fp("bob"), fp("carol"));
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        "kbg",
        "owner",
        &mut docs,
    )
    .await;
    for m in [&bob, &carol] {
        let add = kb_member_msg("kb/add_member", "kbg", m, Some("viewer"));
        let r = dispatch_as(&store, &bc, Some("owner"), Some(&owner), add, &mut docs).await;
        assert!(r.error.is_none(), "{:?}", r.error);
    }
    store.set_kb_anchor("kbg", pk).await.unwrap();
    Kb {
        store,
        bc,
        owner,
        bob,
        carol,
    }
}

/// bob and carol joined, each already admitted once at delivery (so a stale
/// memoized allow is what a missing invalidation would serve).
async fn joined(k: &Kb, governed: bool) -> (Pipe, Pipe) {
    let mut bob = session(&k.store, &k.bc, &k.bob, governed);
    let mut carol = session(&k.store, &k.bc, &k.carol, governed);
    for p in [&mut bob, &mut carol] {
        let r = send(p, kb_join_msg("kbg")).await;
        assert!(r.get("error").is_none(), "join: {r}");
    }
    k.bc.lock().unwrap().broadcast(&update("kbc:kbg"));
    assert_eq!(got(&mut bob, "kbc:kbg").await, "update", "baseline");
    assert_eq!(got(&mut carol, "kbc:kbg").await, "update", "baseline");
    (bob, carol)
}

/// A member removed while connected is closed out at the next delivery (the
/// removal's own collection update) and told why; nothing after reaches them;
/// another member keeps receiving. On both a governed and an ungoverned
/// session. Kills: no gate at delivery, a removal not moving the access
/// generation, and a refusal read as an allow.
#[tokio::test]
async fn a_member_removed_while_connected_is_closed_out_at_the_next_delivery() {
    for governed in [false, true] {
        let k = kb().await;
        let (mut bob, mut carol) = joined(&k, governed).await;
        let rm = kb_member_msg("kb/remove_member", "kbg", &k.bob, None);
        let mut docs = HashSet::new();
        let r = dispatch_as(
            &k.store,
            &k.bc,
            Some("owner"),
            Some(&k.owner),
            rm,
            &mut docs,
        )
        .await;
        assert!(r.error.is_none(), "{:?}", r.error);

        assert_eq!(
            got(&mut bob, "kbc:kbg").await,
            "closed:not_member",
            "governed {governed}"
        );
        assert_eq!(
            got(&mut carol, "kbc:kbg").await,
            "update",
            "governed {governed}"
        );
        k.bc.lock().unwrap().broadcast(&update("kbc:kbg"));
        assert_eq!(
            got(&mut bob, "kbc:kbg").await,
            "nothing",
            "governed {governed}"
        );
        assert_eq!(
            got(&mut carol, "kbc:kbg").await,
            "update",
            "governed {governed}"
        );
    }
}

/// A block changes no document, only the blocklist, yet the blocked
/// principal is closed out at the next delivery. Kills: a block not moving
/// the access generation, which would keep serving the memoized allow.
#[tokio::test]
async fn a_block_closes_a_principal_out_though_no_document_changed() {
    let k = kb().await;
    let (mut bob, mut carol) = joined(&k, false).await;
    let block = kb_block_msg("kb/block_principal", "kbg", &k.bob);
    let mut docs = HashSet::new();
    let r = dispatch_as(
        &k.store,
        &k.bc,
        Some("owner"),
        Some(&k.owner),
        block,
        &mut docs,
    )
    .await;
    assert!(r.error.is_none(), "{:?}", r.error);

    k.bc.lock().unwrap().broadcast(&update("kbc:kbg"));
    assert_eq!(got(&mut bob, "kbc:kbg").await, "closed:not_member");
    assert_eq!(got(&mut carol, "kbc:kbg").await, "update");
}

/// A timeboxed grant lapses with nothing written: the gate decides again once
/// the horizon passes. Kills: a memoized allow that ignores the timebox.
#[tokio::test]
async fn a_lapsed_timebox_is_refused_at_delivery_with_nothing_written() {
    use mae_sync::membership::MembershipAction;
    let store = test_doc_store();
    let owner = Identity::generate("owner");
    let (ofp, opk, osec) = (
        owner.fingerprint(),
        owner.public().to_bytes(),
        owner.secret_bytes(),
    );
    let bob = Identity::generate("bob").fingerprint();
    let now = now_unix();
    let expiry = now + 2;

    let mut coll = KbCollectionDoc::new_owned("kbt", &ofp, "owner");
    let genesis = coll.build_membership_op(
        "kbt",
        MembershipAction::Admit,
        &ofp,
        Some(SyncRole::Owner),
        true,
        &ofp,
        now,
        None,
        0,
    );
    let sig = genesis.sign(&osec);
    coll.append_signed_op(&genesis, &sig, &opk);
    coll.upsert_member(&bob, "bob", SyncRole::Viewer);
    let epoch = coll.epoch_of(&bob);
    let admit = coll.build_membership_op(
        "kbt",
        MembershipAction::Admit,
        &bob,
        Some(SyncRole::Viewer),
        false,
        &ofp,
        now,
        Some(expiry),
        epoch,
    );
    let sig = admit.sign(&osec);
    coll.append_signed_op(&admit, &sig, &opk);
    store
        .share_doc("kbc:kbt", &coll.encode_state(), TEST_BY)
        .await
        .unwrap();
    store.set_kb_anchor("kbt", opk).await.unwrap();

    let mut gate = delivery_gate::DeliveryGate::new(Some(&bob), Transport::Hub);
    assert!(gate.admits(&store, "kbt").await, "inside the timebox");
    while now_unix() <= expiry {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !gate.admits(&store, "kbt").await,
        "past the timebox, nothing written"
    );
}
