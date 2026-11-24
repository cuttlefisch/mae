//! `kb seed --update`: a seeded KB moved to a newer revision of its
//! repository. Each result is compared with a FRESH seed of the same revision
//! (the oracle), read back through the store and `kb/query`. Every repository
//! is synthetic and built here.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, MaterializedNode, Role};
use serde_json::json;

use super::apply_tests::{fixture_files, fixture_repo, memory_store, query, seeded, target, KB, T};
use super::git_tests::{note, Repo};
use super::page;
use crate::collab_handler::{operator_change_membership, OperatorMemberOp};
use crate::doc_editors::Attribution;
use crate::doc_store::DocStore;
use crate::seed::update::crash::{Step, AT};
use crate::seed::*;

pub(super) const ALPHA: &str = "0a000000-0000-4000-8000-000000000001";
const B: &str = "0a000000-0000-4000-8000-000000000002";
const EMPTY: &str = "0a000000-0000-4000-8000-000000000003";
const UNICODE: &str = "0a000000-0000-4000-8000-000000000004";
const NEW: &str = "0a000000-0000-4000-8000-0000000000f1";

/// Every node of the KB as a reader sees it: the manifest, each document.
pub(super) async fn snapshot(store: &DocStore) -> BTreeMap<String, MaterializedNode> {
    let coll = store.encode_state(&format!("kbc:{KB}")).await.unwrap();
    let coll = KbCollectionDoc::from_bytes(&coll).unwrap();
    let mut out = BTreeMap::new();
    for (id, _) in coll.list_nodes() {
        out.insert(id.clone(), node(store, &id).await);
    }
    out
}

pub(super) async fn node(store: &DocStore, id: &str) -> MaterializedNode {
    let name = mae_sync::kb_node_doc_name(KB, id);
    KbNodeDoc::from_bytes(&store.encode_state(&name).await.unwrap())
        .unwrap()
        .materialize()
}

/// The seed rows a reader and an export use: path, blob, digest and times.
pub(super) async fn rows(store: &DocStore) -> BTreeMap<String, (String, String, String, i64, i64)> {
    let (_, rows) = store.seed_record(KB).await.unwrap().unwrap();
    rows.into_iter()
        .map(|r| {
            let v = (
                r.path,
                r.blob,
                r.normalised_sha256,
                r.created_at,
                r.modified_at,
            );
            (r.node_id, v)
        })
        .collect()
}

/// A fresh seed of `rev` into a new store: the oracle.
pub(super) async fn fresh(r: &Repo, rev: &str) -> Arc<DocStore> {
    let plan = r.plan(rev);
    let hash = plan.plan_hash().unwrap();
    let store = memory_store();
    let owner = Identity::generate("oracle");
    apply(&store, &plan, &target(&owner, &hash)).await.unwrap();
    store
}

/// Commit what `edit` does to the working copy as `v{n + 1}` (the fixture is
/// `v1`), at `T + 100 n`.
pub(super) fn release(r: &Repo, n: i64, edit: impl FnOnce(&Repo)) -> String {
    edit(r);
    r.commit(T + 100 * n);
    let tag = format!("v{}", n + 1);
    r.git(&["tag", &tag]);
    tag
}

pub(super) fn remove(r: &Repo, path: &str) {
    std::fs::remove_file(r.path().join(path)).unwrap();
}

pub(super) async fn update_to(
    store: &DocStore,
    owner: &Identity,
    r: &Repo,
    rev: &str,
    discard: bool,
) -> Result<UpdateOutcome, ApplyError> {
    let plan = r.plan(rev);
    let hash = plan.plan_hash().expect("the revision plans cleanly");
    update(store, &plan, &target(owner, &hash), discard).await
}

/// Write `body` into node `id` as principal `who`, as a member's edit lands.
pub(super) async fn edit_body(store: &DocStore, id: &str, body: &str, who: &str) {
    let name = mae_sync::kb_node_doc_name(KB, id);
    let mut doc = KbNodeDoc::from_bytes(&store.encode_state(&name).await.unwrap()).unwrap();
    let update = doc.set_body(body);
    store
        .apply_update(&name, &update, None, Attribution::Principal(who))
        .await
        .unwrap();
}

