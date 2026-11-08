//! ADVERSARIAL: a session authenticated by the plaintext key handshake (or a
//! pre-shared key) is bound to what the handshake proved -- the key's
//! fingerprint, or `psk:<keyid>` -- never to an identity without a principal.
//! A session without a principal is allowed every KB operation, so binding one
//! after a real handshake let ANY authorized key act on EVERY KB.

use super::*;
use mae_mcp::auth::KeyAuth;
use mae_mcp::identity::{AuthorizedKeys, HostKeyVerifier, Identity, PublicKey};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug)]
struct AcceptAny;
impl HostKeyVerifier for AcceptAny {
    fn verify(&self, _addr: &str, _server_pub: &PublicKey) -> bool {
        true
    }
}

/// One connection to a daemon session authenticated with `server`: a key
/// handshake as `client`, then `request`; the response.
async fn call_as(
    store: Arc<DocStore>,
    bc: SharedBroadcaster,
    server: KeyAuth,
    client: Arc<Identity>,
    request: serde_json::Value,
) -> serde_json::Value {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (r, w) = stream.into_split();
        crate::collab_handler::handle_client_with_auth(
            tokio::io::BufReader::new(r),
            w,
            &server,
            store,
            bc,
            std::time::Instant::now(),
            mae_sync::kb::Transport::Hub,
            Arc::new(crate::artifact_store::NoArtifactStore),
            Arc::new(crate::quota::NoQuota),
            crate::kb_query::KbQueryLimits::default(),
            None,
        )
        .await;
    });
    let stream = TcpStream::connect(addr).await.unwrap();
    let (r, mut w) = stream.into_split();
    let mut r = tokio::io::BufReader::new(r);
    let auth = KeyAuth::client(client, "daemon:9473".into(), Arc::new(AcceptAny));
    auth.client_handshake(&mut r, &mut w)
        .await
        .expect("handshake");
    let body = request.to_string();
    mae_mcp::write_framed(&mut w, body.as_bytes(), std::time::Duration::from_secs(5))
        .await
        .unwrap();
    let reply = mae_mcp::read_message(&mut r)
        .await
        .unwrap()
        .expect("a reply");
    drop(w);
    srv.abort();
    serde_json::from_str(&reply).unwrap()
}

#[tokio::test]
async fn a_key_authenticated_session_acts_as_its_key_not_as_everyone() {
    let dir = tempfile::tempdir().unwrap();
    let alice = Arc::new(Identity::generate("alice"));
    let bob = Arc::new(Identity::generate("bob"));
    let mut keys = AuthorizedKeys::load(&dir.path().join("authorized_keys"));
    keys.add(alice.public()).unwrap();
    keys.add(bob.public()).unwrap();
    let keys = Arc::new(keys);
    let daemon = Arc::new(Identity::generate("daemon"));

    let store = test_doc_store();
    let bc = test_broadcaster();
    let alice_fp = alice.public().fingerprint();
    let mut docs = HashSet::new();
    let shared = kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&alice_fp),
        "kb-a",
        "alice",
        &mut docs,
    )
    .await;
    assert!(shared.error.is_none(), "seed: {:?}", shared.error);

    let add = |node: &str| {
        serde_json::json!({"jsonrpc":"2.0","id":7,"method":"kb/collection_node_add",
            "params":{"kb_id":"kb-a","node_id":node,"title":"T"}})
    };
    // Bob holds an authorized key but is not a member of kb-a.
    let bob_reply = call_as(
        store.clone(),
        bc.clone(),
        KeyAuth::server(daemon.clone(), keys.clone()),
        bob,
        add("concept:bob-was-here"),
    )
    .await;
    assert!(
        bob_reply.get("error").is_some(),
        "a non-member key wrote to kb-a over the key handshake: {bob_reply}"
    );
    // Alice, the owner, still can: the session is bound to her key, not refused wholesale.
    let alice_reply = call_as(
        store,
        bc,
        KeyAuth::server(daemon, keys),
        alice,
        add("concept:alice-note"),
    )
    .await;
    assert!(
        alice_reply.get("error").is_none(),
        "the owner was refused: {alice_reply}"
    );
}
