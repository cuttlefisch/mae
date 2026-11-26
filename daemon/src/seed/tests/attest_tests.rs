//! `kb seed --author` (ADR-113 A3): every seed write to a node is logged with
//! its WAL id and the attested author, or none, and an attested write signs an
//! `authorship_attested` event into the KB's review chain. The review's author
//! check (`review::acts::authors_since`) relies on exactly this: the log entry
//! at the write's WAL id, and as many entries as the editor record's seed row
//! counts updates.

use mae_mcp::identity::Identity;
use serde_json::{json, Value};

use super::apply_tests::{fixture_files, fixture_repo, memory_store, target, KB};
use super::update_tests::{v2, ALPHA, B, NEW};
use crate::doc_editors::{EditorKind, EditorsSince, InternalWriter};
use crate::doc_store::DocStore;
use crate::review::store::{seed_writes, LoggedSeedWrite};
use crate::seed::*;

async fn log_of(store: &DocStore, node: &str) -> Vec<LoggedSeedWrite> {
    let node = node.to_string();
    store
        .review_read(move |c| seed_writes(c, KB, &node))
        .await
        .unwrap()
}

async fn high_water(store: &DocStore, node: &str) -> u64 {
    let name = mae_sync::kb_node_doc_name(KB, node);
    store.wal_high_water(&name).await.unwrap()
}

/// The seed row's update count, read from the record origin on: a seed's
/// restore resets the origin, so the rows after it are what the review reads.
async fn seed_updates_after_origin(store: &DocStore, node: &str) -> Option<u64> {
    let name = mae_sync::kb_node_doc_name(KB, node);
    let origin = match store.editors_since_wal(&name, 0).await.unwrap() {
        EditorsSince::Incomplete {
            recorded_after_wal_id,
        } => recorded_after_wal_id,
        other => panic!("a seeded node's record starts at the seed: {other:?}"),
    };
    let EditorsSince::Known(rows) = store.editors_since_wal(&name, origin).await.unwrap() else {
        panic!("known from the origin on");
    };
    rows.iter()
        .find(|r| r.kind == EditorKind::Internal && r.editor == InternalWriter::Seed.as_str())
        .map(|r| r.updates)
}

/// The decoded events of the KB's review chain, verified under `owner`.
async fn events(store: &DocStore, owner: &Identity) -> Vec<Value> {
    let lines = crate::review::export_lines(store, KB).await.unwrap();
    let text: String = lines.iter().map(|l| l.to_string() + "\n").collect();
    crate::review::chain::verify_export(&text, &owner.public().to_bytes(), None).unwrap();
    lines
        .iter()
        .filter(|l| l["type"] == "event")
        .map(|l| {
            let b = mae_sync::encoding::base64_to_update(l["canonical"].as_str().unwrap()).unwrap();
            serde_json::from_slice(&b).unwrap()
        })
        .collect()
}

/// An attested first apply: one log entry per node at the WAL id of its
/// restore, naming the author, and one signed event per node. Kills: the log
/// not written, the author dropped, the event missing or unsigned.
#[tokio::test]
async fn an_attested_apply_logs_and_signs_every_write() {
    let r = fixture_repo(&fixture_files());
    let plan = r.plan("v1");
    let hash = plan.plan_hash().unwrap();
    let owner = Identity::generate("daemon");
    let author = Identity::generate("author").fingerprint();
    let store = memory_store();
    let mut t = target(&owner, &hash);
    t.author = Some(&author);
    t.operator = "op-one";
    apply(&store, &plan, &t).await.unwrap();
    store.set_signer(std::sync::Arc::new(Identity::from_seed(
        &owner.secret_bytes(),
        "daemon",
    )));
    for n in &plan.nodes {
        let wal = high_water(&store, &n.file.id).await;
        let want = vec![LoggedSeedWrite {
            wal_id: wal,
            author: Some(author.clone()),
        }];
        assert_eq!(log_of(&store, &n.file.id).await, want, "{}", n.file.id);
        assert_eq!(seed_updates_after_origin(&store, &n.file.id).await, None);
    }
    let ev = events(&store, &owner).await;
    assert_eq!(ev.len(), plan.nodes.len());
    for (e, n) in ev.iter().zip(&plan.nodes) {
        assert_eq!(e["kind"], json!("authorship_attested"));
        assert_eq!(e["actor"]["principal"], json!("operator:local"));
        assert_eq!(e["actor"]["os_user"], json!("op-one"));
        let wal = high_water(&store, &n.file.id).await;
        assert_eq!(
            e["payload"],
            json!({"node_id": n.file.id, "author": author,
                "commit": plan.source.commit, "wal_id": wal})
        );
    }
}

/// An update logs each node it writes, attested or not, and only those: an
/// unchanged node gains no entry. The log keeps pace with the editor record
/// (as many entries as the seed row counts updates), which is what lets the
/// review tell a write the log missed. Kills: an update's writes not logged
/// (updated or created), an unattested apply logging an author.
#[tokio::test]
async fn an_update_logs_the_writes_it_makes_and_only_those() {
    let r = fixture_repo(&fixture_files());
    let plan = r.plan("v1");
    let hash = plan.plan_hash().unwrap();
    let owner = Identity::generate("daemon");
    let store = memory_store();
    apply(&store, &plan, &target(&owner, &hash)).await.unwrap();
    for n in &plan.nodes {
        let log = log_of(&store, &n.file.id).await;
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].author, None, "no --author, no author");
    }
    let unchanged = "0a000000-0000-4000-8000-000000000005";
    let before = log_of(&store, unchanged).await;

    let rev = v2(&r);
    let plan2 = r.plan(&rev);
    let hash2 = plan2.plan_hash().unwrap();
    let author = Identity::generate("author").fingerprint();
    let mut t = target(&owner, &hash2);
    t.author = Some(&author);
    update(&store, &plan2, &t, None).await.unwrap();

    for node in [ALPHA, B] {
        let log = log_of(&store, node).await;
        assert_eq!(log.len(), 2, "{node}");
        assert_eq!(log[0].author, None);
        assert_eq!(
            log[1],
            LoggedSeedWrite {
                wal_id: high_water(&store, node).await,
                author: Some(author.clone())
            }
        );
        assert_eq!(seed_updates_after_origin(&store, node).await, Some(2));
    }
    let created = log_of(&store, NEW).await;
    assert_eq!(
        created,
        vec![LoggedSeedWrite {
            wal_id: high_water(&store, NEW).await,
            author: Some(author.clone())
        }]
    );
    assert_eq!(log_of(&store, unchanged).await, before);
}
