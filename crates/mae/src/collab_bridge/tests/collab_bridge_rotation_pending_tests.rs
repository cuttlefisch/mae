//! ADR-040 §4, the failure modes of an identity rotation: the connection drops after a
//! `Rebind` was sent and before the daemon's answer. The daemon may have applied it — then
//! that KB accepts only the new key, and an editor that forgot the key would be locked out —
//! or not, and then the new key must never be adopted for it.
//!
//! Drives the real `run_collab_task` over loopback TCP with a real `KeyAuth` handshake, like
//! `collab_bridge_rotation_reconnect_tests`, against a hub that keeps a membership set per KB
//! and a transport allow-list, and can apply a `Rebind` and drop the connection before
//! answering, drop it unapplied, or refuse it. Like the real daemon (`a_retired_keys_session_
//! gets_nothing_after_rotation`), it refuses a retired key everything, and it answers
//! `kb/query.capabilities` only to a member.

use super::collab_bridge_write_failure_tests::{
    owned_e2e_collection_state, recv_until, FixedTrustVerifier,
};
use super::*;
use mae_mcp::auth::{AuthProvider, KeyAuth};
use mae_mcp::identity::{AuthorizedKeys, HostKeyVerifier, Identity, PublicKey};
use mae_sync::kb::KbCollectionDoc;
use mae_sync::membership::MembershipAction;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::BufReader;
use tokio::net::TcpListener;

/// What the hub does with a `Rebind` it receives.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    /// Apply it and answer.
    Answer,
    /// Apply it, then drop the connection without answering.
    ApplyThenDrop,
    /// Drop the connection without applying or answering.
    DropUnapplied,
    /// Answer with a refusal; apply nothing.
    Refuse,
    /// Apply it, then answer with an `internal` error (a persistence fault after the apply).
    ApplyThenFail,
    /// Answer with an `internal` error; apply nothing.
    FailUnapplied,
}

/// What the hub saw.
#[derive(Debug, Clone)]
enum Seen {
    Authenticated {
        conn: usize,
        fp: String,
    },
    /// A `kb/collection_op`: the session's key, the update as sent, the membership ops it
    /// added (none for a duplicate), and whether it was accepted.
    Request {
        conn: usize,
        kb: String,
        session: String,
        update: String,
        new_ops: Vec<(MembershipAction, String, bool)>,
        accepted: bool,
    },
    Probe {
        kb: String,
        fp: String,
        member: bool,
    },
}

struct Hub {
    /// Keys the transport admits (the daemon's `authorized_keys`).
    authorized: Vec<PublicKey>,
    replicas: HashMap<String, KbCollectionDoc>,
    members: HashMap<String, HashSet<String>>,
    /// Per KB, what to do with each `Rebind` in turn; `Answer` once empty.
    script: HashMap<String, VecDeque<Act>>,
    /// An applied `Rebind` revokes the retired key at the transport.
    revoke_retired: bool,
    /// An applied `Rebind` authorizes its successor at the transport (the operator step).
    authorize_successor: bool,
    /// Any `Rebind` it receives, applied or not, authorizes its successor at the transport:
    /// an operator who authorized the new key ahead of time, so only membership decides.
    authorize_on_sight: bool,
    /// Successor keys seen in applied `Rebind`s.
    successors: Vec<PublicKey>,
    /// When non-zero, a dropping act (`ApplyThenDrop`, `DropUnapplied`) answers nothing but
    /// keeps the connection until this many `Rebind`s have arrived on it, then drops it —
    /// so a multi-KB rotation loses every answer whatever order its KBs are sent in.
    drop_after_rebinds: usize,
}

type Shared = Arc<Mutex<Hub>>;

fn unlabeled(id: &Identity) -> PublicKey {
    PublicKey::from_bytes(&id.public().to_bytes(), None).unwrap()
}

fn ok(id: &serde_json::Value, result: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap()
}

fn refusal(id: &serde_json::Value, kind: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"jsonrpc": "2.0", "id": id,
        "error": {"code": -32603, "message": format!("{kind} (test)"), "data": {"kind": kind}}}))
    .unwrap()
}

impl Hub {
    fn member(&self, kb: &str, fp: &str) -> bool {
        self.members.get(kb).is_some_and(|m| m.contains(fp))
    }