/// The WAL high-water of the collection and every node document the store
/// holds for the KB: equal before and after means nothing was written.
async fn high_waters(store: &DocStore) -> Vec<(String, u64)> {
    let mut names = store.document_names().await;
    let nodes = mae_sync::kb_node_doc_name(KB, "");
    names.retain(|n| *n == format!("kbc:{KB}") || n.starts_with(&nodes));
    names.sort();
    let mut out = Vec::new();
    for n in names {
        let w = store.wal_high_water(&n).await.unwrap();
        out.push((n, w));
    }
    out
}

/// v2 of the fixture: one body edited, a page given a title and tags, one
/// page removed, one moved to another folder, one added.
fn v2(r: &Repo) -> String {
    release(r, 1, |r| {
        let alpha = fixture_files()[0].1.replace("Text.\n", "Text, revised.\n");
        r.write("notes/alpha.org", &alpha);
        let b = page(
            &[("ID", B)],
            Some("Now titled"),
            Some(&["t1"]),
            "Plain body.\n",
        );
        r.write("notes/no-title.org", &b);
        remove(r, "notes/empty-lines.org");
        let unicode = std::fs::read_to_string(r.path().join("deep/dir/unicode.org")).unwrap();
        remove(r, "deep/dir/unicode.org");
        r.write("moved/unicode.org", &unicode);
        r.write("notes/new.org", &note(NEW, "brand new\n"));
    })
}

/// An update reaches what a fresh seed of the new revision makes, node for
/// node and row for row; keeps the KB id, owner and members; serves the new
/// node set; keeps a removed node's document; and records the change in
/// history. Kills: replacing instead of assigning (history), a removed node
/// left in the manifest, rows not moved to the new plan.
#[tokio::test]
async fn an_update_reaches_a_fresh_seed_of_the_new_revision() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let reader = Identity::generate("reader");
    let add = OperatorMemberOp::Add {
        member: &reader.fingerprint(),
        role: Role::Viewer,
        label: "reader",
    };
    operator_change_membership(&store, &owner, KB, add)
        .await
        .unwrap();
    let rev = v2(&r);

    let d = update_diff(&store, &r.plan(&rev), KB, &owner.fingerprint())
        .await
        .unwrap();
    let ids = |v: &[NodeChange]| v.iter().map(|c| c.node_id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&d.added), [NEW]);
    assert_eq!(ids(&d.removed), [EMPTY]);
    assert_eq!(ids(&d.changed), [UNICODE, ALPHA, B], "{:?}", d.changed);
    assert_eq!(
        d.changed[0].from_path.as_deref(),
        Some("deep/dir/unicode.org")
    );
    assert_eq!(d.changed[1].fields, ["body"]);
    assert_eq!(d.changed[2].fields, ["title", "tags", "org_header"]);
    assert_eq!((d.unchanged, d.edited.len()), (1, 0));

    let out = update_to(&store, &owner, &r, &rev, false).await.unwrap();
    let expect = UpdateOutcome::Updated {
        created: 1,
        written: 2,
        present: 2,
        removed: 1,
    };
    assert_eq!(out, expect);
    let oracle = fresh(&r, &rev).await;
    assert_eq!(snapshot(&store).await, snapshot(&oracle).await);
    assert_eq!(rows(&store).await, rows(&oracle).await);
    let (record, _) = store.seed_record(KB).await.unwrap().unwrap();
    assert_eq!(record.plan_hash, r.plan(&rev).plan_hash().unwrap());
    assert_eq!(record.rev, rev);
    assert!(store.seed_update(KB).await.unwrap().is_none());

    let (got_owner, members) = crate::collab_handler::operator_list_members(&store, KB)
        .await
        .unwrap();
    assert_eq!(got_owner, owner.fingerprint());
    assert!(members.iter().any(|(m, _)| *m == reader.fingerprint()));
    let fp = reader.fingerprint();
    let read = |id: &'static str| {
        let store = Arc::clone(&store);
        let fp = fp.clone();
        async move {
            crate::kb_query::dispatch(
                "kb/query.get",
                &json!({"kb_id": KB, "node_id": id}),
                &store,
                Some(&fp),
                crate::kb_query::KbQueryLimits::default(),
            )
            .await
        }
    };
    let alpha = read(ALPHA).await.unwrap();
    assert!(alpha["body"].as_str().unwrap().contains("revised"));
    assert_eq!(alpha["modified_at"], json!("2020-09-13T12:28:20Z"));
    assert_eq!(alpha["modified_at_basis"], json!("seed"));
    assert!(read(EMPTY).await.is_err(), "a removed node is not served");
    let removed = mae_sync::kb_node_doc_name(KB, EMPTY);
    assert!(
        store.has_durable_doc(&removed).await,
        "its document is kept"
    );

    let versions = store.history_list(KB, ALPHA).await.unwrap();
    assert!(
        versions
            .iter()
            .any(|v| v.kind == mae_sync::kb::history::VersionKind::Seed),
        "the content an update replaces is kept: {versions:?}"
    );
    let gone = store.history_list(KB, EMPTY).await.unwrap();
    assert!(gone
        .iter()
        .any(|v| v.kind == mae_sync::kb::history::VersionKind::Delete));

    let before = high_waters(&store).await;
    let again = update_to(&store, &owner, &r, &rev, false).await;
    assert_eq!(again, Ok(UpdateOutcome::AlreadyApplied));
    assert_eq!(high_waters(&store).await, before);
}

