//! ADVERSARIAL: a daemon's local blocklist belongs to its operator.
//!
//! The blocklist is this daemon's local trust (ADR-039 A2): it may name
//! principals of any hosted KB, including owners. Only the daemon's own
//! identity, or the unauthenticated loopback of `auth.mode = none`, may read
//! or change it. Any other authenticated client of a shared daemon must not
//! list the hosted KB ids and blocked keys, block another KB's owner, or
//! unblock itself.

use super::*;
use mae_mcp::identity::Identity;

fn msg(method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}

/// A daemon whose operator has blocked `blocked` on KB `kb-a`.
async fn setup() -> (Arc<DocStore>, SharedBroadcaster, String, String) {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let me = Identity::generate("daemon");
    let me_fp = me.fingerprint();
    store.set_signer(Arc::new(me));
    let blocked = fp("mallory");
    store.add_kb_block("kb-a", &blocked).await.unwrap();
    (store, bc, me_fp, blocked)
}

#[tokio::test]
async fn another_authenticated_client_cannot_read_or_change_the_blocklist() {
    let (store, bc, _me, blocked) = setup().await;
    let before = store.all_kb_blocklists().await;
    let mut docs = HashSet::new();
    let cases = [
        msg("kb/blocklist", serde_json::json!({})),
        msg("kb/blocklist", serde_json::json!({"kb_id": "kb-a"})),
        msg(
            "kb/block_principal",
            serde_json::json!({"kb_id": "kb-a", "fingerprint": fp("owner")}),
        ),
        // The blocked principal unblocking itself.
        msg(
            "kb/unblock_principal",
            serde_json::json!({"kb_id": "kb-a", "fingerprint": blocked}),
        ),
    ];
    for who in ["stranger", "mallory"] {
        for m in &cases {
            let r = dispatch_as(&store, &bc, Some(who), Some(&fp(who)), m.clone(), &mut docs).await;
            let err = r
                .error
                .as_ref()
                .unwrap_or_else(|| panic!("{who}: {} was answered: {:?}", m["method"], r.result));
            assert_eq!(
                err.data.as_deref().map(|d| d["kind"].clone()),
                Some(serde_json::json!("not_operator")),
                "{who}: {}",
                m["method"]
            );
            let text = serde_json::to_string(&r).unwrap();
            assert!(
                !text.contains("kb-a"),
                "{who}: the reply names a hosted KB: {text}"
            );
        }
    }
    assert_eq!(store.all_kb_blocklists().await, before);
}

#[tokio::test]
async fn the_daemons_own_identity_manages_the_blocklist() {
    let (store, bc, me, blocked) = setup().await;
    let mut docs = HashSet::new();
    let listed = dispatch_as(
        &store,
        &bc,
        Some("daemon"),
        Some(&me),
        msg("kb/blocklist", serde_json::json!({})),
        &mut docs,
    )
    .await;
    assert_eq!(
        listed.result.unwrap()["blocklist"]["kb-a"],
        serde_json::json!([blocked])
    );
    let un = dispatch_as(
        &store,
        &bc,
        Some("daemon"),
        Some(&me),
        msg(
            "kb/unblock_principal",
            serde_json::json!({"kb_id": "kb-a", "fingerprint": blocked}),
        ),
        &mut docs,
    )
    .await;
    assert!(un.error.is_none(), "{:?}", un.error);
    assert!(store
        .all_kb_blocklists()
        .await
        .get("kb-a")
        .is_none_or(|v| v.is_empty()));
}

/// An authenticated session is never the operator when the daemon has no
/// identity of its own (fail closed); the unauthenticated loopback still is.
#[tokio::test]
async fn without_a_daemon_identity_only_the_loopback_is_the_operator() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let r = dispatch_as(
        &store,
        &bc,
        Some("x"),
        Some(&fp("x")),
        msg("kb/blocklist", serde_json::json!({})),
        &mut docs,
    )
    .await;
    let kind = r.error.and_then(|e| e.data).map(|d| d["kind"].clone());
    assert_eq!(kind, Some(serde_json::json!("not_operator")));
    let r = dispatch_as(
        &store,
        &bc,
        None,
        None,
        msg("kb/blocklist", serde_json::json!({})),
        &mut docs,
    )
    .await;
    assert!(r.error.is_none(), "{:?}", r.error);
}