    /// One request: the reply (if any), whether to drop the connection after it, and what to
    /// record.
    fn handle(
        &mut self,
        req: &serde_json::Value,
        fp: &str,
        conn: usize,
    ) -> (Option<Vec<u8>>, bool, Option<Seen>) {
        let id = &req["id"];
        match req["method"].as_str() {
            Some("initialize") => (
                Some(ok(
                    id,
                    serde_json::json!({"serverInfo": {"connections": 1}}),
                )),
                false,
                None,
            ),
            Some("kb/query.capabilities") => {
                let kb = req["params"]["kb_id"].as_str().unwrap().to_string();
                let member = self.member(&kb, fp);
                let body = if member {
                    ok(id, serde_json::json!({"kb_id": kb}))
                } else {
                    refusal(id, "not_member")
                };
                let seen = Seen::Probe {
                    kb,
                    fp: fp.to_string(),
                    member,
                };
                (Some(body), false, Some(seen))
            }
            Some("kb/collection_op") => self.collection_op(req, fp, conn),
            _ => (None, false, None),
        }
    }

    fn collection_op(
        &mut self,
        req: &serde_json::Value,
        fp: &str,
        conn: usize,
    ) -> (Option<Vec<u8>>, bool, Option<Seen>) {
        let id = &req["id"];
        let kb = req["params"]["kb_id"].as_str().unwrap().to_string();
        let update = req["params"]["update"].as_str().unwrap().to_string();
        let delta = mae_sync::encoding::base64_to_update(&update).unwrap();
        let replica = &self.replicas[&kb];
        let before: HashSet<String> = replica.oplog_ops().iter().map(|o| o.chain_hash()).collect();
        let mut probe = KbCollectionDoc::from_bytes(&replica.encode_state()).unwrap();
        probe.apply_update(&delta).unwrap();
        let fresh: Vec<_> = probe
            .oplog_ops()
            .into_iter()
            .filter(|o| !before.contains(&o.chain_hash()))
            .collect();
        let new_ops = fresh
            .iter()
            .map(|o| (o.op.action, o.op.author.clone(), o.op.wrapped_key.is_some()))
            .collect();
        let rebind = fresh
            .iter()
            .find(|o| o.op.action == MembershipAction::Rebind)
            .cloned();
        // A retired (or never admitted) key gets nothing; neither does a duplicate.
        let act = if !self.member(&kb, fp) || fresh.is_empty() {
            Act::Refuse
        } else if rebind.is_some() {
            self.script
                .get_mut(&kb)
                .and_then(|q| q.pop_front())
                .unwrap_or(Act::Answer)
        } else {
            Act::Answer
        };
        if let (true, Some(o)) = (self.authorize_on_sight, &rebind) {
            let pk = PublicKey::from_bytes(&o.op.new_pubkey.unwrap(), None).unwrap();
            if !self
                .authorized
                .iter()
                .any(|k| k.to_bytes() == pk.to_bytes())
            {
                self.authorized.push(pk);
            }
        }
        let apply = matches!(act, Act::Answer | Act::ApplyThenDrop | Act::ApplyThenFail);
        if apply {
            self.replicas.insert(kb.clone(), probe);
            if let Some(o) = &rebind {
                let m = self.members.entry(kb.clone()).or_default();
                m.remove(&o.op.author);
                m.insert(o.op.subject.clone());
                let pk = PublicKey::from_bytes(&o.op.new_pubkey.unwrap(), None).unwrap();
                if self.revoke_retired {
                    let gone = o.op.author.clone();
                    self.authorized.retain(|k| k.fingerprint() != gone);
                }
                if self.authorize_successor
                    && !self
                        .authorized
                        .iter()
                        .any(|k| k.to_bytes() == pk.to_bytes())
                {
                    self.authorized.push(pk.clone());
                }
                self.successors.push(pk);
            }
        }
        let reply = match act {
            Act::Answer => Some(ok(id, serde_json::json!({"applied": true}))),
            Act::Refuse => Some(refusal(id, "role_forbidden")),
            Act::ApplyThenFail | Act::FailUnapplied => Some(refusal(id, "internal")),
            Act::ApplyThenDrop | Act::DropUnapplied => None,
        };
        let drop_after =
            matches!(act, Act::ApplyThenDrop | Act::DropUnapplied) && self.drop_after_rebinds == 0;
        let seen = Seen::Request {
            conn,
            kb,
            session: fp.to_string(),
            update,
            new_ops,
            accepted: apply,
        };
        (reply, drop_after, Some(seen))
    }
}

