//! ADR-092 D2 — character-level CRDT convergence for a KB node's text fields.
//!
//! Split out of `node_tests.rs` to stay under the structural ceiling. These are the
//! adversarial cases for `set_body`/`set_title`/`set_tags`: the oracle is never mere
//! peer-equality (a CRDT gives that for free) but what happened to the text nobody
//! edited.

use super::*;

/// ADR-092 D2. The oracle here is deliberately NOT peer-equality: a CRDT gives
/// convergence for free, so `a.body() == b.body()` passes even when the merged
/// text is garbage. It is exactly that assertion — the only one
/// `two_clients_merge_body` above makes — which let this ship.
///
/// The meaningful question is what happened to the text neither peer touched.
/// Under a wholesale `remove_range(0,len)` + `insert(0,new)`, both peers tombstone
/// the shared base once and both re-insert their own full copy at origin 0, so the
/// untouched base survives TWICE. Two people editing a 500-line node concurrently
/// get a 1000-line node with everything doubled.
#[test]
fn concurrent_same_field_edits_do_not_duplicate_the_untouched_base() {
    let base = "Line one.\nLine two.\n";
    let mut a = KbNodeDoc::new_with_client_id("n1", "T", base, &[], 1);
    let mut b = KbNodeDoc::from_bytes_with_client_id(&a.encode(), 2).unwrap();

    // Each appends its own line; neither touches the shared base.
    let ua = a.set_body("Line one.\nLine two.\nFrom A.\n");
    let ub = b.set_body("Line one.\nLine two.\nFrom B.\n");

    a.apply_update(&ub).unwrap();
    b.apply_update(&ua).unwrap();

    let merged = a.body();
    assert_eq!(
        merged,
        b.body(),
        "peers converge (necessary, not sufficient)"
    );
    assert!(merged.contains("From A."), "A's edit survives: {merged:?}");
    assert!(merged.contains("From B."), "B's edit survives: {merged:?}");
    assert_eq!(
        merged.matches("Line one.").count(),
        1,
        "the base neither peer edited must appear exactly ONCE, not once per \
         peer — got {merged:?}"
    );
}

/// The ≥3-peer bar (principle #14), and order-independence on top of it: the
/// merged text must be identical no matter which order a peer receives the other
/// two updates in, and the untouched base must still appear once.
#[test]
fn three_peers_editing_one_body_converge_identically_under_every_apply_order() {
    let base = "shared prefix\n";
    let edits = ["alpha", "beta", "gamma"];

    // All six orderings of applying the two remote updates at each peer.
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let mut results = Vec::new();

    for order in orders {
        let seed = KbNodeDoc::new_with_client_id("n1", "T", base, &[], 1);
        let state = seed.encode();

        // Three independent peers, distinct client ids, all from one lineage.
        let mut peers: Vec<KbNodeDoc> = (0..3)
            .map(|i| KbNodeDoc::from_bytes_with_client_id(&state, (i + 1) as u64).unwrap())
            .collect();
        let updates: Vec<Vec<u8>> = (0..3)
            .map(|i| peers[i].set_body(&format!("{base}{}\n", edits[i])))
            .collect();

        for (i, peer) in peers.iter_mut().enumerate() {
            for &j in order.iter().filter(|&&j| j != i) {
                peer.apply_update(&updates[j]).unwrap();
            }
        }

        let merged = peers[0].body();
        for (i, peer) in peers.iter().enumerate() {
            assert_eq!(
                peer.body(),
                merged,
                "peer {i} diverged under order {order:?}"
            );
        }
        for e in edits {
            assert!(
                merged.contains(e),
                "'{e}' lost under order {order:?}: {merged:?}"
            );
        }
        assert_eq!(
            merged.matches("shared prefix").count(),
            1,
            "the untouched base must appear once under order {order:?}, got {merged:?}"
        );
        results.push(merged);
    }

    assert!(
        results.windows(2).all(|w| w[0] == w[1]),
        "apply order changed the merged text: {results:?}"
    );
}