/// An edit made after the apply refuses the update, naming the node, the
/// field and the editor, and writes nothing; with `--discard-edits` the node
/// takes the plan's content and the edit stays in its history. Kills: no
/// edit check, `--discard-edits` assumed, the edit lost from history.
#[tokio::test]
async fn an_edit_outside_the_seed_refuses_the_update_until_discarded() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    edit_body(&store, ALPHA, "Someone's own words.\n", "SHA256:editor").await;
    let rev = v2(&r);
    let d = update_diff(&store, &r.plan(&rev), KB, &owner.fingerprint())
        .await
        .unwrap();
    assert_eq!(d.edited.len(), 1, "{:?}", d.edited);
    assert_eq!(d.edited[0].node_id, ALPHA);
    assert_eq!(d.edited[0].fields.as_deref(), Some(&["body"][..]));
    assert_eq!(d.edited[0].editors, ["principal:SHA256:editor"]);

    let before = high_waters(&store).await;
    let err = update_to(&store, &owner, &r, &rev, false)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ApplyRefusalKind::EditedOutsideSeed, "{err}");
    assert!(err.detail.contains(ALPHA), "{err}");
    assert_eq!(high_waters(&store).await, before);
    assert!(store.seed_update(KB).await.unwrap().is_none());

    update_to(&store, &owner, &r, &rev, true).await.unwrap();
    let oracle = fresh(&r, &rev).await;
    assert_eq!(snapshot(&store).await, snapshot(&oracle).await);
    let kept = store.history_list(KB, ALPHA).await.unwrap();
    let bodies: Vec<String> = kept
        .iter()
        .filter_map(|v| v.content.as_ref())
        .map(|c| mae_sync::kb::history::decode_content(c).unwrap().body)
        .collect();
    assert!(
        bodies.iter().any(|b| b == "Someone's own words.\n"),
        "{bodies:?}"
    );
}

/// An edit to a node the new revision does not change is still an edit: the
/// comparison is with what the seed wrote, not with the new plan. Kills:
/// checking only the nodes the plan changes.
#[tokio::test]
async fn an_edit_to_a_node_the_revision_does_not_change_is_still_detected() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let untouched = "0a000000-0000-4000-8000-000000000005";
    edit_body(&store, untouched, "Edited here.\n", "SHA256:editor").await;
    let rev = release(&r, 1, |r| r.write("notes/new.org", &note(NEW, "x\n")));
    let err = update_to(&store, &owner, &r, &rev, false)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ApplyRefusalKind::EditedOutsideSeed);
    assert!(err.detail.contains(untouched), "{err}");
    update_to(&store, &owner, &r, &rev, true).await.unwrap();
    assert_eq!(node(&store, untouched).await.body, "Body.\n");
}