/// Serve one connection: a real `KeyAuth` handshake against the hub's CURRENT allow-list,
/// then requests until the client hangs up or the hub drops it.
async fn serve(
    hub: Shared,
    server: Arc<Identity>,
    stream: tokio::net::TcpStream,
    conn: usize,
    seen: mpsc::Sender<Seen>,
) {
    let (r, mut w) = stream.into_split();
    let mut sr = BufReader::new(r);
    let tmp = tempfile::tempdir().unwrap();
    let mut keys = AuthorizedKeys::load(&tmp.path().join("authorized_keys"));
    let allowed = hub.lock().unwrap().authorized.clone();
    for k in allowed {
        keys.add(k).unwrap();
    }
    let auth = KeyAuth::server(server, Arc::new(keys));
    let Ok(result) = auth.server_handshake(&mut sr, &mut w).await else {
        return; // a key this hub does not authorize: refused, as a daemon would
    };
    let fp = result.peer_identity().fingerprint;
    let _ = seen
        .send(Seen::Authenticated {
            conn,
            fp: fp.clone(),
        })
        .await;
    let mut rebinds = 0usize;
    while let Ok(Some(text)) = mae_mcp::read_message(&mut sr).await {
        let req: serde_json::Value = serde_json::from_str(&text).unwrap();
        let (reply, drop_after, event) = {
            let mut h = hub.lock().unwrap();
            let out = h.handle(&req, &fp, conn);
            if out.2.as_ref().is_some_and(carries_rebind) {
                rebinds += 1;
            }
            let held = h.drop_after_rebinds;
            (out.0, out.1 || (held > 0 && rebinds == held), out.2)
        };
        if let Some(e) = event {
            let _ = seen.send(e).await;
        }
        if let Some(body) = reply {
            if mae_mcp::write_framed(&mut w, &body, Duration::from_secs(2))
                .await
                .is_err()
            {
                return;
            }
        }
        if drop_after {
            return;
        }
    }
}

/// Spawn the hub. Every connection is served concurrently: a pending rotation's membership
/// question rides its own connection while the session's is open.
async fn spawn_hub(server: Identity, hub: Shared) -> (std::net::SocketAddr, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (seen_tx, seen_rx) = mpsc::channel(256);
    let server = Arc::new(server);
    tokio::spawn(async move {
        let mut conn = 0usize;
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve(
                hub.clone(),
                server.clone(),
                stream,
                conn,
                seen_tx.clone(),
            ));
            conn += 1;
        }
    });
    (addr, seen_rx)
}

/// A hub hosting `kbs`, each owned by `owner`, whose only member and only authorized key is
/// `owner` unless the test says otherwise. Returns the hub and each KB's seed state.
fn hub_for(owner: &Identity, kbs: &[&str]) -> (Shared, Vec<(String, Vec<u8>)>) {
    let mut replicas = HashMap::new();
    let mut members = HashMap::new();
    let mut seeds = Vec::new();
    for kb in kbs {
        let seed = owned_e2e_collection_state(kb, owner);
        replicas.insert(kb.to_string(), KbCollectionDoc::from_bytes(&seed).unwrap());
        members.insert(kb.to_string(), HashSet::from([owner.fingerprint()]));
        seeds.push((kb.to_string(), seed));
    }
    let hub = Hub {
        authorized: vec![unlabeled(owner)],
        replicas,
        members,
        script: HashMap::new(),
        revoke_retired: false,
        authorize_successor: false,
        authorize_on_sight: false,
        successors: Vec::new(),
        drop_after_rebinds: 0,
    };
    (Arc::new(Mutex::new(hub)), seeds)
}

const OLD_SEED: [u8; 32] = [81u8; 32];

fn old_key() -> Identity {
    Identity::from_seed(&OLD_SEED, "owner")
}

/// Start a real `run_collab_task` authenticated as the old key, with `slot` as its rotation
/// state, connect it, and seed `seeds` (owned E2e KBs, with their content keys).
async fn start(
    addr: std::net::SocketAddr,
    server_pub: PublicKey,
    slot: rotation::Slot,
    seeds: &[(String, Vec<u8>)],
) -> (
    mpsc::Sender<CollabCommand>,
    mpsc::Receiver<CollabEvent>,
    tokio::task::JoinHandle<()>,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    let (evt_tx, mut evt_rx) = mpsc::channel(256);
    let verifier: Arc<dyn HostKeyVerifier> = Arc::new(FixedTrustVerifier::new(addr, server_pub));
    let transport = ClientTransport::KeyJson {
        identity: Arc::new(old_key()),
        verifier,
        rotation: slot,
    };
    // reconnect_secs = 1: reconnects and membership retries fire within the test's patience.
    let task = tokio::spawn(run_collab_task(
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
    for (kb, seed) in seeds {
        cmd_tx
            .send(CollabCommand::KbSetEncryption {
                kb_id: kb.clone(),
                mode: "e2e".to_string(),
                collection_state: seed.clone(),
                node_states: Vec::new(),
            })
            .await
            .unwrap();
    }
    // Barrier: `ShowStatus` is answered by the same command loop, in order. Only with KBs
    // to seed — it would otherwise swallow events a test waits for.
    if !seeds.is_empty() {
        cmd_tx.send(CollabCommand::ShowStatus).await.unwrap();
        let _ = recv_until(&mut evt_rx, is_status).await;
    }
    (cmd_tx, evt_rx, task)
}

/// The answer to `ShowStatus`.
fn is_status(e: &CollabEvent) -> bool {
    matches!(e, CollabEvent::StatusReport { lines }
        if lines.first().is_some_and(|l| l == "Collaborative Editing Status"))
}

/// Everything the hub sees until `pred` matches (inclusive).
async fn seen_until(rx: &mut mpsc::Receiver<Seen>, pred: impl Fn(&Seen) -> bool) -> Vec<Seen> {
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut all = Vec::new();
        loop {
            let s = rx.recv().await.expect("the hub stopped");
            let done = pred(&s);
            all.push(s);
            if done {
                return all;
            }
        }
    })
    .await
    .expect("timed out waiting on the hub")
}

