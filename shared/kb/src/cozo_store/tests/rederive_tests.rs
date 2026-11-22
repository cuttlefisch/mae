//! Stored edges follow the edge derivation (#941 PR2, risks R2/R3): a store
//! whose `links`/`meta_members` rows were made by the old `crate::org`
//! derivation is re-derived from its stored bodies on the next open, without
//! touching a node row, once.

use super::rederive::DERIVATION_VERSION_KEY;
use super::*;
use crate::cozo_store::test_hooks::set_fail_rederive;

/// A body whose old and new derivations differ in every way #941 lists.
const A_BODY: &str = "See [[help:x][help]], [[#custom]], [[file:notes.org][notes]] and \
                      [[id:b?rel=answers&w=0.5][b]].\n# [[c]] in a comment\n";

/// The pre-PR2 `CozoKbStore::derived_links`, reproduced from the parser it
/// called, so the stale rows are the ones a shipped store really holds.
fn old_derived(id: &str, body: &str) -> Vec<(String, String, f64, f64)> {
    let mut out: Vec<(String, String, f64, f64)> = Vec::new();
    for l in crate::org::parse_typed_links(body, id) {
        if crate::org::is_external_link_target(&l.target) {
            continue;
        }
        match out
            .iter_mut()
            .find(|(d, r, _, _)| *d == l.target && *r == l.rel_type)
        {
            Some(e) => *e = (l.target, l.rel_type, l.weight, l.confidence),
            None => out.push((l.target, l.rel_type, l.weight, l.confidence)),
        }
    }
    out
}

fn link_rows(store: &CozoKbStore) -> Vec<Vec<DataValue>> {
    store
        .run_immut(
            "?[src, dst, rel_type, weight, confidence, created_at] := \
             *links{src, dst, rel_type, weight, confidence, created_at}",
        )
        .unwrap()
        .rows
}

fn node_rows(store: &CozoKbStore) -> Vec<Vec<DataValue>> {
    store
        .run_immut("?[id, title, body, updated_at] := *nodes{id, title, body, updated_at}")
        .unwrap()
        .rows
}

fn edges_of(store: &CozoKbStore, id: &str) -> Vec<(String, String, f64, f64)> {
    let mut v: Vec<_> = store
        .links_from(id)
        .unwrap()
        .into_iter()
        .map(|l| (l.dst, l.rel_type, l.weight, l.confidence))
        .collect();
    v.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    v
}

fn members_of(store: &CozoKbStore, id: &str) -> Vec<(String, String)> {
    store
        .meta_members(id)
        .unwrap()
        .into_iter()
        .map(|m| (m.member_id, m.role))
        .collect()
}

/// A store as the pre-PR2 binary left it: nodes written, their rows made by
/// the old derivation, a stale meta member, no derivation stamp.
fn write_stale_store(path: &std::path::Path) {
    let store = CozoKbStore::open(path).unwrap();
    for n in [
        Node::new("a", "A", NodeKind::Note, A_BODY),
        Node::new("b", "B", NodeKind::Note, ""),
        Node::new("m", "M", NodeKind::Note, "#+TRANSCLUDE: b reference\n"),
    ] {
        store.insert_node(&n).unwrap();
    }
    let old = old_derived("a", A_BODY);
    // The precondition that makes this test mean anything: the old
    // derivation stored edges the new one does not, and missed the typed one.
    let targets: Vec<&str> = old.iter().map(|(d, _, _, _)| d.as_str()).collect();
    assert!(
        targets.contains(&"help:x") && targets.contains(&"c"),
        "old derivation: {old:?}"
    );
    store.replace_node_links("a", &old).unwrap();
    store.add_meta_member("m", "stale", 7, "content").unwrap();
    store
        .record_source_file("/kb/a.org", "hash-a", 42, &["a".to_string()])
        .unwrap();
    store
        .run_mut_params(
            "?[key] <- [[$k]] :rm instance_meta {key}",
            BTreeMap::from([("k".to_string(), DataValue::from(DERIVATION_VERSION_KEY))]),
        )
        .unwrap();
}

fn stamp(store: &CozoKbStore) -> Option<String> {
    store.get_meta(DERIVATION_VERSION_KEY).unwrap()
}

#[test]
fn stale_edges_are_rederived_on_open_and_nothing_else_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("kb");
    write_stale_store(&path);
    let (nodes_before, files_before) = {
        // Read the stale store's node rows and source files through a store
        // that has NOT re-derived: the failure hook keeps open read-only here.
        set_fail_rederive(true);
        let store = CozoKbStore::open(&path).unwrap();
        set_fail_rederive(false);
        (node_rows(&store), store.list_source_files().unwrap())
    };

    let store = CozoKbStore::open(&path).unwrap();
    assert_eq!(
        edges_of(&store, "a"),
        vec![("b".to_string(), "answers".to_string(), 0.5, 1.0)],
        "help:, file:, a local [[#custom]] and a link in a comment are not edges; \
         the typed link keeps its relation and weight"
    );
    assert_eq!(
        members_of(&store, "m"),
        vec![("b".into(), "reference".into())]
    );
    assert_eq!(
        node_rows(&store),
        nodes_before,
        "node rows are never rewritten"
    );
    assert_eq!(store.list_source_files().unwrap(), files_before);
    assert_eq!(stamp(&store).as_deref(), Some(DERIVATION_VERSION));

    // Re-open: nothing to do, and nothing moves (a kept row keeps its
    // created_at).
    let links = link_rows(&store);
    drop(store);
    let store = CozoKbStore::open(&path).unwrap();
    assert_eq!(link_rows(&store), links);
    assert!(store.rederive_all_links().unwrap().is_noop());
}

#[test]
fn a_failed_rederive_does_not_block_open_and_retries_on_the_next() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("kb");
    write_stale_store(&path);

    set_fail_rederive(true);
    let opened = CozoKbStore::open(&path);
    set_fail_rederive(false);
    let store = opened.expect("a failed re-derive must not block open");
    assert!(
        edges_of(&store, "a")
            .iter()
            .any(|(d, _, _, _)| d == "help:x"),
        "nothing was re-derived"
    );
    assert_eq!(stamp(&store), None, "the stamp waits for a successful pass");
    drop(store);

    let store = CozoKbStore::open(&path).unwrap();
    assert_eq!(edges_of(&store, "a").len(), 1);
    assert_eq!(stamp(&store).as_deref(), Some(DERIVATION_VERSION));
}

#[test]
fn rederive_reaches_every_node_across_batches() {
    let store = CozoKbStore::open_mem().unwrap();
    let n = 600; // more than two batches
    for i in 0..n {
        let id = format!("n{i:04}");
        store
            .insert_node(&Node::new(&id, &id, NodeKind::Note, "[[target]]"))
            .unwrap();
        store
            .add_typed_link_with_confidence(&id, "help:stale", "references", 1.0, 1.0)
            .unwrap();
    }
    let report = store.rederive_all_links().unwrap();
    assert_eq!(report.nodes, n);
    assert_eq!(report.links_removed, n);
    assert_eq!(report.links_written, 0, "the kept edge is not rewritten");
    let stale: Vec<_> = link_rows(&store)
        .into_iter()
        .filter(|r| r[1].get_str() == Some("help:stale"))
        .collect();
    assert!(stale.is_empty(), "{} stale rows left", stale.len());
    assert!(store.rederive_all_links().unwrap().is_noop());
}
