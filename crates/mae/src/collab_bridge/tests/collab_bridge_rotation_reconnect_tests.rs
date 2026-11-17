//! ADR-040 §4, the editor's half: after `collab-rotate-identity`'s `Rebind` is accepted, the
//! session reconnects authenticated by the NEW key, and the owner's self re-wrap goes out on
//! that session — never on the old key's, which a hub that verifies ops refuses everything
//! (the daemon's `a_retired_keys_session_gets_nothing_after_rotation`).
//!
//! Drives the real `run_collab_task` over loopback TCP with a real `KeyAuth` handshake (the
//! `collab_bridge_write_failure_tests` harness). The fake hub decodes every
//! `kb/collection_op` against the KB's replica, learns the successor's key from the
//! `Rebind`, and from then on authorizes ONLY that key: the old key's reconnect is refused,
//! as a daemon's operator would refuse it once it revokes the retired key.

use super::collab_bridge_write_failure_tests::{
    owned_e2e_collection_state, recv_until, FixedTrustVerifier,
};
use super::*;
use mae_mcp::auth::{AuthProvider, KeyAuth};
use mae_mcp::identity::{AuthorizedKeys, HostKeyVerifier, Identity, PublicKey};
use mae_sync::kb::KbCollectionDoc;
use mae_sync::membership::MembershipAction;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::BufReader;
use tokio::net::TcpListener;

/// What the fake hub saw: which key each connection authenticated as, and each membership
/// op a `kb/collection_op` carried, with the connection it arrived on.
#[derive(Debug)]
enum Seen {
    Authenticated {
        conn: usize,
        fp: String,
    },
    Op {
        conn: usize,
        action: MembershipAction,
        author: String,
        wraps: bool,
    },
}

/// A hub that authorizes `first` until it accepts a `Rebind` naming a successor, then only
/// the successor. Answers `initialize` and every `kb/collection_op` — ok, or, for a `Rebind`
/// when `refuse_rebind`, an error — and ignores the rest.
async fn spawn_rotating_hub(
    server: Identity,
    first: PublicKey,
    seed: Vec<u8>,
    refuse_rebind: bool,
) -> (std::net::SocketAddr, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (seen_tx, seen_rx) = mpsc::channel(64);
    let server = Arc::new(server);
    tokio::spawn(async move {
        let mut replica = KbCollectionDoc::from_bytes(&seed).unwrap();
        let mut allowed = first;
        let mut conn = 0usize;
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let (r, mut w) = stream.into_split();
            let mut sr = BufReader::new(r);
            let tmp = tempfile::tempdir().unwrap();
            let mut authorized = AuthorizedKeys::load(&tmp.path().join("authorized_keys"));
            authorized.add(allowed.clone()).unwrap();
            let auth = KeyAuth::server(server.clone(), Arc::new(authorized));
            let Ok(result) = auth.server_handshake(&mut sr, &mut w).await else {
                continue; // a key this hub does not authorize: refused, as a daemon would
            };
            let fp = result.peer_identity().fingerprint;
            let _ = seen_tx.send(Seen::Authenticated { conn, fp }).await;
            let reply = |id: &serde_json::Value, result: serde_json::Value| {
                serde_json::to_vec(
                    &serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}),
                )
                .unwrap()
            };
            while let Ok(Some(text)) = mae_mcp::read_message(&mut sr).await {
                let req: serde_json::Value = serde_json::from_str(&text).unwrap();
                let body = match req["method"].as_str() {
                    Some("initialize") => reply(
                        &req["id"],
                        serde_json::json!({"serverInfo": {"connections": 1}}),
                    ),
                    Some("kb/collection_op") => {
                        let b64 = req["params"]["update"].as_str().unwrap();
                        let delta = mae_sync::encoding::base64_to_update(b64).unwrap();
                        // The op log is not kept in arrival order: what is new is what the
                        // replica did not hold before.
                        let before: std::collections::HashSet<String> =
                            replica.oplog_ops().iter().map(|o| o.chain_hash()).collect();
                        let mut probe =
                            KbCollectionDoc::from_bytes(&replica.encode_state()).unwrap();
                        probe.apply_update(&delta).unwrap();
                        let new_ops: Vec<_> = probe
                            .oplog_ops()
                            .into_iter()
                            .filter(|o| !before.contains(&o.chain_hash()))
                            .collect();
                        let rebind = new_ops
                            .iter()
                            .any(|o| o.op.action == MembershipAction::Rebind);
                        let refused = rebind && refuse_rebind;
                        if !refused {
                            replica = probe;
                        }
                        for op in new_ops {
                            if op.op.action == MembershipAction::Rebind && !refused {
                                if let Some(pk) = op.op.new_pubkey {
                                    allowed = PublicKey::from_bytes(&pk, None).unwrap();
                                }
                            }
                            let seen = Seen::Op {
                                conn,
                                action: op.op.action,
                                author: op.op.author.clone(),
                                wraps: op.op.wrapped_key.is_some(),
                            };
                            let _ = seen_tx.send(seen).await;
                        }
                        // The daemon's refusal of a `Rebind` it will not apply, as the
                        // daemon sends it: a kind the client treats as final.
                        if refused {
                            serde_json::to_vec(
                                &serde_json::json!({"jsonrpc": "2.0", "id": req["id"],
                                "error": {"code": -32603, "message": "refused (test)",
                                          "data": {"kind": "role_forbidden"}}}),
                            )
                            .unwrap()
                        } else {
                            reply(&req["id"], serde_json::json!({"ok": true}))
                        }
                    }
                    _ => continue,
                };
                if mae_mcp::write_framed(&mut w, &body, Duration::from_secs(2))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            conn += 1;
        }
    });
    (addr, seen_rx)
}