async fn event_until(
    rx: &mut mpsc::Receiver<CollabEvent>,
    pred: impl Fn(&CollabEvent) -> bool,
) -> CollabEvent {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let e = rx.recv().await.expect("event channel closed");
            if pred(&e) {
                return e;
            }
        }
    })
    .await
    .expect("timed out waiting for the event")
}

fn is_rewrap(s: &Seen, kb: &str) -> bool {
    matches!(s, Seen::Request { kb: k, new_ops, .. }
        if k == kb && new_ops.iter().any(|(a, _, wraps)| *a == MembershipAction::Admit && *wraps))
}

/// A request carrying a `Rebind`: one that adds it, or a duplicate of one (adds nothing).
fn carries_rebind(s: &Seen) -> bool {
    matches!(s, Seen::Request { new_ops, .. }
        if new_ops.is_empty() || new_ops.iter().any(|(a, _, _)| *a == MembershipAction::Rebind))
}

/// The old key's `Rebind` requests for `kb` (not the set-up's other collection ops).
fn rebind_requests<'a>(all: &'a [Seen], kb: &str) -> Vec<&'a Seen> {
    all.iter()
        .filter(|s| {
            carries_rebind(s)
                && matches!(s, Seen::Request { kb: k, session, .. } if k == kb && *session == old_key().fingerprint())
        })
        .collect()
}

fn interrupted(e: &CollabEvent) -> bool {
    matches!(e, CollabEvent::Error { message } if message.contains("Identity rotation interrupted"))
}

fn rotated(e: &CollabEvent) -> bool {
    matches!(e, CollabEvent::StatusReport { lines } if lines.iter().any(|l| l.contains("New fingerprint")))
}

/// The session ends up authenticated as `new_fp`, the owner's re-wrap for `kb` arrives on
/// that session signed by it, the identity file holds the new key, and the pending record is
/// gone. Returns everything the hub saw.
async fn assert_committed_to(
    seen: &mut mpsc::Receiver<Seen>,
    cmd_tx: &mpsc::Sender<CollabCommand>,
    evt_rx: &mut mpsc::Receiver<CollabEvent>,
    key_dir: &std::path::Path,
    new_fp: &str,
    kb: &str,
) -> Vec<Seen> {
    let all = seen_until(seen, |s| is_rewrap(s, kb)).await;
    let session = all
        .iter()
        .rev()
        .find_map(|s| match s {
            Seen::Authenticated { conn, fp } => Some((*conn, fp.clone())),
            _ => None,
        })
        .expect("a session");
    let Some(Seen::Request {
        conn,
        session: by,
        new_ops,
        ..
    }) = all.last()
    else {
        unreachable!()
    };
    assert_eq!(session.1, new_fp, "the session is the NEW key's");
    assert_eq!(*conn, session.0, "the re-wrap rides the new key's session");
    assert_eq!(by, new_fp, "sent by the new key's session");
    assert!(
        new_ops.iter().all(|(_, author, _)| author == new_fp),
        "signed by the new key: {new_ops:?}"
    );
    // The re-wrap is flushed before the loop takes its next command, so a status answered
    // after it proves the record was removed.
    cmd_tx.send(CollabCommand::ShowStatus).await.unwrap();
    let _ = event_until(evt_rx, is_status).await;
    let saved = Identity::load_secret(key_dir, "owner").expect("the identity file");
    assert_eq!(saved.fingerprint(), new_fp, "the new key is the identity");
    assert!(
        !key_dir.join("rotation_pending").exists(),
        "a committed rotation leaves no pending record"
    );
    all
}

