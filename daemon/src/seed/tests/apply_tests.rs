//! Applying a plan: the KB it writes, read back through `kb/query`, and the
//! atomicity rules (resume after a crash, no-op re-run, different plan, never
//! overwrite). Every repository is synthetic and built here.

use std::sync::Arc;

use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, BODY_FORMAT_ORG_LITERAL_V1};
use serde_json::{json, Value};

use super::git_tests::Repo;
use crate::doc_editors::Attribution;
use crate::doc_store::DocStore;
use crate::kb_query::{self, KbQueryLimits};
use crate::seed::*;
use crate::storage::SqliteBackend;

pub(super) const KB: &str = "seeded";
/// The commit time of the fixture (2020-09-13T12:26:40Z).
pub(super) const T: i64 = 1_600_000_000;

const A: &str = "0a000000-0000-4000-8000-000000000001";
const B: &str = "0a000000-0000-4000-8000-000000000002";

/// Pages chosen to exercise every part of the contract: six drawer keys in no
/// sorted order (one mixed-case, one empty), a raw `:KIND:`, no title, an empty
/// `#+title:` and `#+filetags:`, unicode, no final newline, a headline id and
/// links inside the body, and a non-canonical header.
pub(super) fn fixture_files() -> Vec<(&'static str, String)> {
    vec![
        (
            "notes/alpha.org",
            format!(
                ":PROPERTIES:\n:ID: {A}\n:zeta: last\n:KIND: Concept\n:Alpha: first\n\
                 :mIxEd_Case: kept\n:beta:\n:END:\n#+title: Alpha\n#+filetags: :x:y:\n\
                 Intro with [[id:{B}][a link]].\n* Heading\n:PROPERTIES:\n\
                 :ID: 0a000000-0000-4000-8000-0000000000a1\n:END:\nText.\n"
            ),
        ),
        (
            "notes/no-title.org",
            format!(":PROPERTIES:\n:ID: {B}\n:END:\nPlain body.\n"),
        ),
        (
            "notes/empty-lines.org",
            ":PROPERTIES:\n:ID: 0a000000-0000-4000-8000-000000000003\n:END:\n#+title:\n\
             #+filetags:\nBody.\n"
                .to_string(),
        ),
        (
            "deep/dir/unicode.org",
            ":PROPERTIES:\n:ID: 0a000000-0000-4000-8000-000000000004\n:END:\n\
             #+title: Ñandú 😀 日本\nNo final newline: €"
                .to_string(),
        ),
        (
            "notes/noncanon.org",
            ":PROPERTIES:\n:ID:    0a000000-0000-4000-8000-000000000005\n:END:\n\
             #+TITLE:  Spaced\nBody.\n"
                .to_string(),
        ),
    ]
}

/// The fixture committed and tagged `v1` at [`T`].
pub(super) fn fixture_repo(files: &[(&str, String)]) -> Repo {
    let r = Repo::new();
    for (path, text) in files {
        r.write(path, text);
    }
    r.commit(T);
    r.git(&["tag", "v1"]);
    r
}

pub(super) fn memory_store() -> Arc<DocStore> {
    Arc::new(DocStore::new(
        Arc::new(SqliteBackend::open_memory().unwrap()),
        500,
    ))
}

pub(super) fn target<'a>(owner: &'a Identity, hash: &'a str) -> ApplyTarget<'a> {
    ApplyTarget {
        kb_id: KB,
        plan_hash: hash,
        owner,
        now: T + 10,
    }
}

/// Plan the repo, apply it, and return the store (with the owner as its
/// signer, as the serving daemon has), the owner and the plan.
pub(super) async fn seeded(r: &Repo) -> (Arc<DocStore>, Arc<Identity>, SeedPlan) {
    let plan = r.plan("v1");
    let hash = plan.plan_hash().expect("the fixture plans cleanly");
    let owner = Arc::new(Identity::generate("daemon"));
    let store = memory_store();
    let out = apply(&store, &plan, &target(&owner, &hash)).await.unwrap();
    assert!(matches!(out, ApplyOutcome::Applied { .. }), "{out:?}");
    store.set_signer(Arc::clone(&owner));
    (store, owner, plan)
}