async fn next_seen(rx: &mut mpsc::Receiver<Seen>, pred: impl Fn(&Seen) -> bool) -> Seen {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let s = rx.recv().await.expect("the hub stopped");
            if pred(&s) {
                return s;
            }
        }
    })
    .await
    .expect("timed out waiting on the hub")
}

/// Connect a real `run_collab_task` as `old` (seed [71; 32]) to the hub, wait until the hub
/// has authenticated it, and seed the owned E2e KB (and its content key) a rotation plans from.
async fn start_client(
    addr: std::net::SocketAddr,
    server_pub: PublicKey,
    seen: &mut mpsc::Receiver<Seen>,
    old_fp: &str,
    seed: Vec<u8>,
    key_dir: &std::path::Path,
) -> (mpsc::Sender<CollabCommand>, mpsc::Receiver<CollabEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    let (evt_tx, evt_rx) = mpsc::channel(64);
    let verifier: Arc<dyn HostKeyVerifier> = Arc::new(FixedTrustVerifier::new(addr, server_pub));
    let transport = ClientTransport::KeyJson {
        identity: Arc::new(Identity::from_seed(&[71u8; 32], "owner")),
        verifier,
        rotation: rotation::Slot::new(Some(key_dir.to_path_buf())),
    };
    // reconnect_secs = 1: the reconnect fires within the test's patience.
    tokio::spawn(run_collab_task(
        cmd_rx,
        evt_tx,
        1,
        Duration::from_secs(2),
        1,
        0,
        0,
        0,
        0,
        transport,
    ));
    let address = addr.to_string();
    cmd_tx
        .send(CollabCommand::Connect { address })
        .await
        .unwrap();
    let first = next_seen(seen, |s| matches!(s, Seen::Authenticated { .. })).await;
    assert!(
        matches!(&first, Seen::Authenticated { conn: 0, fp } if *fp == old_fp),
        "{first:?}"
    );

    // Seed the owned E2e KB (and its content key) the rotation plans from.
    cmd_tx
        .send(CollabCommand::KbSetEncryption {
            kb_id: "kb-rot".to_string(),
            mode: "e2e".to_string(),
            collection_state: seed,
            node_states: Vec::new(),
        })
        .await
        .unwrap();
    (cmd_tx, evt_rx)
}

