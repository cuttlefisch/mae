//! KB write contract, part 7: a node write is atomic.
//!
//! A node's row and its outgoing links are written by separate store scripts
//! with no enclosing transaction (`insert_node`: node row, then link
//! replacement). Two writers sharing one SQLite file — the editor and the
//! daemon do exactly this — can interleave them and leave a node whose stored
//! links come from a DIFFERENT version of its body than the one stored.
//!
//! Contract: after any number of concurrent or failed writes, every node's
//! stored links are exactly the links of its stored body.

use super::*;
use std::collections::BTreeSet;
use std::sync::Arc;

const ROUNDS: usize = 3000;

fn body_linking(target: &str, round: usize) -> String {
    format!("Round {round} links to [[id:{target}][{target}]].")
}

fn stored_targets(store: &CozoKbStore, id: &str) -> BTreeSet<String> {
    store
        .links_from(id)
        .unwrap()
        .into_iter()
        .map(|l| l.dst)
        .collect()
}

/// Body of version `v`: a typed link to `target`, an external URL (a link in
/// the text, never an edge), and a `#+TRANSCLUDE:` member.
fn versioned_body(target: &str, v: usize) -> String {
    format!(
        "#+TRANSCLUDE: {target} member\nVersion {v} links to [[id:{target}][{target}]] \
         and [[https://example.org/v{v}][a site]]."
    )
}

fn stored_members(store: &CozoKbStore, id: &str) -> Vec<String> {
    store
        .meta_members(id)
        .unwrap()
        .into_iter()
        .map(|m| m.member_id)
        .collect()
}

/// Deterministic proof through `insert_node` itself: make the write fail
/// AFTER its node-row step (test seam) and check what is stored. While the
/// row and the derived rows are separate writes, the new body is stored with
/// the OLD links and members: RED. Once a node write is one transaction, the
/// failure rolls the row back too and the node is exactly as before: GREEN.
///
/// (No second writer is needed: concurrent interleaving and a failure part
/// way are the same property — the row and what is derived from it commit
/// together or not at all. The real two-writer race is the `#[ignore]`
/// stress test below.)
#[test]
fn proof_a_failed_node_write_leaves_no_partial_state() {
    let tmp = tempfile::tempdir().unwrap();
    let store = CozoKbStore::open_with_engine(tmp.path().join("s.sqlite"), "sqlite").unwrap();
    for t in ["t:alpha", "t:beta"] {
        store
            .insert_node(&Node::new(t, t, NodeKind::Note, "target"))
            .unwrap();
    }
    store
        .insert_node(&Node::new(
            "n:x",
            "X",
            NodeKind::Note,
            versioned_body("t:alpha", 0),
        ))
        .unwrap();
    assert_eq!(
        stored_targets(&store, "n:x"),
        BTreeSet::from(["t:alpha".to_string()]),
        "fixture: the first write stored its link, and no edge for the external URL"
    );
    assert_eq!(
        stored_members(&store, "n:x"),
        vec!["t:alpha".to_string()],
        "fixture: the first write stored its transclusion member"
    );

    crate::cozo_store::test_hooks::set_fail_after_node_row(true);
    let failed = store.insert_node(&Node::new(
        "n:x",
        "X",
        NodeKind::Note,
        versioned_body("t:beta", 1),
    ));
    crate::cozo_store::test_hooks::set_fail_after_node_row(false);
    assert!(
        failed.is_err(),
        "fixture: the injected failure must surface"
    );

    let body = store.get_node("n:x").unwrap().expect("node").body;
    assert!(
        body.contains("Version 0"),
        "a failed write replaced the stored body: {body:?}"
    );
    assert_eq!(
        stored_targets(&store, "n:x"),
        BTreeSet::from(["t:alpha".to_string()]),
        "a failed write changed the stored links"
    );
    assert_eq!(
        stored_members(&store, "n:x"),
        vec!["t:alpha".to_string()],
        "a failed write changed the stored transclusion members"
    );
}

/// A delete removes the node, its links and its members together.
#[test]
fn delete_removes_the_node_and_everything_derived_from_it() {
    let tmp = tempfile::tempdir().unwrap();
    let store = CozoKbStore::open_with_engine(tmp.path().join("s.sqlite"), "sqlite").unwrap();
    store
        .insert_node(&Node::new("t:alpha", "A", NodeKind::Note, "target"))
        .unwrap();
    store
        .insert_node(&Node::new(
            "n:x",
            "X",
            NodeKind::Note,
            versioned_body("t:alpha", 0),
        ))
        .unwrap();
    assert!(
        !stored_members(&store, "n:x").is_empty(),
        "fixture: members stored"
    );
    store.delete_node("n:x").unwrap();
    assert!(store.get_node("n:x").unwrap().is_none(), "node left behind");
    assert!(
        stored_targets(&store, "n:x").is_empty(),
        "links left behind"
    );
    assert!(
        stored_members(&store, "n:x").is_empty(),
        "members left behind"
    );
}

/// The real race, two handles on one SQLite file (as editor + daemon).
/// Measured on 0.14.128: ~1 inconsistent round per 3,000 — too rare to be a
/// reliable default-CI test, so it is opt-in. A fix must pass it repeatedly:
/// `cargo test -p mae-kb --lib atomicity -- --ignored` (run it 10 times).
#[test]
#[ignore = "probabilistic stress test; see doc comment"]
fn proof_concurrent_writers_never_leave_links_from_another_version_of_the_body() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("shared.sqlite");
    let a = Arc::new(CozoKbStore::open_with_engine(&path, "sqlite").unwrap());
    let b = Arc::new(CozoKbStore::open_with_engine(&path, "sqlite").unwrap());
    for t in ["t:alpha", "t:beta"] {
        a.insert_node(&Node::new(t, t, NodeKind::Note, "target"))
            .unwrap();
    }

    let mut mismatches = Vec::new();
    let mut writes_landed = 0usize;
    for round in 0..ROUNDS {
        let (sa, sb) = (Arc::clone(&a), Arc::clone(&b));
        let wa = std::thread::spawn(move || {
            sa.insert_node(&Node::new(
                "n:x",
                "X",
                NodeKind::Note,
                body_linking("t:alpha", round),
            ))
        });
        let wb = std::thread::spawn(move || {
            sb.insert_node(&Node::new(
                "n:x",
                "X",
                NodeKind::Note,
                body_linking("t:beta", round),
            ))
        });
        // A writer may fail with a busy error; that is fine. Silent
        // inconsistency is not.
        let (ra, rb) = (wa.join().unwrap(), wb.join().unwrap());
        writes_landed += usize::from(ra.is_ok()) + usize::from(rb.is_ok());

        let body = a.get_node("n:x").unwrap().expect("node").body;
        let expected: BTreeSet<String> = ["t:alpha", "t:beta"]
            .into_iter()
            .filter(|t| body.contains(&format!("[[id:{t}]")))
            .map(String::from)
            .collect();
        let actual = stored_targets(&a, "n:x");
        if actual != expected {
            mismatches.push(format!(
                "round {round}: body links {expected:?}, stored links {actual:?}"
            ));
        }
    }
    // Not vacuous: if every write failed, the body never changed and every
    // round would trivially "match".
    assert!(
        writes_landed > ROUNDS,
        "only {writes_landed} of {} writes landed; the race was not exercised",
        2 * ROUNDS
    );
    assert!(
        mismatches.is_empty(),
        "{} of {ROUNDS} rounds left links from another version of the body, e.g. {:?}",
        mismatches.len(),
        mismatches.first()
    );
}
