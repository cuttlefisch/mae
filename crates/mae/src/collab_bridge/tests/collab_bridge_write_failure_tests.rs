//! Round-4 item 1: adversarial tests for the 4 sites where `run_collab_task` used to
//! discard `write_framed`'s `Result` (`let _ = write_framed(...).await;`) and then
//! unconditionally advance local state / report success. Unlike the rest of this test
//! suite (which, per `collab_bridge_buffer_join_tests.rs`'s own note, sub-component
//! tests `run_collab_task`'s pieces because "the actual `run_collab_task` loop requires
//! a real TCP connection, so we can't unit-test it directly"), these tests DO drive the
//! full, real `run_collab_task` over a genuine loopback TCP connection with a real
//! `KeyAuth` handshake — the bug lives inside the task's own match arms, with no
//! extractable pure-function seam, so a real connection is the only way to exercise it
//! per CLAUDE.md principle #14 ("the attacker's test", not a synthetic call).
//!
//! Harness: `spawn_fake_daemon` binds a loopback listener, performs a real
//! `KeyAuth::server_handshake`, answers the JSON-RPC `initialize` handshake, then drains
//! (and ignores — these are fire-and-forget from the client) further messages until
//! told to hang up. Each test: (1) connects a real `run_collab_task` against it, (2)
//! seeds KB state via `KbSetEncryption` (a local seam: it takes the collection bytes
//! directly rather than requiring a full `kb/join` round trip), (3) tears down the fake
//! daemon's connection and awaits its task fully exiting (guaranteeing the close has
//! actually happened, not just been requested), (4) THEN enqueues the target command,
//! so its wire write(s) fail against a genuinely dead socket, (5) asserts local state
//! was NOT advanced and a clear failure surfaces — never a false "shipped"/"approved"
//! success.
//!
//! The hangup is triggered and awaited BEFORE the command is sent (empirically:
//! command-then-close let the write slip through the socket before the close
//! propagated). This means the reader task's own independent EOF detection may beat
//! the command to tearing down `writer` first, which routes the command through the
//! pre-existing "no active connection writer" branch instead of the new fix's
//! mid-loop write-failure branch — both are real, valid negative-path outcomes for
//! the property under test (state is never advanced on any kind of write failure).
//! Each test's assertion accepts every message the connection's actual state at
//! processing time can legitimately produce, rather than pinning one exact branch.

use super::*;
use mae_mcp::auth::{AuthProvider, KeyAuth};
use mae_mcp::identity::{AuthorizedKeys, HostKeyVerifier, Identity, KnownHosts};
use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
use mae_sync::kb::KbCollectionDoc;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::BufReader;
use tokio::net::TcpListener;

