//! ADR-029 Gate C — the projector's own verification suite.
//!
//! Split out of `projector.rs` to stay under the structural ceiling. These are the
//! adversarial cases: a CRDT write must actually reach a Cozo query, re-projection
//! must be idempotent and order-independent, and every class of drift must be
//! DETECTED after being injected deliberately — a verification pass that has never
//! caught anything is indistinguishable from one that cannot.

use super::*;
use crate::storage::SqliteBackend;
use mae_kb::store::SearchHit;
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

struct MemStores(StdMutex<HashMap<String, Arc<CozoKbStore>>>);
impl MemStores {
    fn new() -> Arc<Self> {
        Arc::new(Self(StdMutex::new(HashMap::new())))
    }
}
#[async_trait::async_trait]
impl ProjectionStores for MemStores {
    async fn store_for(&self, kb_id: &str) -> Result<Arc<CozoKbStore>, String> {
        let mut m = self.0.lock().unwrap();
        Ok(Arc::clone(m.entry(kb_id.to_string()).or_insert_with(
            || Arc::new(CozoKbStore::open_mem().unwrap()),
        )))
    }
}

/// Seed a doc store with a collection listing `nodes`, and each node's doc.
async fn seed(doc_store: &Arc<DocStore>, kb_id: &str, nodes: &[(&str, &str, &str)]) {
    let mut coll = mae_sync::kb::KbCollectionDoc::new(kb_id, "owner");
    for (id, title, body) in nodes {
        let n = mae_sync::kb::KbNodeDoc::new(id, title, body, &[]);
        doc_store
            .apply_update(&format!("kbn:kb1:{id}"), &n.encode(), None)
            .await
            .unwrap();
        coll.add_node(id, title);
    }
    doc_store
        .apply_update(&format!("kbc:{kb_id}"), &coll.encode_state(), None)
        .await
        .unwrap();
}

fn mem_store() -> Arc<DocStore> {
    Arc::new(DocStore::new(
        Arc::new(SqliteBackend::open_memory().unwrap()),
        500,
    ))
}

/// GATE C.1 — a CRDT node write is visible to a Cozo-backed query.
///
/// This is the whole point of wiring the projector, and it failed before it: the
/// change feed had no subscriber, so `kb/search` and every other `kb/query.*` caller
/// read a projection that no CRDT write ever reached.
#[tokio::test]
async fn a_crdt_node_write_becomes_visible_to_a_cozo_query() {
    let doc_store = mem_store();
    let stores = MemStores::new();
    let projector = Projector::new(Arc::clone(&doc_store), stores.clone());

    seed(
        &doc_store,
        "kb1",
        &[("concept:rope", "Rope", "the rope buffer structure")],
    )
    .await;
    projector.project_doc("kbc:kb1").await.unwrap();

    let store = stores.store_for("kb1").await.unwrap();
    let hits: Vec<SearchHit> = store.fts_search("rope buffer", 10).unwrap();
    assert!(
        hits.iter().any(|h| h.id == "concept:rope"),
        "a CRDT-written node must be findable through the cozo projection: {hits:?}"
    );

    // And a subsequent EDIT reaches the projection too — not just the initial seed.
    let mut edited = mae_sync::kb::KbNodeDoc::from_bytes(
        &doc_store
            .encode_state_and_sv("kbn:kb1:concept:rope")
            .await
            .unwrap()
            .0,
    )
    .unwrap();
    let upd = edited.set_body("now mentions zippers instead");
    doc_store
        .apply_update("kbn:kb1:concept:rope", &upd, None)
        .await
        .unwrap();
    projector.project_doc("kbn:kb1:concept:rope").await.unwrap();

    let n = store.get_node("concept:rope").unwrap().unwrap();
    assert_eq!(
        n.body, "now mentions zippers instead",
        "an edit after the initial projection must also land"
    );
}