pub(super) async fn query(
    store: &DocStore,
    owner: &Identity,
    method: &str,
    params: Value,
) -> Value {
    let fp = owner.fingerprint();
    kb_query::dispatch(method, &params, store, Some(&fp), KbQueryLimits::default())
        .await
        .unwrap_or_else(|e| panic!("{method}: {e:?}"))
}

pub(super) async fn get(store: &DocStore, owner: &Identity, id: &str) -> Value {
    query(
        store,
        owner,
        "kb/query.get",
        json!({"kb_id": KB, "node_id": id}),
    )
    .await
}

/// The WAL high-water of every document of the KB: equal before and after
/// means nothing was written.
async fn high_waters(store: &DocStore, plan: &SeedPlan) -> Vec<u64> {
    let mut names = vec![format!("kbc:{KB}")];
    names.extend(
        plan.nodes
            .iter()
            .map(|n| mae_sync::kb_node_doc_name(KB, &n.file.id)),
    );
    let mut out = Vec::new();
    for n in names {
        out.push(store.wal_high_water(&n).await.unwrap());
    }
    out
}

/// Every planned node reads back through `kb/query.get` with its typed fields,
/// its body byte for byte, and the git times. Kills: dropping the order, the
/// line flags, the raw kind, or the plan's `created_at`.
#[tokio::test]
async fn a_seeded_kb_reads_back_field_for_field() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, plan) = seeded(&r).await;
    for n in &plan.nodes {
        let g = get(&store, &owner, &n.file.id).await;
        let f = &n.file;
        let keys: Vec<&str> = f.properties.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(g["body"], json!(f.body), "{}", n.path);
        assert_eq!(g["title"], json!(f.title.clone().unwrap_or_default()));
        assert_eq!(g["tags"], json!(f.filetags.clone().unwrap_or_default()));
        assert_eq!(g["property_order"], json!(keys));
        assert_eq!(g["org_header"]["has_title_line"], json!(f.title.is_some()));
        assert_eq!(
            g["org_header"]["has_filetags_line"],
            json!(f.filetags.is_some())
        );
        for (k, v) in &f.properties {
            assert_eq!(g["properties"][k], json!(v), "{} {k}", n.path);
        }
        assert_eq!(g["created_at"], json!("2020-09-13T12:26:40Z"));
        assert_eq!(g["modified_at"], json!("2020-09-13T12:26:40Z"));
        assert_eq!(g["modified_at_basis"], json!("seed"));
    }
    let alpha = get(&store, &owner, A).await;
    assert_eq!(
        alpha["kind"],
        json!("Concept"),
        "the raw :KIND:, never folded"
    );
    assert_eq!(
        alpha["property_order"],
        json!(["ID", "zeta", "KIND", "Alpha", "mIxEd_Case", "beta"])
    );
}

/// `list` reports the same fields and times, and `search` finds body text.
#[tokio::test]
async fn list_and_search_see_the_seeded_kb() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, plan) = seeded(&r).await;
    let l = query(&store, &owner, "kb/query.list", json!({"kb_id": KB})).await;
    let nodes = l["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), plan.nodes.len());
    for n in nodes {
        assert_eq!(n["created_at"], json!("2020-09-13T12:26:40Z"));
        assert_eq!(n["modified_at"], json!("2020-09-13T12:26:40Z"));
        assert!(n["property_order"].is_array(), "{n}");
    }
    let s = query(
        &store,
        &owner,
        "kb/query.search",
        json!({"kb_id": KB, "query": "final newline"}),
    )
    .await;
    assert!(
        s.to_string()
            .contains("0a000000-0000-4000-8000-000000000004"),
        "{s}"
    );
}