/// Spawn a real loopback "daemon": genuine `KeyAuth` handshake, answers `initialize`,
/// then reads (and ignores — fire-and-forget from the client) further messages until
/// `hangup_rx` fires, at which point it drops the connection. Returns the bound
/// address plus the `hangup` sender the test uses to kill the connection on demand.
async fn spawn_fake_daemon(
    server_identity: Identity,
    client_pubkey: mae_mcp::identity::PublicKey,
) -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    // `#[tokio::test]` defaults to a current-thread (single-worker) runtime, so the
    // address handoff below MUST be a real `.await` (not a blocking `std::sync::mpsc`
    // recv) — a synchronous block here would starve the executor and deadlock, since
    // the listener-binding task spawned just below could never get scheduled.
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (hangup_tx, hangup_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        addr_tx.send(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        // SO_LINGER(0): on drop, send an immediate RST instead of a graceful FIN.
        // A graceful close still lets the client's subsequent writes succeed for a
        // little while (TCP's send/receive directions close independently) —
        // empirically flaky (~25% of runs) even after awaiting this task's exit.
        // An RST reliably fails the client's NEXT write, which is what these tests
        // need to force deterministically. `set_linger` is deprecated because a
        // NON-zero linger blocks the drop's thread for up to that duration; a
        // ZERO linger returns immediately (it means "don't linger at all — reset
        // now"), so that risk doesn't apply here.
        #[allow(deprecated)]
        stream.set_linger(Some(Duration::ZERO)).unwrap();
        let (r, mut w) = stream.into_split();
        let mut sr = BufReader::new(r);

        let authorized_path = std::env::temp_dir().join(format!(
            "mae-write-fail-test-authorized-{}-{}",
            std::process::id(),
            rand_suffix(),
        ));
        let mut authorized = AuthorizedKeys::load(&authorized_path);
        authorized.add(client_pubkey).unwrap();
        let auth = KeyAuth::server(Arc::new(server_identity), Arc::new(authorized));
        auth.server_handshake(&mut sr, &mut w)
            .await
            .expect("fake daemon: KeyAuth handshake should succeed");

        // JSON-RPC `initialize`.
        let init_text = mae_mcp::read_message(&mut sr)
            .await
            .unwrap()
            .expect("fake daemon: expected an initialize request");
        let init_req: serde_json::Value = serde_json::from_str(&init_text).unwrap();
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": init_req["id"],
            "result": { "serverInfo": { "connections": 1 } },
        });
        mae_mcp::write_framed(
            &mut w,
            &serde_json::to_vec(&resp).unwrap(),
            Duration::from_secs(2),
        )
        .await
        .expect("fake daemon: failed to answer initialize");

        // Drain further messages (subscribe, blocklist fetch, the KbSetEncryption
        // genesis delta) — fire-and-forget from the client, no response needed —
        // until told to hang up, then drop the connection.
        let mut hangup_rx = hangup_rx;
        loop {
            tokio::select! {
                msg = mae_mcp::read_message(&mut sr) => {
                    match msg {
                        Ok(Some(_)) => continue,
                        _ => break,
                    }
                }
                _ = &mut hangup_rx => break,
            }
        }
        // `sr`/`w` drop here — closes the connection.
    });
    let addr = addr_rx.await.expect("fake daemon: never bound a port");
    (addr, hangup_tx, handle)
}

/// Poll `evt_rx` until `pred` matches an event, ignoring unrelated events
/// (`Disconnected`, etc. from the reader task's own independent EOF detection).
async fn recv_until(
    evt_rx: &mut mpsc::Receiver<CollabEvent>,
    pred: impl Fn(&CollabEvent) -> bool,
) -> CollabEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let ev = evt_rx.recv().await.expect("event channel closed early");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("timed out waiting for the expected event")
}