/// (a) The daemon applied the `Rebind`, and the connection dropped before its answer. The
/// old key still connects, but the KB retired it, so the re-sent `Rebind` is refused — which
/// alone cannot tell "applied" from "never will be". The editor keeps the new key, asks as
/// the new key once the operator has authorized it, finds it is a member, and commits.
/// Before the fix, it discarded the new key here: the KB's only seat was lost.
#[tokio::test]
async fn a_rebind_applied_before_a_lost_answer_is_kept_and_committed() {
    let old = old_key();
    let old_fp = old.fingerprint();
    let server = Identity::from_seed(&[82u8; 32], "daemon");
    let server_pub = server.public();
    let (hub, seeds) = hub_for(&old, &["kb-a"]);
    hub.lock()
        .unwrap()
        .script
        .insert("kb-a".into(), VecDeque::from([Act::ApplyThenDrop]));
    let (addr, mut seen) = spawn_hub(server, hub.clone()).await;
    let key_dir = tempfile::tempdir().unwrap();
    let slot = rotation::Slot::new(Some(key_dir.path().to_path_buf()));
    let (cmd_tx, mut evt_rx, _task) = start(addr, server_pub, slot, &seeds).await;
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();

    let ev = event_until(&mut evt_rx, |e| {
        interrupted(e) || rotated(e) || matches!(e, CollabEvent::Error { .. })
    })
    .await;
    assert!(interrupted(&ev), "the drop is reported as unknown: {ev:?}");
    // The old key reconnects and re-sends; the KB refuses its retired key.
    let before = seen_until(
        &mut seen,
        |s| matches!(s, Seen::Request { session, accepted: false, .. } if *session == old_fp),
    )
    .await;
    assert!(
        before.iter().any(
            |s| matches!(s, Seen::Request { accepted: true, session, .. } if *session == old_fp)
        ),
        "the original was applied: {before:?}"
    );
    assert!(
        key_dir.path().join("rotation_pending").exists(),
        "a refused re-send does not discard the key"
    );
    // The operator authorizes the new key.
    let successor = hub.lock().unwrap().successors[0].clone();
    let new_fp = successor.fingerprint();
    hub.lock().unwrap().authorized.push(successor);
    let _ = event_until(&mut evt_rx, rotated).await;
    let all = assert_committed_to(
        &mut seen,
        &cmd_tx,
        &mut evt_rx,
        key_dir.path(),
        &new_fp,
        "kb-a",
    )
    .await;
    assert!(
        all.iter().any(
            |s| matches!(s, Seen::Probe { fp, kb, member: true } if *fp == new_fp && kb == "kb-a")
        ),
        "the outcome came from the daemon, asked as the new key"
    );
}

/// (b) The connection dropped before the daemon applied the `Rebind`. Asked right away, the
/// daemon says the new key is not a member — not final, since the original might still land
/// — so the editor reconnects as the old key and re-sends the SAME signed `Rebind`, byte for
/// byte; the daemon accepts it, and the rotation completes.
#[tokio::test]
async fn a_rebind_lost_unapplied_is_resent_byte_for_byte_and_completes() {
    let old = old_key();
    let server = Identity::from_seed(&[83u8; 32], "daemon");
    let server_pub = server.public();
    let (hub, seeds) = hub_for(&old, &["kb-a"]);
    {
        let mut h = hub.lock().unwrap();
        h.script
            .insert("kb-a".into(), VecDeque::from([Act::DropUnapplied]));
        h.authorize_on_sight = true;
    }
    let (addr, mut seen) = spawn_hub(server, hub.clone()).await;
    let key_dir = tempfile::tempdir().unwrap();
    let slot = rotation::Slot::new(Some(key_dir.path().to_path_buf()));
    let (cmd_tx, mut evt_rx, _task) = start(addr, server_pub, slot, &seeds).await;
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();

    let ev = event_until(&mut evt_rx, |e| {
        interrupted(e) || rotated(e) || matches!(e, CollabEvent::Error { .. })
    })
    .await;
    assert!(interrupted(&ev), "{ev:?}");
    assert!(
        key_dir.path().join("rotation_pending").exists(),
        "the new key is kept while the outcome is unknown"
    );
    let _ = event_until(&mut evt_rx, rotated).await;
    let new_fp = hub.lock().unwrap().successors[0].fingerprint();
    let all = assert_committed_to(
        &mut seen,
        &cmd_tx,
        &mut evt_rx,
        key_dir.path(),
        &new_fp,
        "kb-a",
    )
    .await;
    assert!(
        all.iter()
            .any(|s| matches!(s, Seen::Probe { fp, member: false, .. } if *fp == new_fp)),
        "asked before the re-send, and told no: {all:?}"
    );
    let sent = rebind_requests(&all, "kb-a");
    assert_eq!(sent.len(), 2, "sent, then re-sent once: {sent:?}");
    let (
        Seen::Request {
            update: a,
            conn: c1,
            accepted: false,
            ..
        },
        Seen::Request {
            update: b,
            conn: c2,
            accepted: true,
            ..
        },
    ) = (sent[0], sent[1])
    else {
        panic!("dropped, then accepted: {sent:?}")
    };
    assert_eq!(a, b, "the re-send is the same signed Rebind, byte for byte");
    assert_ne!(c1, c2, "on a new connection");
}