/// A page removed in one revision and restored in a later one under the same
/// `:ID:` is the same document: one history across all three revisions, and
/// the dry run says it returns. Kills: re-creating it (which would drop its
/// history) or refusing the existing document.
#[tokio::test]
async fn a_removed_node_that_returns_is_the_same_document_with_its_history() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let b_text = fixture_files()[1].1.clone();
    let b_name = mae_sync::kb_node_doc_name(KB, B);
    let first = KbNodeDoc::from_bytes(&store.encode_state(&b_name).await.unwrap()).unwrap();
    let v2 = release(&r, 1, |r| remove(r, "notes/no-title.org"));
    update_to(&store, &owner, &r, &v2, false).await.unwrap();
    assert!(!snapshot(&store).await.contains_key(B));
    let returned = b_text.replace("Plain body.", "Plain body, back.");
    let v3 = release(&r, 2, |r| r.write("elsewhere/b.org", &returned));
    let d = update_diff(&store, &r.plan(&v3), KB, &owner.fingerprint())
        .await
        .unwrap();
    assert_eq!(d.added.len(), 1);
    assert!(d.added[0].returns, "{:?}", d.added);
    assert_eq!(d.added[0].fields, ["body"]);
    update_to(&store, &owner, &r, &v3, false).await.unwrap();
    assert_eq!(
        snapshot(&store).await,
        snapshot(&*fresh(&r, &v3).await).await
    );
    let now = KbNodeDoc::from_bytes(&store.encode_state(&b_name).await.unwrap()).unwrap();
    assert!(
        !first.has_ops_beyond(&now.state_vector()).unwrap(),
        "the returned node continues the first document's lineage"
    );
    let kinds: Vec<_> = store
        .history_list(KB, B)
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.kind)
        .collect();
    use mae_sync::kb::history::VersionKind;
    assert_eq!(
        kinds,
        [VersionKind::Delete],
        "the v1 content, kept at removal"
    );
}

/// A crash at every step of an update, then a re-run with the same plan,
/// converges to exactly what an uninterrupted update makes. In between, each
/// node a reader sees is the old one or the new one, and the node set is the
/// old set or the new one. Kills: marking the record applied before the node
/// writes, a step that is not idempotent, the manifest changed piecemeal.
#[tokio::test]
async fn a_crash_at_every_step_then_a_rerun_converges() {
    let r = fixture_repo(&fixture_files());
    let old = snapshot(&seeded(&r).await.0).await;
    let rev = v2(&r);
    let oracle = fresh(&r, &rev).await;
    let new = snapshot(&oracle).await;
    let n = r.plan(&rev).nodes.len();
    let mut steps = vec![Step::Begun, Step::BeforeManifest, Step::BeforeFinish];
    steps.extend((0..n).flat_map(|i| [Step::BeforeNode(i), Step::AfterWrite(i)]));
    for step in steps {
        let (store, owner, _) = seeded(&r).await;
        AT.with(|c| c.set(Some(step)));
        let cut = update_to(&store, &owner, &r, &rev, false).await;
        AT.with(|c| c.set(None));
        if matches!(step, Step::AfterWrite(_)) && cut.is_ok() {
            continue; // that node needed no write, so the step never ran
        }
        assert!(cut.is_err(), "{step:?} must stop the update");
        let mid = snapshot(&store).await;
        let ids: BTreeSet<&String> = mid.keys().collect();
        let set_old: BTreeSet<&String> = old.keys().collect();
        let set_new: BTreeSet<&String> = new.keys().collect();
        assert!(
            ids == set_old || ids == set_new,
            "{step:?}: node set {ids:?}"
        );
        for (id, m) in &mid {
            let either = old.get(id) == Some(m) || new.get(id) == Some(m);
            assert!(either, "{step:?}: node {id} is neither old nor new");
        }
        let (record, _) = store.seed_record(KB).await.unwrap().unwrap();
        assert_ne!(record.rev, rev, "{step:?}: applied before it finished");
        let done = update_to(&store, &owner, &r, &rev, false).await;
        assert!(
            matches!(done, Ok(UpdateOutcome::Updated { .. })),
            "{step:?}: {done:?}"
        );
        assert_eq!(snapshot(&store).await, new, "{step:?}");
        assert_eq!(rows(&store).await, rows(&oracle).await, "{step:?}");
        assert!(store.seed_update(KB).await.unwrap().is_none());
    }
}