/// The rotation's whole client-side contract, against a hub that refuses the retired key:
/// the `Rebind` goes out on the old key's session; once it is accepted the session comes
/// back authenticated as the successor (the old key could not reconnect at all here); and
/// the owner's self re-wrap — signed by the successor — arrives on that new session and
/// never on the old one.
#[tokio::test]
async fn after_rotation_the_session_reconnects_as_the_new_key_and_sends_the_rewrap_there() {
    let old = Identity::from_seed(&[71u8; 32], "owner");
    let server = Identity::from_seed(&[72u8; 32], "daemon");
    let server_pub = server.public();
    let old_fp = old.fingerprint();
    let seed = owned_e2e_collection_state("kb-rot", &old);
    let (addr, mut seen) = spawn_rotating_hub(server, old.public(), seed.clone(), false).await;
    let key_dir = tempfile::tempdir().unwrap();
    let (cmd_tx, mut evt_rx) =
        start_client(addr, server_pub, &mut seen, &old_fp, seed, key_dir.path()).await;
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();

    // 1. The Rebind, signed by the old key, on the old key's session.
    let rebind = next_seen(&mut seen, |s| {
        matches!(
            s,
            Seen::Op {
                action: MembershipAction::Rebind,
                ..
            }
        )
    })
    .await;
    assert!(
        matches!(&rebind, Seen::Op { conn: 0, author, .. } if *author == old_fp),
        "{rebind:?}"
    );

    // 2. The session comes back — and the hub, which now authorizes ONLY the successor,
    //    accepted it, so it authenticated as the new key.
    let second = next_seen(&mut seen, |s| matches!(s, Seen::Authenticated { .. })).await;
    let Seen::Authenticated { conn, fp: new_fp } = second else {
        unreachable!()
    };
    assert!(conn > 0, "a new connection");
    assert_ne!(new_fp, old_fp, "authenticated by the NEW key");

    // 3. The self re-wrap: signed by the successor, on the successor's session. Every op
    //    seen before it is checked too, so a re-wrap sent on the old session fails here.
    let rewrap = next_seen(&mut seen, |s| match s {
        Seen::Op {
            conn: 0,
            wraps: true,
            author,
            ..
        } => {
            panic!("the re-wrap went out on the retired key's session (by {author})")
        }
        Seen::Op { wraps: true, .. } => true,
        _ => false,
    })
    .await;
    assert!(
        matches!(&rewrap, Seen::Op { conn: c, author, action: MembershipAction::Admit, .. }
            if *c == conn && *author == new_fp),
        "the re-wrap rides the new session, signed by the new key: {rewrap:?}"
    );

    // The user is told, and nothing reported a failure.
    let report = recv_until(&mut evt_rx, |e| {
        matches!(e, CollabEvent::StatusReport { lines } if lines.iter().any(|l| l.contains("New fingerprint")))
            || matches!(e, CollabEvent::Error { .. })
    })
    .await;
    match report {
        CollabEvent::StatusReport { lines } => assert!(
            lines.iter().any(|l| l.contains(&new_fp)),
            "the report names the key the session now uses: {lines:?}"
        ),
        other => panic!("rotation reported an error: {other:?}"),
    }
}

/// The daemon refuses the `Rebind`: nothing moves. The rotation waits for that answer before
/// it touches anything, so the session keeps the old key and its connection, no new key is
/// adopted, and the user is told it failed — never "rotated".
#[tokio::test]
async fn a_refused_rebind_changes_nothing() {
    let old = Identity::from_seed(&[71u8; 32], "owner");
    let server = Identity::from_seed(&[73u8; 32], "daemon");
    let server_pub = server.public();
    let old_fp = old.fingerprint();
    let seed = owned_e2e_collection_state("kb-rot", &old);
    let (addr, mut seen) = spawn_rotating_hub(server, old.public(), seed.clone(), true).await;
    let key_dir = tempfile::tempdir().unwrap();
    let (cmd_tx, mut evt_rx) =
        start_client(addr, server_pub, &mut seen, &old_fp, seed, key_dir.path()).await;
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();

    let ev = recv_until(&mut evt_rx, |e| {
        matches!(e, CollabEvent::Error { .. } | CollabEvent::Disconnected { .. })
            || matches!(e, CollabEvent::StatusReport { lines } if lines.iter().any(|l| l.contains("New fingerprint")))
    })
    .await;
    match ev {
        CollabEvent::Error { message } => assert!(
            message.contains("accepted no Rebind") && message.contains("NOT changed"),
            "{message}"
        ),
        other => panic!("a refused Rebind still rotated: {other:?}"),
    }
    // Still the old key's session: a later collection op arrives on connection 0, signed by
    // the old key (the next rotation attempt's `Rebind` is the cheapest such op).
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();
    let again = next_seen(&mut seen, |s| {
        matches!(
            s,
            Seen::Op {
                action: MembershipAction::Rebind,
                ..
            }
        )
    })
    .await;
    assert!(
        matches!(&again, Seen::Op { conn: 0, author, .. } if *author == old_fp),
        "still the first connection, still the old key: {again:?}"
    );
}