/// GATE C.2 — re-projection is idempotent and order-independent.
///
/// ADR-029's determinism contract: the projection is a pure function of CRDT state,
/// so replaying the same changes in any order converges on the same rows. Without
/// this, a reconciliation pass could itself introduce drift.
#[tokio::test]
async fn reprojection_is_idempotent_and_order_independent() {
    let nodes = [
        ("concept:a", "A", "see [[concept:b]]"),
        ("concept:b", "B", "b body"),
        ("concept:c", "C", "c body with words"),
    ];

    // Two projectors, same CRDT state, different apply orders.
    let mut snapshots = Vec::new();
    for order in [[0usize, 1, 2], [2, 0, 1]] {
        let doc_store = mem_store();
        let stores = MemStores::new();
        let projector = Projector::new(Arc::clone(&doc_store), stores.clone());
        seed(&doc_store, "kb1", &nodes).await;

        projector.project_doc("kbc:kb1").await.unwrap();
        for i in order {
            projector
                .project_doc(&format!("kbn:kb1:{}", nodes[i].0))
                .await
                .unwrap();
        }
        // Project everything a second time — idempotence.
        for i in order {
            projector
                .project_doc(&format!("kbn:kb1:{}", nodes[i].0))
                .await
                .unwrap();
        }

        let store = stores.store_for("kb1").await.unwrap();
        let mut rows: Vec<(String, String, String)> = store
            .list_ids(None)
            .unwrap()
            .into_iter()
            .map(|id| {
                let n = store.get_node(&id).unwrap().unwrap();
                (n.id, n.title, n.body)
            })
            .collect();
        rows.sort();
        // Links must not accumulate on re-projection either.
        let links = store.links_from("concept:a").unwrap();
        assert_eq!(
            links.len(),
            1,
            "re-projecting must replace links, not append: {links:?}"
        );
        snapshots.push(rows);
    }

    assert_eq!(
        snapshots[0], snapshots[1],
        "apply order changed the projection — determinism contract broken"
    );
    assert_eq!(snapshots[0].len(), 3);
}

/// GATE C.3 + C.4 — reconciliation reports zero drift on a healthy projection, and
/// an INJECTED mismatch is both detected and healed.
///
/// The injection is the part that matters. A verification pass that has never caught
/// anything is indistinguishable from one that cannot: asserting "reports clean" on a
/// projection the same code just built proves nothing on its own. So each drift
/// class is forced deliberately and must be seen.
#[tokio::test]
async fn reconciliation_detects_and_heals_every_drift_class() {
    let doc_store = mem_store();
    let stores = MemStores::new();
    let projector = Projector::new(Arc::clone(&doc_store), stores.clone());

    seed(
        &doc_store,
        "kb1",
        &[
            ("concept:a", "A", "a body"),
            ("concept:b", "B", "b body"),
            ("concept:c", "C", "c body"),
        ],
    )
    .await;
    projector.project_doc("kbc:kb1").await.unwrap();

    // Baseline: a freshly built projection has no drift.
    let clean = projector.reconcile_kb("kb1", false).await.unwrap();
    assert!(
        clean.is_clean(),
        "healthy projection reported drift: {clean:?}"
    );

    // Inject all three classes directly into the store, behind the projector's back —
    // exactly what a dropped emit, a failed write or a crash would leave.
    let store = stores.store_for("kb1").await.unwrap();
    store.delete_node("concept:a").unwrap(); // missing
    let mut tampered = store.get_node("concept:b").unwrap().unwrap();
    tampered.body = "TAMPERED — not what the CRDT says".to_string();
    store.insert_node(&tampered).unwrap(); // differing
    let ghost = mae_kb::Node::new("concept:ghost", "Ghost", NodeKind::Note, "not in the CRDT");
    store.insert_node(&ghost).unwrap(); // extra

    // Detection, without healing.
    let drift = projector.reconcile_kb("kb1", false).await.unwrap();
    assert_eq!(drift.missing, vec!["concept:a".to_string()], "{drift:?}");
    assert_eq!(drift.differing, vec!["concept:b".to_string()], "{drift:?}");
    assert_eq!(drift.extra, vec!["concept:ghost".to_string()], "{drift:?}");
    assert_eq!(drift.total(), 3);

    // Still present after a detect-only pass — reporting must not mutate.
    assert!(store.get_node("concept:ghost").unwrap().is_some());

    // Heal, then verify against CRDT truth rather than against the report.
    let healed = projector.reconcile_kb("kb1", true).await.unwrap();
    assert_eq!(healed.total(), 3, "the healing pass reports what it fixed");
    assert_eq!(
        store.get_node("concept:a").unwrap().unwrap().body,
        "a body",
        "missing node restored from CRDT truth"
    );
    assert_eq!(
        store.get_node("concept:b").unwrap().unwrap().body,
        "b body",
        "tampered node restored from CRDT truth"
    );
    assert!(
        store.get_node("concept:ghost").unwrap().is_none(),
        "node absent from CRDT truth removed from the projection"
    );

    // Idempotent: a second pass finds nothing left to do.
    let after = projector.reconcile_kb("kb1", true).await.unwrap();
    assert!(
        after.is_clean(),
        "reconciliation is not idempotent — second pass still reports {after:?}"
    );
}