/// While an update is cut short: another plan is refused (by `--update` and
/// by a first apply), the export refuses, and a node written in the meantime
/// is caught on resume unless `--discard-edits`. Kills: a resume that does
/// not check again, an export that writes a half-moved KB.
#[tokio::test]
async fn a_cut_short_update_is_resumed_only_as_itself() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let rev = v2(&r);
    AT.with(|c| c.set(Some(Step::BeforeNode(0))));
    let _ = update_to(&store, &owner, &r, &rev, false).await;
    AT.with(|c| c.set(None));

    let v3 = release(&r, 2, |r| r.write("notes/more.org", &note("more", "m\n")));
    let other = update_to(&store, &owner, &r, &v3, false).await.unwrap_err();
    assert_eq!(other.kind, ApplyRefusalKind::DifferentPlan, "{other}");
    let plan = r.plan(&rev);
    let first = apply(&store, &plan, &target(&owner, &plan.plan_hash().unwrap())).await;
    assert_eq!(first.unwrap_err().kind, ApplyRefusalKind::DifferentPlan);
    let out = tempfile::tempdir().unwrap();
    assert!(export(&store, KB, out.path(), false).await.is_err());

    edit_body(&store, ALPHA, "Meanwhile.\n", "SHA256:editor").await;
    let err = update_to(&store, &owner, &r, &rev, false)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ApplyRefusalKind::EditedOutsideSeed, "{err}");
    assert!(err.detail.contains(ALPHA));
    update_to(&store, &owner, &r, &rev, true).await.unwrap();
    assert_eq!(
        snapshot(&store).await,
        snapshot(&*fresh(&r, &rev).await).await
    );
}

/// Every refusal writes nothing: a hash other than the reviewed one, a plan
/// with refusals, a KB no seed made, a seed whose first apply never finished,
/// and an end-to-end encrypted KB. Kills: skipping the hash check, updating a
/// KB that is not a seed's, writing into an E2E KB.
#[tokio::test]
async fn every_refusal_writes_nothing() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let rev = v2(&r);
    let plan = r.plan(&rev);
    let before = high_waters(&store).await;
    let refused = |e: Result<UpdateOutcome, ApplyError>| e.unwrap_err().kind;

    let wrong = "0".repeat(64);
    let k = refused(update(&store, &plan, &target(&owner, &wrong), true).await);
    assert_eq!(k, ApplyRefusalKind::PlanHashMismatch);
    let mut bad = plan.clone();
    bad.refusals.push(Refusal {
        path: "x.org".into(),
        kind: ProblemKind::MissingId,
        detail: String::new(),
    });
    let hash = plan.plan_hash().unwrap();
    let k = refused(update(&store, &bad, &target(&owner, &hash), true).await);
    assert_eq!(k, ApplyRefusalKind::PlanRefused);
    let mut other = target(&owner, &hash);
    other.kb_id = "never-seeded";
    let coll = KbCollectionDoc::new_owned("never-seeded", &owner.fingerprint(), "o");
    let by = Attribution::Principal("SHA256:o");
    store
        .share_doc("kbc:never-seeded", &coll.encode_state(), by)
        .await
        .unwrap();
    let k = refused(update(&store, &plan, &other, true).await);
    assert_eq!(k, ApplyRefusalKind::NotSeeded);
    assert_eq!(high_waters(&store).await, before);
    assert!(store.seed_update(KB).await.unwrap().is_none());

    let name = format!("kbc:{KB}");
    let mut coll = KbCollectionDoc::from_bytes(&store.encode_state(&name).await.unwrap()).unwrap();
    let e2e = coll.set_encryption(mae_sync::kb::Encryption::E2e);
    store.apply_update(&name, &e2e, None, by).await.unwrap();
    let before = high_waters(&store).await;
    let k = refused(update(&store, &plan, &target(&owner, &hash), true).await);
    assert_eq!(k, ApplyRefusalKind::Encrypted);
    assert_eq!(high_waters(&store).await, before);

    let unfinished = memory_store();
    let v1 = r.plan("v1");
    crate::seed::apply::crash::BEFORE_NODE.with(|c| c.set(Some(1)));
    let _ = apply(&unfinished, &v1, &target(&owner, &v1.plan_hash().unwrap())).await;
    crate::seed::apply::crash::BEFORE_NODE.with(|c| c.set(None));
    let k = refused(update(&unfinished, &plan, &target(&owner, &hash), true).await);
    assert_eq!(k, ApplyRefusalKind::SeedUnfinished);
}