/// An edit after the seed moves `modified_at` past the git time, and says so.
/// Kills: reporting the seed time regardless, or counting the seed's own write
/// as an edit (which would report the apply time before any edit).
#[tokio::test]
async fn an_edit_after_the_seed_moves_modified_at() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let name = mae_sync::kb_node_doc_name(KB, B);
    let mut doc = KbNodeDoc::from_bytes(&store.encode_state(&name).await.unwrap()).unwrap();
    let update = doc.set_body("Edited.\n");
    store
        .apply_update(&name, &update, None, Attribution::Principal("editor-1"))
        .await
        .unwrap();
    let g = get(&store, &owner, B).await;
    assert_eq!(g["modified_at_basis"], json!("edit"));
    assert!(
        g["modified_at"].as_str().unwrap() > "2020-09-13T12:26:40Z",
        "{g}"
    );
    assert_eq!(g["created_at"], json!("2020-09-13T12:26:40Z"));
}

/// The collection a reader (or a joining editor) receives is daemon-owned,
/// carries a signed genesis, and declares `org-literal-v1`, which is what MAE's
/// own writer refuses (`kb_ops::literal_guard`).
#[tokio::test]
async fn the_collection_is_daemon_owned_literal_and_signed() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, plan) = seeded(&r).await;
    let bytes = store.encode_state(&format!("kbc:{KB}")).await.unwrap();
    let coll = KbCollectionDoc::from_bytes(&bytes).unwrap();
    assert_eq!(
        coll.body_format().as_deref(),
        Some(BODY_FORMAT_ORG_LITERAL_V1)
    );
    assert_eq!(coll.owner(), owner.fingerprint());
    assert!(
        coll.oplog_head().is_some(),
        "the owner genesis is signed into the op-log"
    );
    assert_eq!(coll.list_nodes().len(), plan.nodes.len());
    let (record, rows) = store.seed_record(KB).await.unwrap().unwrap();
    assert_eq!(record.plan_hash, plan.plan_hash().unwrap());
    assert_eq!(record.commit, plan.source.commit);
    assert_eq!(record.contract, CONTRACT);
    assert_eq!(record.applied_at, Some(T + 10));
    assert!(rows.iter().all(|r| r.seed_wal_id.is_some()));
}

/// A re-run after success writes nothing.
#[tokio::test]
async fn a_rerun_after_success_is_a_no_op() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, plan) = seeded(&r).await;
    let before = high_waters(&store, &plan).await;
    let hash = plan.plan_hash().unwrap();
    let again = apply(&store, &r.plan("v1"), &target(&owner, &hash)).await;
    assert_eq!(again, Ok(ApplyOutcome::AlreadyApplied));
    assert_eq!(high_waters(&store, &plan).await, before);
}

/// A crash at every point, then a re-run with the same plan: the KB converges
/// to every node exactly once, and nothing is served before it is complete.
/// Kills: writing the collection first, re-creating a present node, or a
/// re-run refusing its own unfinished KB.
#[tokio::test]
async fn a_crash_at_any_point_then_a_rerun_converges() {
    let r = fixture_repo(&fixture_files());
    let plan = r.plan("v1");
    let hash = plan.plan_hash().unwrap();
    let n = plan.nodes.len();
    for k in 0..=n + 1 {
        let owner = Identity::generate("daemon");
        let store = memory_store();
        crate::seed::apply::crash::BEFORE_NODE.with(|c| c.set(Some(k)));
        let first = apply(&store, &plan, &target(&owner, &hash)).await;
        crate::seed::apply::crash::BEFORE_NODE.with(|c| c.set(None));
        assert!(first.is_err(), "crash point {k} must stop the apply");
        let visible = store.has_durable_doc(&format!("kbc:{KB}")).await;
        assert_eq!(
            visible,
            k > n,
            "the KB is visible only once every node exists (k={k})"
        );
        let second = apply(&store, &r.plan("v1"), &target(&owner, &hash))
            .await
            .unwrap();
        let expect = if k > n {
            ApplyOutcome::Applied {
                created: 0,
                present: n,
            }
        } else {
            ApplyOutcome::Applied {
                created: n - k,
                present: k,
            }
        };
        assert_eq!(second, expect, "k={k}");
        let l = query(&store, &owner, "kb/query.list", json!({"kb_id": KB})).await;
        assert_eq!(l["nodes"].as_array().unwrap().len(), n, "k={k}");
        for p in &plan.nodes {
            let row = store.seed_node(KB, &p.file.id).await.unwrap().unwrap();
            let stored = store
                .encode_state(&mae_sync::kb_node_doc_name(KB, &p.file.id))
                .await;
            let stored = KbNodeDoc::from_bytes(&stored.unwrap()).unwrap();
            assert_eq!(stored.materialize(), build_node(p, &row).materialize());
            assert!(row.seed_wal_id.is_some(), "k={k}: {}", p.path);
        }
    }
}