/// (b′) The connection dropped before the daemon applied the `Rebind`, and the daemon
/// refuses the re-send. That refusal alone is ambiguous (a retired key is refused too), so
/// the editor asks as the new key — AFTER the refused re-send — and the daemon says it is no
/// member. Nothing changes: the key is discarded, no identity file is written, and the
/// session is still the old key's.
#[tokio::test]
async fn a_lost_rebind_refused_on_resend_and_never_applied_changes_nothing() {
    let old = old_key();
    let old_fp = old.fingerprint();
    let server = Identity::from_seed(&[84u8; 32], "daemon");
    let server_pub = server.public();
    let (hub, seeds) = hub_for(&old, &["kb-a"]);
    let (addr, mut seen) = spawn_hub(server, hub.clone()).await;
    let key_dir = tempfile::tempdir().unwrap();
    let slot = rotation::Slot::new(Some(key_dir.path().to_path_buf()));
    let (cmd_tx, mut evt_rx, _task) = start(addr, server_pub, slot, &seeds).await;
    // The operator authorized the new key at the transport, so membership is all that
    // decides.
    {
        let mut h = hub.lock().unwrap();
        h.script.insert(
            "kb-a".into(),
            VecDeque::from([Act::DropUnapplied, Act::Refuse]),
        );
        h.authorize_on_sight = true;
    }
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();
    let first = seen_until(&mut seen, carries_rebind).await;
    let Some(Seen::Request { update, .. }) = first.last() else {
        unreachable!()
    };
    let delta = mae_sync::encoding::base64_to_update(update).unwrap();
    let mut probe = KbCollectionDoc::from_bytes(&seeds[0].1).unwrap();
    probe.apply_update(&delta).unwrap();
    let new_fp = probe
        .oplog_ops()
        .into_iter()
        .find(|o| o.op.action == MembershipAction::Rebind)
        .unwrap()
        .op
        .subject;

    let ev = event_until(&mut evt_rx, |e| {
        rotated(e)
            || matches!(e, CollabEvent::Error { message } if message.contains("accepted no Rebind"))
    })
    .await;
    match &ev {
        CollabEvent::Error { message } => assert!(message.contains("NOT changed"), "{message}"),
        other => panic!("a never-applied Rebind rotated the identity: {other:?}"),
    }
    // Everything the hub saw before the client's verdict is already queued.
    let mut all = Vec::new();
    while let Ok(s) = seen.try_recv() {
        all.push(s);
    }
    // The deciding question was asked after the refused re-send.
    let resend = all
        .iter()
        .position(
            |s| matches!(s, Seen::Request { accepted: false, session, .. } if *session == old_fp),
        )
        .expect("the re-send");
    let last_probe = all
        .iter()
        .rposition(|s| matches!(s, Seen::Probe { fp, member: false, .. } if *fp == new_fp))
        .expect("asked as the new key");
    assert!(
        resend < last_probe,
        "asked after the refused re-send: {all:?}"
    );
    assert!(
        !key_dir.path().join("rotation_pending").exists(),
        "a refused rotation leaves no pending key"
    );
    assert!(
        Identity::load_secret(key_dir.path(), "owner").is_none(),
        "no identity file was written"
    );
    // Still the old key's session: a new rotation is accepted and its Rebind is signed by
    // the old key.
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();
    let again = seen_until(&mut seen, carries_rebind).await;
    assert!(
        matches!(again.last(), Some(Seen::Request { session, new_ops, .. })
            if *session == old_fp && new_ops.iter().any(|(a, author, _)| *a == MembershipAction::Rebind && *author == old_fp)),
        "{again:?}"
    );
}