/// A seed recorded before the version digest existed: the node the seed
/// wrote is rebuilt from the recorded blob, so an unedited node passes, an
/// edited one is named with its field, and a node whose blob cannot be read
/// cannot be vouched for. Kills: treating a missing digest as "not edited".
#[tokio::test]
async fn a_seed_recorded_before_version_digests_is_judged_from_its_blobs() {
    use crate::storage::SqliteBackend;
    let r = fixture_repo(&fixture_files());
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    let store = DocStore::new(backend.clone(), 500);
    let owner = Identity::generate("daemon");
    let v1 = r.plan("v1");
    apply(&store, &v1, &target(&owner, &v1.plan_hash().unwrap()))
        .await
        .unwrap();
    backend.execute_for_test("UPDATE kb_seed_nodes SET seed_version = NULL");
    edit_body(&store, B, "Edited.\n", "SHA256:editor").await;
    let rev = v2(&r);
    let plan = r.plan(&rev);
    let d = update_diff(&store, &plan, KB, &owner.fingerprint())
        .await
        .unwrap();
    let edited: Vec<_> = d
        .edited
        .iter()
        .map(|e| (e.node_id.as_str(), e.fields.clone()))
        .collect();
    assert_eq!(edited, [(B, Some(vec!["body"]))]);

    backend.execute_for_test(&format!(
        "UPDATE kb_seed_nodes SET blob = '{}' WHERE node_id = '{ALPHA}'",
        "1".repeat(40)
    ));
    let d = update_diff(&store, &plan, KB, &owner.fingerprint())
        .await
        .unwrap();
    let alpha = d
        .edited
        .iter()
        .find(|e| e.node_id == ALPHA)
        .expect("unknown counts");
    assert!(
        alpha.note.as_deref().unwrap().contains("cannot tell"),
        "{alpha:?}"
    );
    update_to(&store, &owner, &r, &rev, true).await.unwrap();
    let (_, after) = store.seed_record(KB).await.unwrap().unwrap();
    assert!(after.iter().all(|r| r.seed_version.is_some()));
}

/// A reader reading a node while an update writes it sees the old node or the
/// new one, never a mix (title of one, body of the other): one CRDT update
/// per node, applied under the document's lock. Pins that the update writes
/// each node in one update.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrent_reader_sees_each_node_old_or_new() {
    let r = fixture_repo(&fixture_files());
    let (store, owner, _) = seeded(&r).await;
    let old = node(&store, B).await;
    let rev = v2(&r);
    let new = snapshot(&*fresh(&r, &rev).await).await[B].clone();
    assert_ne!(old.title, new.title);
    assert_ne!(old.tags, new.tags);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let as_json = |m: &MaterializedNode| (json!(m.title), json!(m.tags), json!(m.body));
    let (was, will) = (as_json(&old), as_json(&new));
    let reader = {
        let (store, stop, owner) = (Arc::clone(&store), Arc::clone(&stop), Arc::clone(&owner));
        tokio::spawn(async move {
            let mut seen = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) || seen == 0 {
                let params = json!({"kb_id": KB, "node_id": B});
                let g = query(&store, &owner, "kb/query.get", params).await;
                let got = (g["title"].clone(), g["tags"].clone(), g["body"].clone());
                assert!(got == was || got == will, "torn read: {got:?}");
                seen += 1;
                tokio::task::yield_now().await;
            }
            seen
        })
    };
    update_to(&store, &owner, &r, &rev, false).await.unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(reader.await.unwrap() > 0);
    assert_eq!(node(&store, B).await, new);
}
