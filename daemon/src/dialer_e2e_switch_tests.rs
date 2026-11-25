//! ADR-116 Amendment 1 and ADR-113 O17 on the mesh: a peer's collection
//! update that switches a KB to E2E is refused whole by the replica holding
//! plaintext it would leave readable (a publication-review revision, or
//! content in version history), through the dialer's own apply path, and is
//! accepted by one holding neither. Kills a switch guard that sits on one hub
//! method only (the dialer applied a peer's flip with a bare `apply_update`).

use super::*;
use mae_daemon::doc_editors::Attribution;
use mae_daemon::doc_store::DocStore;
use mae_daemon::oauth_self_issue::{Credential, PrincipalKind, ValidatedPrincipal};
use mae_mcp::identity::Identity;
use mae_sync::kb::{Encryption, KbCollectionDoc, KbNodeDoc, Role, TransportPolicy};
use serde_json::json;

const PEER: Attribution<'static> = Attribution::MeshPeer("peer");

struct Replica {
    store: Arc<DocStore>,
    owner: String,
    editor: String,
}

/// A replica holding plaintext hub KB `kb` with node `n`, its key the signer.
async fn replica(kb: &str) -> Replica {
    let backend = Arc::new(crate::storage::SqliteBackend::open_memory().unwrap());
    let store = Arc::new(DocStore::new(backend, 500));
    store.set_signer(Arc::new(Identity::generate("replica")));
    let (owner, editor) = (
        Identity::generate("owner").fingerprint(),
        Identity::generate("editor").fingerprint(),
    );
    let mut coll = KbCollectionDoc::new_owned(kb, &owner, "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    coll.upsert_member(&editor, "editor", Role::Editor);
    let _ = coll.add_node("n", "N");
    let by = Attribution::Principal("seed");
    let kbc = format!("kbc:{kb}");
    store
        .share_doc(&kbc, &coll.encode_state(), by)
        .await
        .unwrap();
    let node = KbNodeDoc::new("n", "N", "Body.", &[]).encode_state();
    let name = mae_sync::kb_node_doc_name(kb, "n");
    store.share_doc(&name, &node, by).await.unwrap();
    Replica {
        store,
        owner,
        editor,
    }
}

async fn node_state(r: &Replica, kb: &str) -> Vec<u8> {
    let name = mae_sync::kb_node_doc_name(kb, "n");
    r.store.encode_state(&name).await.unwrap()
}

/// Two authors in turn: the second edit's handoff cuts a history version.
async fn give_history(r: &Replica, kb: &str) {
    let name = mae_sync::kb_node_doc_name(kb, "n");
    for (i, who) in ["alice", "bob"].into_iter().enumerate() {
        let state = node_state(r, kb).await;
        let mut d = KbNodeDoc::from_bytes_with_client_id(&state, 700 + i as u64).unwrap();
        let up = d.set_title(&format!("T{i}"));
        let by = Attribution::Principal(who);
        r.store.apply_update(&name, &up, None, by).await.unwrap();
    }
    assert!(r.store.history_holds_content(kb).await.unwrap());
}

fn person(p: &str) -> ValidatedPrincipal {
    ValidatedPrincipal {
        principal: p.to_string(),
        audience: vec![],
        expires_at: u64::MAX,
        issuer: "https://idp.example".into(),
        subject: format!("sub-{p}"),
        credential: Credential::IdpBound,
        kind: PrincipalKind::Person,
        sign_in: None,
    }
}

/// A publication-review revision of `n`, made the way a client makes one.
async fn give_revision(r: &Replica, kb: &str) {
    let limits = mae_daemon::kb_query::KbQueryLimits::default();
    let call = |who: String, method: &'static str, params: Value| {
        let store = Arc::clone(&r.store);
        async move { mae_daemon::review::dispatch(method, &params, &store, &person(&who), limits).await }
    };
    let on = json!({"kb_id": kb, "review_enabled": true});
    call(r.owner.clone(), "kb/review.set_policy", on)
        .await
        .unwrap();
    let version = KbNodeDoc::from_bytes(&node_state(r, kb).await)
        .unwrap()
        .version();
    let ask = json!({"kb_id": kb, "nodes": [{"node_id": "n", "version": version}], "reason": "R"});
    call(r.editor.clone(), "kb/review.request", ask)
        .await
        .unwrap();
}

async fn is_e2e(r: &Replica, kb: &str) -> bool {
    let state = r.store.encode_state(&format!("kbc:{kb}")).await.unwrap();
    let coll = KbCollectionDoc::from_bytes(&state).unwrap();
    matches!(coll.encryption(), Encryption::E2e)
}

/// The peer's flip, relayed to `r` through the dialer's apply path: whether
/// it left the session diverged.
async fn relay_flip(r: &Replica, kb: &str) -> bool {
    let kbc = format!("kbc:{kb}");
    let state = r.store.encode_state(&kbc).await.unwrap();
    let flip = KbCollectionDoc::from_bytes(&state)
        .unwrap()
        .set_encryption(Encryption::E2e);
    apply_doc(&r.store, None, 0, &kbc, &flip, None, PEER)
        .await
        .is_err()
}

#[tokio::test]
async fn a_mesh_peers_switch_to_e2e_is_refused_over_plaintext_and_accepted_without() {
    let r = replica("kb-hist").await;
    give_history(&r, "kb-hist").await;
    assert!(relay_flip(&r, "kb-hist").await, "history: diverged");
    assert!(
        !is_e2e(&r, "kb-hist").await,
        "history: the flip was applied"
    );
    assert_eq!(r.store.e2e_switches_refused(), 1);

    let r = replica("kb-rev").await;
    give_revision(&r, "kb-rev").await;
    assert!(relay_flip(&r, "kb-rev").await, "revision: diverged");
    assert!(
        !is_e2e(&r, "kb-rev").await,
        "revision: the flip was applied"
    );
    assert_eq!(r.store.e2e_switches_refused(), 1);

    let r = replica("kb-clean").await;
    assert!(!r.store.history_holds_content("kb-clean").await.unwrap());
    assert!(!relay_flip(&r, "kb-clean").await, "neither: not diverged");
    assert!(
        is_e2e(&r, "kb-clean").await,
        "neither: the flip is accepted"
    );
    assert_eq!(r.store.e2e_switches_refused(), 0);
}