/// A different plan against an existing KB id is refused, whether the first
/// seed finished or was cut short, and writes nothing.
#[tokio::test]
async fn a_different_plan_is_refused_finished_or_not() {
    let r = fixture_repo(&fixture_files());
    let plan = r.plan("v1");
    let hash = plan.plan_hash().unwrap();
    r.write(
        "notes/new.org",
        ":PROPERTIES:\n:ID: 0a000000-0000-4000-8000-0000000000ff\n:END:\nx\n",
    );
    r.commit(T + 100);
    r.git(&["tag", "v2"]);
    let other = r.plan("v2");
    let other_hash = other.plan_hash().unwrap();
    for crash in [None, Some(2)] {
        let owner = Identity::generate("daemon");
        let store = memory_store();
        crate::seed::apply::crash::BEFORE_NODE.with(|c| c.set(crash));
        let _ = apply(&store, &plan, &target(&owner, &hash)).await;
        crate::seed::apply::crash::BEFORE_NODE.with(|c| c.set(None));
        let before = high_waters(&store, &other).await;
        let err = apply(&store, &other, &target(&owner, &other_hash))
            .await
            .unwrap_err();
        assert_eq!(
            err.kind,
            ApplyRefusalKind::DifferentPlan,
            "{crash:?}: {err}"
        );
        assert_eq!(high_waters(&store, &other).await, before);
    }
}

/// A node that exists with other content is never overwritten, and a fresh
/// seed that meets one writes nothing at all. Kills: comparing by id only.
#[tokio::test]
async fn an_existing_different_node_is_never_overwritten() {
    let r = fixture_repo(&fixture_files());
    let plan = r.plan("v1");
    let hash = plan.plan_hash().unwrap();
    let owner = Identity::generate("daemon");
    let store = memory_store();
    let name = mae_sync::kb_node_doc_name(KB, B);
    let squatter = KbNodeDoc::new(B, "Other", "someone's content\n", &[]);
    store
        .share_doc(&name, &squatter.encode(), Attribution::Principal("someone"))
        .await
        .unwrap();
    let err = apply(&store, &plan, &target(&owner, &hash))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ApplyRefusalKind::NodeDiffers, "{err}");
    let kept = KbNodeDoc::from_bytes(&store.encode_state(&name).await.unwrap()).unwrap();
    assert_eq!(kept.body(), "someone's content\n");
    assert_eq!(
        store.seed_record(KB).await.unwrap(),
        None,
        "refused before recording"
    );
    assert!(!store.has_durable_doc(&format!("kbc:{KB}")).await);
}

