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
                                           // "extra" = a node the collection REMOVED, which is what this class means.
                                           // Its `kbn:` doc therefore exists while the manifest no longer lists it --
                                           // C6's discriminator. A row with no node doc at all is a different thing (a
                                           // node the CRDT never knew about, e.g. every node in an unshared KB) and is
                                           // deliberately NOT deleted; see `heal_keeps_a_node_that_was_never_crdt_managed`.
    let ghost = mae_kb::Node::new("concept:ghost", "Ghost", NodeKind::Note, "not in the CRDT");
    store.insert_node(&ghost).unwrap(); // extra
    let ghost_doc = mae_sync::kb::KbNodeDoc::new("concept:ghost", "Ghost", "not in the CRDT", &[]);
    doc_store
        .apply_update("kbn:kb1:concept:ghost", &ghost_doc.encode(), None)
        .await
        .unwrap();

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

/// #730 — **every** field the projector writes must be drift-detectable.
///
/// The comparison used to check title/body/tags only, so drift in any field
/// ADR-093 added — `kind`, `todo_state`, `priority`, `aliases`, `properties`,
/// `source_version` — reported a CLEAN projection. This file's own header says
/// a verification pass that has never caught anything is indistinguishable from
/// one that cannot; that was literally true for two thirds of the schema.
///
/// Written as a sweep rather than a case per field on purpose: the failure mode
/// is "a field nobody thought to compare", and a hand-listed set of assertions
/// reproduces exactly that blind spot. Each iteration corrupts ONE field of the
/// projected row and requires the reconciler to notice.
#[tokio::test]
async fn every_projected_field_is_drift_detectable() {
    let doc_store = mem_store();
    let stores = MemStores::new();

    // A node carrying a value in every v2 field, so corrupting any one of them
    // is a real change rather than a no-op against a default.
    let mut node = mae_sync::kb::KbNodeDoc::new("n1", "Title", "Body", &["tag".into()]);
    let _ = node.set_kind(Some("concept"));
    let _ = node.set_todo_state(Some("TODO"));
    let _ = node.set_priority(Some("A"));
    let _ = node.set_aliases(&["alias-one".into()]);
    let _ = node.set_source_version(Some(7));
    let mut props = std::collections::HashMap::new();
    props.insert("role".to_string(), "original".to_string());
    let _ = node.set_properties(&props);

    let mut coll = mae_sync::kb::KbCollectionDoc::new("kb1", "owner");
    coll.add_node("n1", "Title");
    doc_store
        .apply_update("kbn:kb1:n1", &node.encode(), None)
        .await
        .unwrap();
    doc_store
        .apply_update("kbc:kb1", &coll.encode_state(), None)
        .await
        .unwrap();

    let projector = Projector::new(
        Arc::clone(&doc_store),
        Arc::clone(&stores) as Arc<dyn ProjectionStores>,
    );
    projector.rebuild_kb("kb1").await.unwrap();

    let store = stores.store_for("kb1").await.unwrap();
    assert!(
        projector
            .reconcile_kb("kb1", false)
            .await
            .unwrap()
            .is_clean(),
        "precondition: a freshly projected KB must report no drift"
    );

    // One corruption per projected field. `Node` is mutated in place and written
    // straight back, so only the named field differs from CRDT truth.
    /// One named corruption of a single projected field.
    type Corruption = (&'static str, fn(&mut Node));

    let corruptions: Vec<Corruption> = vec![
        ("title", |n| n.title = "CORRUPTED".into()),
        ("body", |n| n.body = "CORRUPTED".into()),
        ("tags", |n| n.tags = vec!["CORRUPTED".into()]),
        ("kind", |n| n.kind = mae_kb::NodeKind::Task),
        ("todo_state", |n| n.todo_state = Some("DONE".into())),
        ("priority", |n| n.priority = Some('C')),
        ("aliases", |n| n.aliases = vec!["CORRUPTED".into()]),
        ("properties", |n| {
            n.properties.insert("role".into(), "CORRUPTED".into());
        }),
        ("source_version", |n| n.source_version = Some(999)),
    ];

    for (field, corrupt) in corruptions {
        let mut projected = store.get_node("n1").unwrap().unwrap();
        corrupt(&mut projected);
        store.insert_node(&projected).unwrap();

        let report = projector.reconcile_kb("kb1", false).await.unwrap();
        assert!(
            report.differing.contains(&"n1".to_string()),
            "drift in '{field}' went UNDETECTED — the reconciler reported {report:?}"
        );

        // Heal, and confirm the heal actually restored this field, so the test
        // also proves repair covers what detection covers.
        let healed = projector.reconcile_kb("kb1", true).await.unwrap();
        assert_eq!(
            healed.differing,
            vec!["n1".to_string()],
            "healing pass must report the same drift it repairs ('{field}')"
        );
        assert!(
            projector
                .reconcile_kb("kb1", false)
                .await
                .unwrap()
                .is_clean(),
            "after healing '{field}', the projection must match CRDT truth again"
        );
    }
}

/// #732 — the startup self-heal must address the collection doc by the KB's
/// **minted id**, not its display name.
///
/// `spawn_projector` collected `instance.name` and passed it as `kb_id`, while
/// `rebuild_kb` reads `kbc:{kb_id}` — the address every other daemon site
/// (dialer, checkpoint, kb_membership, and `scheduler.rs`'s own `collab_id`
/// lookup) derives from the minted id. So for any KB shared after ADR-105 D4
/// started minting uuids, startup read a document that does not exist.
///
/// Nothing caught it because nothing distinguished **"rebuilt 0 nodes"** from
/// **"never found the document"** — both surfaced as a quiet `debug!`. This test
/// pins that distinction: a rebuild keyed on a name that is not the doc's
/// address must be an ERROR, and the one keyed on the minted id must return the
/// real node count.
#[tokio::test]
async fn rebuild_is_keyed_on_the_minted_id_not_the_display_name() {
    let doc_store = mem_store();
    let stores = MemStores::new();

    // The realistic post-ADR-105 shape: display name and minted id differ.
    let minted_id = "6f1c9a2e-0d43-4b77-9a51-2c8e7b3f5a10";
    let display_name = "my-notes";

    let mut coll = mae_sync::kb::KbCollectionDoc::new(minted_id, "owner");
    for (id, title, body) in [("n1", "One", "first"), ("n2", "Two", "second")] {
        let n = mae_sync::kb::KbNodeDoc::new(id, title, body, &[]);
        doc_store
            .apply_update(
                &mae_sync::kb_node_doc_name(minted_id, id),
                &n.encode(),
                None,
            )
            .await
            .unwrap();
        coll.add_node(id, title);
    }
    doc_store
        .apply_update(&format!("kbc:{minted_id}"), &coll.encode_state(), None)
        .await
        .unwrap();

    let projector = Projector::new(
        Arc::clone(&doc_store),
        Arc::clone(&stores) as Arc<dyn ProjectionStores>,
    );

    // The bug: keyed on the display name, there is no such collection document.
    // This MUST fail loudly rather than quietly rebuild nothing.
    assert!(
        projector.rebuild_kb(display_name).await.is_err(),
        "a rebuild keyed on the display name must report that the collection doc \
         does not exist — reporting success with 0 nodes is what made #732 silent"
    );

    // Keyed on the minted id, the rebuild finds the real manifest.
    assert_eq!(
        projector.rebuild_kb(minted_id).await.unwrap(),
        2,
        "the minted id addresses the collection doc, so both nodes project"
    );

    // And the projection is genuinely populated — the count is not enough on its
    // own, since a rebuild that reported 2 while projecting nothing would be the
    // same class of silent failure.
    let store = stores.store_for(minted_id).await.unwrap();
    for id in ["n1", "n2"] {
        assert!(
            store.get_node(id).unwrap().is_some(),
            "node '{id}' must be present in the Cozo projection after rebuild"
        );
    }
}

// ---------------------------------------------------------------------------
// C6 — `heal` must not delete what the CRDT never knew about.
// ---------------------------------------------------------------------------

/// **The data-loss case.** A node in the projection with no `kbn:` document was
/// never CRDT-managed — which is how EVERY node in an un-shared KB looks, since
/// they persist with `crdt_doc = None` and appear in no manifest.
///
/// Healing used to delete all of them. That is not reconciliation, it is
/// emptying the user's KB the first time ADR-092 wires this call.
#[tokio::test]
async fn heal_keeps_a_node_that_was_never_crdt_managed() {
    let doc_store = mem_store();
    seed(&doc_store, "kb1", &[("synced", "Synced", "body")]).await;
    let stores = MemStores::new();
    let projector = Projector::new(Arc::clone(&doc_store), stores.clone());
    projector.reconcile_kb("kb1", true).await.unwrap();

    // A locally-authored node: in the store, in no manifest, with no node doc.
    let store = stores.store_for("kb1").await.unwrap();
    store
        .insert_node(&mae_kb::Node::new(
            "local-only",
            "Local only",
            mae_kb::NodeKind::Note,
            "never shared",
        ))
        .unwrap();

    let report = projector.reconcile_kb("kb1", true).await.unwrap();
    assert!(
        report.extra.contains(&"local-only".to_string()),
        "it is still REPORTED as extra -- the operator should see it"
    );
    assert!(
        store.get_node("local-only").unwrap().is_some(),
        "...but it must not be DELETED: no CRDT document means the collection \
         never knew about it, not that it was removed"
    );
}

/// The other half: a node that WAS in the collection and was removed still gets
/// healed away, so the fix did not simply disable reconciliation.
#[tokio::test]
async fn heal_still_deletes_a_node_that_was_genuinely_removed() {
    let doc_store = mem_store();
    seed(&doc_store, "kb1", &[("a", "A", "body"), ("b", "B", "body")]).await;
    let stores = MemStores::new();
    let projector = Projector::new(Arc::clone(&doc_store), stores.clone());
    projector.reconcile_kb("kb1", true).await.unwrap();
    let store = stores.store_for("kb1").await.unwrap();
    assert!(store.get_node("b").unwrap().is_some());

    // Remove `b` from the manifest, leaving its node doc behind (what a real
    // removal looks like).
    let (state, _) = doc_store.encode_state_and_sv("kbc:kb1").await.unwrap();
    let mut coll = mae_sync::kb::KbCollectionDoc::from_bytes(&state).unwrap();
    coll.remove_node("b");
    doc_store
        .apply_update("kbc:kb1", &coll.encode_state(), None)
        .await
        .unwrap();

    let report = projector.reconcile_kb("kb1", true).await.unwrap();
    assert_eq!(report.extra, vec!["b".to_string()]);
    assert!(
        store.get_node("b").unwrap().is_none(),
        "a genuinely removed node must still be healed away"
    );
    assert!(
        store.get_node("a").unwrap().is_some(),
        "and `a` must survive"
    );
}

/// **The C4 blast radius.** An empty manifest is far more likely to be a doc
/// that failed to load or was addressed by the wrong name than a KB whose every
/// node was deleted — that exact defect shipped once, silently.
///
/// Heal must refuse rather than interpret it as "delete everything".
#[tokio::test]
async fn heal_refuses_to_empty_a_projection_when_the_manifest_lists_nothing() {
    let doc_store = mem_store();
    seed(&doc_store, "kb1", &[("a", "A", "body"), ("b", "B", "body")]).await;
    let stores = MemStores::new();
    let projector = Projector::new(Arc::clone(&doc_store), stores.clone());
    projector.reconcile_kb("kb1", true).await.unwrap();
    let store = stores.store_for("kb1").await.unwrap();

    // Wipe the manifest's node list, as a mis-addressed or unloadable doc would.
    let mut empty = mae_sync::kb::KbCollectionDoc::new("kb1", "owner");
    let _ = empty.add_node("placeholder", "p");
    empty.remove_node("placeholder");
    doc_store
        .apply_update("kbc:kb1", &empty.encode_state(), None)
        .await
        .unwrap();

    projector.reconcile_kb("kb1", true).await.unwrap();
    assert!(
        store.get_node("a").unwrap().is_some() && store.get_node("b").unwrap().is_some(),
        "an empty manifest must never be read as an instruction to delete the KB"
    );
}