/// The restart path. The daemon applied the `Rebind`, the answer was lost, and the editor
/// stopped before it could find out (the new key was not authorized yet). The next run
/// loads the pending rotation for the identity it starts with, asks as the new key once the
/// operator has authorized it, and commits to the SAME key the daemon accepted.
#[tokio::test]
async fn a_rotation_pending_at_restart_is_resolved_and_committed() {
    let old = old_key();
    let server = Identity::from_seed(&[85u8; 32], "daemon");
    let server_pub = server.public();
    let (hub, seeds) = hub_for(&old, &["kb-a"]);
    {
        let mut h = hub.lock().unwrap();
        h.script
            .insert("kb-a".into(), VecDeque::from([Act::ApplyThenDrop]));
        h.revoke_retired = true;
    }
    let (addr, mut seen) = spawn_hub(server, hub.clone()).await;
    let key_dir = tempfile::tempdir().unwrap();
    let slot = rotation::Slot::new(Some(key_dir.path().to_path_buf()));
    let (cmd_tx, mut evt_rx, task) = start(addr, server_pub.clone(), slot, &seeds).await;
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();
    let ev = event_until(&mut evt_rx, |e| {
        interrupted(e) || rotated(e) || matches!(e, CollabEvent::Error { .. })
    })
    .await;
    assert!(interrupted(&ev), "{ev:?}");
    // The first run ends with the outcome still unknown.
    task.abort();
    let _ = task.await;
    drop((cmd_tx, evt_rx));
    assert!(key_dir.path().join("rotation_pending").exists());
    assert!(Identity::load_secret(key_dir.path(), "owner").is_none());

    // The operator authorizes the new key; the editor starts again with the old identity.
    let successor = hub.lock().unwrap().successors[0].clone();
    let new_fp = successor.fingerprint();
    hub.lock().unwrap().authorized.push(successor);
    let slot = rotation::Slot::load(key_dir.path().to_path_buf(), &old_key());
    assert!(
        slot.is_pending(),
        "the restart picks up the pending rotation"
    );
    let (cmd_tx, mut evt_rx, _task) = start(addr, server_pub, slot, &[]).await;
    let _ = event_until(&mut evt_rx, rotated).await;
    assert_committed_to(
        &mut seen,
        &cmd_tx,
        &mut evt_rx,
        key_dir.path(),
        &new_fp,
        "kb-a",
    )
    .await;
}

/// Two KBs, one rotation, both answers lost: the daemon applied kb-a's `Rebind`, not kb-b's,
/// and refuses kb-b's re-send. The new key's membership settles kb-a (applied) before the
/// re-send, so only kb-b is re-sent; then kb-b is settled as refused. The session moves to the
/// new key for kb-a, kb-b is reported as still on the old key, and nothing signed by the new
/// key — no re-wrap, no op — is ever sent for kb-b.
#[tokio::test]
async fn a_partially_applied_rotation_adopts_only_the_kbs_that_accepted_it() {
    let old = old_key();
    let server = Identity::from_seed(&[86u8; 32], "daemon");
    let server_pub = server.public();
    let (hub, seeds) = hub_for(&old, &["kb-a", "kb-b"]);
    {
        let mut h = hub.lock().unwrap();
        // Both answers are lost, in either send order: kb-a applied, kb-b not.
        h.script
            .insert("kb-a".into(), VecDeque::from([Act::ApplyThenDrop]));
        h.script.insert(
            "kb-b".into(),
            VecDeque::from([Act::DropUnapplied, Act::Refuse, Act::Refuse]),
        );
        h.drop_after_rebinds = 2;
        h.authorize_successor = true;
    }
    let (addr, mut seen) = spawn_hub(server, hub.clone()).await;
    let key_dir = tempfile::tempdir().unwrap();
    let slot = rotation::Slot::new(Some(key_dir.path().to_path_buf()));
    let (cmd_tx, mut evt_rx, _task) = start(addr, server_pub, slot, &seeds).await;
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();

    let report = event_until(&mut evt_rx, |e| {
        rotated(e)
            || matches!(e, CollabEvent::Error { message } if message.contains("accepted no Rebind"))
    })
    .await;
    let CollabEvent::StatusReport { lines } = &report else {
        panic!("kb-a accepted the Rebind, so the rotation commits: {report:?}")
    };
    assert!(
        lines
            .iter()
            .any(|l| l.contains("still on the OLD key") && l.contains("kb-b")),
        "{lines:?}"
    );
    let new_fp = hub.lock().unwrap().successors[0].fingerprint();
    let all = assert_committed_to(
        &mut seen,
        &cmd_tx,
        &mut evt_rx,
        key_dir.path(),
        &new_fp,
        "kb-a",
    )
    .await;
    let kb_a = rebind_requests(&all, "kb-a");
    assert_eq!(
        kb_a.len(),
        1,
        "kb-a's Rebind is not re-sent once applied: {kb_a:?}"
    );
    assert!(
        !all.iter().any(
            |s| matches!(s, Seen::Request { kb, session, .. } if kb == "kb-b" && *session == new_fp)
        ),
        "nothing is sent for kb-b on the new key's session: {all:?}"
    );
    assert!(
        hub.lock().unwrap().member("kb-b", &old.fingerprint()),
        "kb-b still has the old key"
    );
}