/// A KB id that exists without a seed record, a refused plan and a hash other
/// than the reviewed one are all refused before anything is written.
#[tokio::test]
async fn the_plan_and_the_id_are_checked_before_any_write() {
    let r = fixture_repo(&fixture_files());
    let plan = r.plan("v1");
    let hash = plan.plan_hash().unwrap();
    let owner = Identity::generate("daemon");
    let store = memory_store();
    let wrong = "0".repeat(64);
    let err = apply(&store, &plan, &target(&owner, &wrong))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ApplyRefusalKind::PlanHashMismatch);
    let mut refused = plan.clone();
    refused.refusals.push(Refusal {
        path: "x.org".into(),
        kind: ProblemKind::MissingId,
        detail: String::new(),
    });
    let err = apply(&store, &refused, &target(&owner, &hash))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ApplyRefusalKind::PlanRefused);
    assert_eq!(store.seed_record(KB).await.unwrap(), None);
    let other = KbCollectionDoc::new_owned(KB, "someone", "someone");
    store
        .share_doc(
            &format!("kbc:{KB}"),
            &other.encode_state(),
            Attribution::Principal("someone"),
        )
        .await
        .unwrap();
    let err = apply(&store, &plan, &target(&owner, &hash))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ApplyRefusalKind::KbExists);
    assert_eq!(store.seed_record(KB).await.unwrap(), None);
}

/// The collab write path on a seeded KB, through the real dispatch: the owner
/// (the daemon's identity) admits an editor, the editor's `kb/node_update` on
/// a seeded node lands and reads back, a stranger's is refused, and a viewer
/// may read but not write. Members cannot be added in production yet (ADR-098);
/// this pins that the KB a seed writes is one the existing authz serves.
#[tokio::test]
async fn an_authorized_member_can_write_a_seeded_node_and_a_stranger_cannot() {
    use crate::collab_handler::tests::{dispatch_as, kb_member_msg, load_coll, test_broadcaster};
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let bc = test_broadcaster();
    let mut docs = std::collections::HashSet::new();
    let owner_fp = owner.fingerprint();
    for (who, role) in [("SHA256:editor", "editor"), ("SHA256:viewer", "viewer")] {
        let add = kb_member_msg("kb/add_member", KB, who, Some(role));
        let resp = dispatch_as(&store, &bc, Some("daemon"), Some(&owner_fp), add, &mut docs).await;
        assert!(resp.error.is_none(), "add {who}: {:?}", resp.error);
    }
    let write_as = |who: &'static str| {
        let store = Arc::clone(&store);
        async move {
            let epoch = load_coll(&store, KB).await.epoch_of(who);
            let name = mae_sync::kb_node_doc_name(KB, B);
            let state = store.encode_state(&name).await.unwrap();
            let cid = mae_sync::kb::derive_kb_client_id(who, epoch);
            let mut doc = KbNodeDoc::from_bytes_with_client_id(&state, cid).unwrap();
            let update = doc.set_body(&format!("written by {who}\n"));
            json!({"jsonrpc": "2.0", "id": 1, "method": "kb/node_update", "params": {
                "kb_id": KB, "node_id": B,
                "update": mae_sync::encoding::update_to_base64(&update)}})
        }
    };
    for who in ["SHA256:stranger", "SHA256:viewer"] {
        let resp = dispatch_as(
            &store,
            &bc,
            Some("x"),
            Some(who),
            write_as(who).await,
            &mut docs,
        )
        .await;
        assert!(resp.error.is_some(), "{who} must not write");
    }
    let msg = write_as("SHA256:editor").await;
    let resp = dispatch_as(
        &store,
        &bc,
        Some("e"),
        Some("SHA256:editor"),
        msg,
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "editor write: {:?}", resp.error);
    let fp = "SHA256:viewer";
    let g = kb_query::dispatch(
        "kb/query.get",
        &json!({"kb_id": KB, "node_id": B}),
        &store,
        Some(fp),
        KbQueryLimits::default(),
    )
    .await
    .unwrap();
    assert_eq!(g["body"], json!("written by SHA256:editor\n"));
    assert_eq!(g["modified_at_basis"], json!("edit"));
    let stranger = kb_query::dispatch(
        "kb/query.get",
        &json!({"kb_id": KB, "node_id": B}),
        &store,
        Some("SHA256:stranger"),
        KbQueryLimits::default(),
    )
    .await;
    assert!(stranger.is_err(), "a non-member reads nothing");
}