/// The same hazard on `title` — a shorter field, but the same wholesale replace.
#[test]
fn concurrent_title_edits_do_not_duplicate_the_untouched_base() {
    let mut a = KbNodeDoc::new_with_client_id("n1", "Design Notes", "b", &[], 1);
    let mut b = KbNodeDoc::from_bytes_with_client_id(&a.encode(), 2).unwrap();

    let ua = a.set_title("Design Notes (draft)");
    let ub = b.set_title("Design Notes v2");

    a.apply_update(&ub).unwrap();
    b.apply_update(&ua).unwrap();

    let merged = a.title();
    assert_eq!(merged, b.title(), "peers converge");
    assert_eq!(
        merged.matches("Design Notes").count(),
        1,
        "the common prefix must not be duplicated: {merged:?}"
    );
}

/// Non-ASCII is where a UTF-16-offset diff goes wrong if the units are confused:
/// yrs is configured `OffsetKind::Utf16`, so an emoji counts as 2 and a CJK char
/// as 1. A byte- or char-offset diff would corrupt or panic here.
#[test]
fn reconciled_body_handles_utf16_surrogate_and_cjk_boundaries() {
    let base = "日本語 café 🎉 done\n";
    let mut a = KbNodeDoc::new_with_client_id("n1", "T", base, &[], 1);
    let mut b = KbNodeDoc::from_bytes_with_client_id(&a.encode(), 2).unwrap();

    // A edits after the emoji, B before it — the surrogate pair sits between them.
    let ua = a.set_body("日本語 café 🎉 done — A\n");
    let ub = b.set_body("日本語 café 🎉 done\nB adds a line\n");

    a.apply_update(&ub).unwrap();
    b.apply_update(&ua).unwrap();

    let merged = a.body();
    assert_eq!(merged, b.body(), "peers converge across a surrogate pair");
    assert_eq!(
        merged.matches("🎉").count(),
        1,
        "the emoji must survive exactly once: {merged:?}"
    );
    assert_eq!(
        merged.matches("日本語").count(),
        1,
        "the CJK prefix must not be duplicated: {merged:?}"
    );
}

/// `set_tags` is a `YArray`, but it carries the identical clear-and-refill hazard:
/// two peers each adding one tag both wipe the whole array and re-append their own
/// full list, so every tag they had in common comes back once per peer.
#[test]
fn concurrent_tag_edits_do_not_duplicate_shared_tags() {
    let shared = vec!["rust".to_string(), "kb".to_string()];
    let mut a = KbNodeDoc::new_with_client_id("n1", "T", "b", &shared, 1);
    let mut b = KbNodeDoc::from_bytes_with_client_id(&a.encode(), 2).unwrap();

    let mut a_tags = shared.clone();
    a_tags.push("from-a".to_string());
    let mut b_tags = shared.clone();
    b_tags.push("from-b".to_string());

    let ua = a.set_tags(&a_tags);
    let ub = b.set_tags(&b_tags);

    a.apply_update(&ub).unwrap();
    b.apply_update(&ua).unwrap();

    let merged = a.tags();
    assert_eq!(merged, b.tags(), "peers converge");
    assert_eq!(
        merged.iter().filter(|t| *t == "rust").count(),
        1,
        "a tag both peers kept must appear once, not once per peer: {merged:?}"
    );
    assert_eq!(
        merged.iter().filter(|t| *t == "kb").count(),
        1,
        "second shared tag must not be duplicated either: {merged:?}"
    );
}

/// A save that changes nothing must produce no ops at all. Otherwise every `:w`
/// on an unedited node churns tombstones into a document that is replicated and
/// compacted (ADR-032).
#[test]
fn setting_a_field_to_its_current_value_produces_no_ops() {
    let mut node = KbNodeDoc::new_with_client_id("n1", "Title", "body text", &[], 1);
    let before = node.encode();

    let body_update = node.set_body("body text");
    let title_update = node.set_title("Title");

    assert_eq!(node.body(), "body text");
    assert_eq!(node.title(), "Title");
    assert_eq!(
        node.encode().len(),
        before.len(),
        "an unchanged field must not grow the document"
    );
    for (field, update) in [("body", &body_update), ("title", &title_update)] {
        let mut peer = KbNodeDoc::from_bytes(&before).unwrap();
        let len_before = peer.encode().len();
        peer.apply_update(update).unwrap();
        assert_eq!(
            peer.encode().len(),
            len_before,
            "the no-op {field} update carried real ops"
        );
    }
}