/// The daemon applied the `Rebind` and then answered with an `internal` error (persisting
/// failed after the apply). That error is not a refusal: before the fix it settled the KB as
/// refused and, with no KB left, deleted the record — the new key the daemon now holds was
/// lost. The editor must ask as the new key, find it is a member, and commit.
#[tokio::test]
async fn an_internal_error_after_the_rebind_was_applied_is_settled_by_the_new_key() {
    let old = old_key();
    let server = Identity::from_seed(&[87u8; 32], "daemon");
    let server_pub = server.public();
    let (hub, seeds) = hub_for(&old, &["kb-a"]);
    {
        let mut h = hub.lock().unwrap();
        h.script
            .insert("kb-a".into(), VecDeque::from([Act::ApplyThenFail]));
        h.authorize_successor = true;
    }
    let (addr, mut seen) = spawn_hub(server, hub.clone()).await;
    let key_dir = tempfile::tempdir().unwrap();
    let slot = rotation::Slot::new(Some(key_dir.path().to_path_buf()));
    let (cmd_tx, mut evt_rx, _task) = start(addr, server_pub, slot, &seeds).await;
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();

    let ev = event_until(&mut evt_rx, |e| {
        rotated(e)
            || matches!(e, CollabEvent::Error { message } if message.contains("accepted no Rebind"))
    })
    .await;
    assert!(
        rotated(&ev),
        "the daemon holds the Rebind, so the key is kept: {ev:?}"
    );
    let new_fp = hub.lock().unwrap().successors[0].fingerprint();
    let all = assert_committed_to(
        &mut seen,
        &cmd_tx,
        &mut evt_rx,
        key_dir.path(),
        &new_fp,
        "kb-a",
    )
    .await;
    assert!(
        all.iter().any(
            |s| matches!(s, Seen::Probe { fp, kb, member: true } if *fp == new_fp && kb == "kb-a")
        ),
        "settled by asking the new key: {all:?}"
    );
}

/// The verdict of a rotation whose only KB answered the original `Rebind` with `answer`:
/// the event, and everything the hub saw until then. The new key is authorized at the
/// transport on sight, so a membership question, if asked, is answered.
async fn verdict_after(
    answer: Act,
    server_seed: u8,
) -> (CollabEvent, Vec<Seen>, tempfile::TempDir) {
    let old = old_key();
    let server = Identity::from_seed(&[server_seed; 32], "daemon");
    let server_pub = server.public();
    let (hub, seeds) = hub_for(&old, &["kb-a"]);
    {
        let mut h = hub.lock().unwrap();
        h.script.insert("kb-a".into(), VecDeque::from([answer]));
        h.authorize_on_sight = true;
    }
    let (addr, mut seen) = spawn_hub(server, hub.clone()).await;
    let key_dir = tempfile::tempdir().unwrap();
    let slot = rotation::Slot::new(Some(key_dir.path().to_path_buf()));
    let (cmd_tx, mut evt_rx, _task) = start(addr, server_pub, slot, &seeds).await;
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();
    let ev = event_until(&mut evt_rx, |e| {
        rotated(e)
            || matches!(e, CollabEvent::Error { message } if message.contains("accepted no Rebind"))
    })
    .await;
    // Everything the hub saw before the client's verdict is already queued.
    let mut all = Vec::new();
    while let Ok(s) = seen.try_recv() {
        all.push(s);
    }
    assert!(
        hub.lock().unwrap().member("kb-a", &old.fingerprint()),
        "the KB still has the old key"
    );
    (ev, all, key_dir)
}

/// A refusal the daemon makes before applying anything (`role_forbidden`) is still final:
/// the rotation ends at once, without asking the new key, and leaves nothing behind.
#[tokio::test]
async fn a_refusal_before_applying_is_final_without_asking_the_new_key() {
    let (ev, all, key_dir) = verdict_after(Act::Refuse, 88).await;
    assert!(
        matches!(&ev, CollabEvent::Error { message } if message.contains("NOT changed")),
        "{ev:?}"
    );
    assert!(
        !all.iter().any(|s| matches!(s, Seen::Probe { .. })),
        "a final refusal asks nothing: {all:?}"
    );
    assert!(!key_dir.path().join("rotation_pending").exists());
    assert!(Identity::load_secret(key_dir.path(), "owner").is_none());
}

/// An `internal` error where the daemon applied nothing: asking the new key settles it as
/// refused, and the rotation ends without adopting the key.
#[tokio::test]
async fn an_internal_error_with_nothing_applied_is_settled_as_refused() {
    let (ev, all, key_dir) = verdict_after(Act::FailUnapplied, 89).await;
    assert!(
        matches!(&ev, CollabEvent::Error { message } if message.contains("NOT changed")),
        "{ev:?}"
    );
    assert!(
        all.iter()
            .any(|s| matches!(s, Seen::Probe { member: false, .. })),
        "settled by asking the new key: {all:?}"
    );
    assert!(!key_dir.path().join("rotation_pending").exists());
    assert!(Identity::load_secret(key_dir.path(), "owner").is_none());
}