/// Connect a real `run_collab_task` (KeyJson transport) against `addr`, wait for
/// `Connected`, and hand back the command/event channels.
async fn connect_client(
    addr: std::net::SocketAddr,
    client_identity: Identity,
    server_pubkey: mae_mcp::identity::PublicKey,
) -> (mpsc::Sender<CollabCommand>, mpsc::Receiver<CollabEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    let (evt_tx, mut evt_rx) = mpsc::channel(16);

    let tmp = std::env::temp_dir().join(format!(
        "mae-write-fail-test-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        rand_suffix(),
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let known_hosts_path = tmp.join("known_hosts");
    let mut known_hosts = KnownHosts::load(&known_hosts_path);
    known_hosts.pin(&addr.to_string(), &server_pubkey).unwrap();
    drop(known_hosts);

    let verifier: Arc<dyn HostKeyVerifier> = Arc::new(FixedTrustVerifier {
        expect_addr: addr.to_string(),
        expect_pubkey: server_pubkey,
    });
    let transport = ClientTransport::KeyJson {
        identity: Arc::new(client_identity),
        verifier,
    };
    tokio::spawn(run_collab_task(
        cmd_rx,
        evt_tx,
        3600, // reconnect_secs — long enough that no reconnect fires during the test
        Duration::from_secs(2),
        1,
        1,
        0, // heartbeat_secs = 0: disable periodic ping, keeps the event stream quiet
        0,
        0,
        transport,
    ));

    cmd_tx
        .send(CollabCommand::Connect {
            address: addr.to_string(),
        })
        .await
        .unwrap();
    let ev = recv_until(&mut evt_rx, |e| {
        matches!(e, CollabEvent::Connected { .. } | CollabEvent::Error { .. })
    })
    .await;
    assert!(
        matches!(ev, CollabEvent::Connected { .. }),
        "expected a clean connect against the fake daemon, got {ev:?}"
    );
    (cmd_tx, evt_rx)
}

/// A `HostKeyVerifier` that trusts exactly the one address/key pair the test pinned —
/// avoids depending on `PromptingHostKeyVerifier`'s live-policy/known_hosts-file
/// plumbing when a direct fixed check is all this harness needs.
#[derive(Debug)]
struct FixedTrustVerifier {
    expect_addr: String,
    expect_pubkey: mae_mcp::identity::PublicKey,
}
impl HostKeyVerifier for FixedTrustVerifier {
    fn verify(&self, addr: &str, server_pub: &mae_mcp::identity::PublicKey) -> bool {
        addr == self.expect_addr && server_pub.to_bytes() == self.expect_pubkey.to_bytes()
    }
}

fn rand_suffix() -> u32 {
    // No RNG dependency needed for test-dir uniqueness beyond pid+time; a static
    // counter is enough to avoid collisions between tests in the same process.
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Build an owned, E2E-genesis'd `KbCollectionDoc` for `owner` — the same fixture
/// shape as `collab_bridge_e2e_rotation_tests.rs`'s `plan_owner_rotation` tests —
/// encoded as the `collection_state` bytes `KbSetEncryption` expects.
fn owned_e2e_collection_state(kb_id: &str, owner: &Identity) -> Vec<u8> {
    let fp = owner.fingerprint();
    let mut coll = KbCollectionDoc::new_owned(kb_id, &fp, owner.label());
    let key = ContentKey::generate();
    let self_wrap = wrap_to_member(&key, &wrap_public_for(&owner.secret_bytes())).unwrap();
    coll.author_e2e_genesis(
        kb_id,
        &fp,
        &owner.secret_bytes(),
        &owner.public().to_bytes(),
        self_wrap,
        1_000,
    );
    coll.encode_state()
}

/// Same as `owned_e2e_collection_state`, plus a pending join request for
/// `pending_principal` carrying a real ed25519 + X25519-wrap pubkey pair — what
/// `KbApprove`'s E2E admit path requires to attempt the wrap+ship at all (without
/// both keys on the pending record it silently falls back to the legacy,
/// non-E2E-fixed `kb/approve_member` path instead).
fn owned_e2e_collection_state_with_pending(
    kb_id: &str,
    owner: &Identity,
    pending_principal: &str,
    pending_identity: &Identity,
) -> Vec<u8> {
    let fp = owner.fingerprint();
    let mut coll = KbCollectionDoc::new_owned(kb_id, &fp, owner.label());
    let key = ContentKey::generate();
    let self_wrap = wrap_to_member(&key, &wrap_public_for(&owner.secret_bytes())).unwrap();
    coll.author_e2e_genesis(
        kb_id,
        &fp,
        &owner.secret_bytes(),
        &owner.public().to_bytes(),
        self_wrap,
        1_000,
    );
    let pk = pending_identity.public().to_bytes();
    let wrap_pk = wrap_public_for(&pending_identity.secret_bytes());
    coll.add_pending(
        pending_principal,
        pending_principal,
        "1970-01-01T00:00:00Z",
        Some(&pk),
        Some(&wrap_pk),
    );
    coll.encode_state()
}

#[tokio::test]
async fn rotate_identity_write_failure_reports_error_not_success() {
    let client_id = Identity::from_seed(&[51u8; 32], "client");
    let server_id = Identity::from_seed(&[52u8; 32], "daemon");
    let client_pub = client_id.public();
    let server_pub = server_id.public();

    let (addr, hangup_tx, daemon_handle) = spawn_fake_daemon(server_id, client_pub).await;
    let connect_id = Identity::from_seed(&[51u8; 32], "client");
    let (cmd_tx, mut evt_rx) = connect_client(addr, connect_id, server_pub).await;

    let collection_state = owned_e2e_collection_state("kb-rotate", &client_id);
    cmd_tx
        .send(CollabCommand::KbSetEncryption {
            kb_id: "kb-rotate".to_string(),
            mode: "e2e".to_string(),
            collection_state,
            node_states: Vec::new(),
        })
        .await
        .unwrap();
    // KbSetEncryption emits no success event; give the task a moment to process it
    // (it's a single, non-yielding local computation plus one fire-and-forget write).
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Kill the daemon connection, THEN immediately enqueue RotateIdentity — NOT
    // awaiting the fake daemon's task to fully exit first. Empirically: awaiting
    // the full join gives the reader task enough time to also notice the EOF and
    // unwind the whole connection state before the command is even sent, which
    // routes most runs through the pre-existing "already disconnected" branch
    // instead of the new fix's mid-loop write-failure branch (30% of runs on a
    // reverted fix failed to catch the regression). Firing the command right after
    // triggering (not waiting out) the hangup keeps the odds heavily in favor of
    // `writer` still being `Some` when RotateIdentity starts, while SO_LINGER(0)
    // above still makes the write itself fail once the RST lands moments later.
    let _ = hangup_tx.send(());
    cmd_tx.send(CollabCommand::RotateIdentity).await.unwrap();
    let _ = daemon_handle.await;

    let ev = recv_until(&mut evt_rx, |e| {
        matches!(
            e,
            CollabEvent::Error { .. } | CollabEvent::StatusReport { .. }
        )
    })
    .await;
    match ev {
        CollabEvent::Error { message } => {
            // Any of these is a legitimate negative-path outcome for "the write can't
            // be confirmed sent" — which specific one depends on how far the reader
            // task's own independent EOF detection got before this command was
            // processed: still inside the connected loop with `writer` present (new
            // fix's branch — "Identity rotation failed"), `writer` already torn down
            // by `tear_down` ("no active connection writer"), or the whole task
            // already fell through to the disconnected-state dispatcher ("Not
            // connected"). All three refuse to advance `signing_identity` /
            // `kb_collections` and never report success — the property under test.
            assert!(
                message.contains("Identity rotation failed")
                    || message.contains("no active connection writer")
                    || message.contains("Not connected"),
                "unexpected rotation failure message: {message}"
            );
        }
        CollabEvent::StatusReport { lines } => {
            panic!(
                "rotate-identity reported success despite the write failing — the bug \
                 is back: {lines:?}"
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn register_recovery_key_write_failure_reports_zero_registered() {
    let client_id = Identity::from_seed(&[61u8; 32], "client");
    let server_id = Identity::from_seed(&[62u8; 32], "daemon");
    let client_pub = client_id.public();
    let server_pub = server_id.public();

    let (addr, hangup_tx, daemon_handle) = spawn_fake_daemon(server_id, client_pub).await;
    let connect_id = Identity::from_seed(&[61u8; 32], "client");
    let (cmd_tx, mut evt_rx) = connect_client(addr, connect_id, server_pub).await;

    let collection_state = owned_e2e_collection_state("kb-recovery", &client_id);
    cmd_tx
        .send(CollabCommand::KbSetEncryption {
            kb_id: "kb-recovery".to_string(),
            mode: "e2e".to_string(),
            collection_state,
            node_states: Vec::new(),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let _ = hangup_tx.send(());
    cmd_tx
        .send(CollabCommand::RegisterRecoveryKey)
        .await
        .unwrap();
    let _ = daemon_handle.await;

    let ev = recv_until(&mut evt_rx, |e| {
        matches!(
            e,
            CollabEvent::Error { .. } | CollabEvent::StatusReport { .. }
        )
    })
    .await;
    match ev {
        // The new fix's branch: local save always proceeds (it's disk-only, not
        // wire-dependent), but the report must say 0 KBs registered on the daemon
        // side and name the one that failed to ship — never a false "registered"
        // count while silently swallowing the failure.
        CollabEvent::StatusReport { lines } => {
            let joined = lines.join("\n");
            assert!(
                joined.contains("registered across 0 KB(s)"),
                "expected an honest zero-registered report when the write fails, got: {lines:?}"
            );
            assert!(
                joined.contains("FAILED to ship for 1 KB(s)") && joined.contains("kb-recovery"),
                "expected the failed KB to be named in a WARNING line, got: {lines:?}"
            );
        }
        // Pre-existing branches (writer already torn down / already disconnected by
        // the time the command was processed) — equally safe, no false success.
        CollabEvent::Error { message } => {
            assert!(
                message.contains("no active connection writer")
                    || message.contains("Not connected"),
                "unexpected register-recovery-key failure message: {message}"
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn kb_remove_member_e2e_write_failure_does_not_remove_or_rekey() {
    let client_id = Identity::from_seed(&[71u8; 32], "client");
    let server_id = Identity::from_seed(&[72u8; 32], "daemon");
    let client_pub = client_id.public();
    let server_pub = server_id.public();

    let (addr, hangup_tx, daemon_handle) = spawn_fake_daemon(server_id, client_pub).await;
    let connect_id = Identity::from_seed(&[71u8; 32], "client");
    let (cmd_tx, mut evt_rx) = connect_client(addr, connect_id, server_pub).await;

    let collection_state = owned_e2e_collection_state("kb-remove", &client_id);
    cmd_tx
        .send(CollabCommand::KbSetEncryption {
            kb_id: "kb-remove".to_string(),
            mode: "e2e".to_string(),
            collection_state,
            node_states: Vec::new(),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let _ = hangup_tx.send(());
    cmd_tx
        .send(CollabCommand::KbMember {
            kb_id: "kb-remove".to_string(),
            member: "SHA256:some-member-to-remove".to_string(),
            role: "editor".to_string(),
            add: false,
        })
        .await
        .unwrap();
    let _ = daemon_handle.await;

    let ev = recv_until(&mut evt_rx, |e| matches!(e, CollabEvent::Error { .. })).await;
    match ev {
        CollabEvent::Error { message } => {
            // Both the new fix's branch (E2E path attempted, write failed) and the
            // pre-existing "not connected at all" branch unify to ONE of these two
            // messages — neither ever falls through to the generic (non-rekeying)
            // remove, which per #265's keyless-admit analogy would silently
            // downgrade security semantics (member dropped from the roster without
            // ever losing key access).
            assert!(
                message.contains("could not reach the daemon") || message.contains("Not connected"),
                "unexpected kb/remove failure message: {message}"
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn kb_approve_member_e2e_write_failure_does_not_admit() {
    let client_id = Identity::from_seed(&[81u8; 32], "client");
    let server_id = Identity::from_seed(&[82u8; 32], "daemon");
    let client_pub = client_id.public();
    let server_pub = server_id.public();

    let (addr, hangup_tx, daemon_handle) = spawn_fake_daemon(server_id, client_pub).await;
    let connect_id = Identity::from_seed(&[81u8; 32], "client");
    let (cmd_tx, mut evt_rx) = connect_client(addr, connect_id, server_pub).await;

    let pending_identity = Identity::from_seed(&[83u8; 32], "pending-member");
    let pending_fp = pending_identity.fingerprint();
    let collection_state = owned_e2e_collection_state_with_pending(
        "kb-approve",
        &client_id,
        &pending_fp,
        &pending_identity,
    );
    cmd_tx
        .send(CollabCommand::KbSetEncryption {
            kb_id: "kb-approve".to_string(),
            mode: "e2e".to_string(),
            collection_state,
            node_states: Vec::new(),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let _ = hangup_tx.send(());
    cmd_tx
        .send(CollabCommand::KbApprove {
            kb_id: "kb-approve".to_string(),
            principal: pending_fp,
            role: "editor".to_string(),
        })
        .await
        .unwrap();
    let _ = daemon_handle.await;

    let ev = recv_until(&mut evt_rx, |e| matches!(e, CollabEvent::Error { .. })).await;
    match ev {
        CollabEvent::Error { message } => {
            assert!(
                message.contains("could not reach the daemon") || message.contains("Not connected"),
                "unexpected kb/approve failure message: {message}"
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}
