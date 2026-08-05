//! ADR-092 D2 — character-level CRDT convergence for a KB node's text fields,
//! and ADR-093 Gate A — schema v2 carrying every field.
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

/// An edited body must be stored as a CONTIGUOUS byte run, not one yrs item per
/// character.
///
/// `scripts/collab-p2p-mesh-e2e.sh` greps the daemon store for canary plaintext —
/// once to prove content reached the owner (non-vacuity), and once to prove an
/// E2E KB's plaintext is ABSENT (key-blindness). A per-character diff fragments
/// the text so neither grep can find it, which breaks the first check and, far
/// worse, makes the second pass whether or not anything was actually sealed.
///
/// The replacing case is the one that regressed: inserting into an EMPTY text
/// always produced a single run, so a test that only covered creation missed it.
#[test]
fn an_edited_body_is_stored_as_a_contiguous_run() {
    let canary = "MESH-CANARY-abc123";
    for (label, initial) in [
        ("empty", ""),
        ("replacing an existing body", "old body here"),
    ] {
        let mut node = KbNodeDoc::new_with_client_id("n1", "T", initial, &[], 1);
        let _ = node.set_body(canary);
        let bytes = node.encode();
        assert!(
            bytes.windows(canary.len()).any(|w| w == canary.as_bytes()),
            "body set over {label} must appear as one contiguous run in the \
             encoded document, else a grep-based store assertion silently \
             stops meaning anything"
        );
    }
}

// ---------------------------------------------------------------------------
// ADR-093 Gate A — schema v2
// ---------------------------------------------------------------------------

/// Gate A.2 — a v1 document opens under v2 with no loss and, crucially, **no
/// spurious ops**.
///
/// The "no ops" half is the one that matters. If merely reading a v1 document
/// caused it to be upgraded in place, every reader would author migration ops,
/// which is the concurrent-migration hazard Gate A.3 covers. Asserting the
/// encoded length is unchanged proves reading is genuinely read-only.
#[test]
fn a_v1_document_opens_under_v2_without_loss_or_spurious_ops() {
    // A v1 doc: text fields only, no schema_v key.
    let v1 = KbNodeDoc::new("n1", "Title", "body", &["a".to_string()]);
    let bytes = v1.encode();

    let reopened = KbNodeDoc::from_bytes(&bytes).unwrap();
    assert_eq!(reopened.schema_version(), 1, "no schema_v key ⇒ v1");

    // Every v2 accessor tolerates the absent key.
    assert_eq!(reopened.kind(), None);
    assert_eq!(reopened.todo_state(), None);
    assert_eq!(reopened.priority(), None);
    assert_eq!(reopened.source_version(), None);
    assert!(reopened.aliases().is_empty());
    assert!(reopened.properties().is_empty());

    // The text fields are untouched.
    assert_eq!(reopened.title(), "Title");
    assert_eq!(reopened.body(), "body");
    assert_eq!(reopened.tags(), vec!["a"]);

    assert_eq!(
        reopened.encode().len(),
        bytes.len(),
        "reading a v1 doc must not author a single op"
    );
}

/// Gate A.3 — the Automerge concurrent-migration hazard, made falsifiable.
///
/// Automerge's own documentation is explicit that this is what makes schema
/// migration harder in a CRDT than in a centralized database: *"it could happen
/// that two users independently perform the same migration… you need to ensure
/// that the two migrations don't clash with each other, which is difficult."*
///
/// MAE avoids it by construction rather than by mitigation — there is no
/// upcast-on-read, so opening a v1 doc authors nothing. But two peers can still
/// legitimately WRITE v2 metadata to the same v1-era node at the same time, and
/// that must converge with each field present exactly once rather than doubled.
#[test]
fn two_peers_adding_v2_metadata_to_a_v1_node_converge_without_duplication() {
    let v1 = KbNodeDoc::new_with_client_id("n1", "Shared", "body", &[], 1);
    let seed = v1.encode();

    let mut a = KbNodeDoc::from_bytes_with_client_id(&seed, 1).unwrap();
    let mut b = KbNodeDoc::from_bytes_with_client_id(&seed, 2).unwrap();

    // Both independently upgrade the same v1 node, each setting the SAME kind
    // (the "two users performed the same migration" case) and its own property.
    //
    // Each update is kept and applied SEPARATELY: yrs update payloads are not
    // concatenable, so appending two of them into one buffer produces something
    // that is not a valid update at all.
    let ua = vec![
        a.set_kind(Some("concept")),
        a.set_todo_state(Some("NEXT")),
        a.set_properties(&[("ID".to_string(), "abc".to_string())].into()),
    ];
    let ub = vec![
        b.set_kind(Some("concept")),
        b.set_todo_state(Some("NEXT")),
        b.set_properties(&[("ROLE".to_string(), "hub".to_string())].into()),
    ];

    for u in &ub {
        a.apply_update(u).unwrap();
    }
    for u in &ua {
        b.apply_update(u).unwrap();
    }

    assert_eq!(a.kind(), b.kind(), "peers converge on kind");
    assert_eq!(a.todo_state(), b.todo_state(), "peers converge on todo");
    assert_eq!(
        a.kind().as_deref(),
        Some("concept"),
        "the same migration applied twice yields the value once, not doubled: {:?}",
        a.kind()
    );
    assert_eq!(
        a.todo_state().as_deref(),
        Some("NEXT"),
        "scalar must not concatenate: {:?}",
        a.todo_state()
    );

    // Each peer's distinct property survives — neither migration clobbered the other.
    let props = a.properties();
    assert_eq!(props, b.properties(), "peers converge on properties");
    assert_eq!(props.get("ID").map(String::as_str), Some("abc"));
    assert_eq!(props.get("ROLE").map(String::as_str), Some("hub"));
    assert_eq!(a.schema_version(), 2, "the node is v2 after the upgrade");
}

/// Gate A.4 — concurrent edits to DIFFERENT property keys both survive, and a key
/// both peers carry is not duplicated.
///
/// This is the `YMap` restatement of the ADR-092 D2 oracle: peer-equality alone is
/// free from a CRDT and proves nothing. A clear-and-refill implementation would
/// converge here too — and silently drop whichever peer's unrelated key lost.
#[test]
fn concurrent_property_edits_on_different_keys_do_not_clobber() {
    let base: std::collections::HashMap<String, String> =
        [("SHARED".to_string(), "keep".to_string())].into();
    let mut a = KbNodeDoc::new_with_client_id("n1", "T", "b", &[], 1);
    let _ = a.set_properties(&base);
    let mut b = KbNodeDoc::from_bytes_with_client_id(&a.encode(), 2).unwrap();

    let mut a_props = base.clone();
    a_props.insert("FROM_A".to_string(), "1".to_string());
    let mut b_props = base.clone();
    b_props.insert("FROM_B".to_string(), "2".to_string());

    let ua = a.set_properties(&a_props);
    let ub = b.set_properties(&b_props);

    a.apply_update(&ub).unwrap();
    b.apply_update(&ua).unwrap();

    let merged = a.properties();
    assert_eq!(merged, b.properties(), "peers converge");
    assert_eq!(
        merged.get("FROM_A").map(String::as_str),
        Some("1"),
        "A's key survived B's concurrent write: {merged:?}"
    );
    assert_eq!(
        merged.get("FROM_B").map(String::as_str),
        Some("2"),
        "B's key survived A's concurrent write: {merged:?}"
    );
    assert_eq!(
        merged.get("SHARED").map(String::as_str),
        Some("keep"),
        "the key both peers carried is intact: {merged:?}"
    );
}

/// Aliases carry the same `YArray` hazard `set_tags` did.
#[test]
fn concurrent_alias_edits_do_not_duplicate_shared_aliases() {
    let shared = vec!["primary".to_string()];
    let mut a = KbNodeDoc::new_with_client_id("n1", "T", "b", &[], 1);
    let _ = a.set_aliases(&shared);
    let mut b = KbNodeDoc::from_bytes_with_client_id(&a.encode(), 2).unwrap();

    let mut a_al = shared.clone();
    a_al.push("from-a".to_string());
    let mut b_al = shared.clone();
    b_al.push("from-b".to_string());

    let ua = a.set_aliases(&a_al);
    let ub = b.set_aliases(&b_al);
    a.apply_update(&ub).unwrap();
    b.apply_update(&ua).unwrap();

    let merged = a.aliases();
    assert_eq!(merged, b.aliases(), "peers converge");
    assert_eq!(
        merged.iter().filter(|x| *x == "primary").count(),
        1,
        "the shared alias must appear once, not once per peer: {merged:?}"
    );
    assert!(merged.contains(&"from-a".to_string()), "{merged:?}");
    assert!(merged.contains(&"from-b".to_string()), "{merged:?}");
}

/// Setting v2 fields to values they already hold must produce no ops — the
/// ADR-092 D2 no-churn rule, extended to the metadata setters.
#[test]
fn setting_v2_fields_to_their_current_values_produces_no_ops() {
    let mut node = KbNodeDoc::new_with_client_id("n1", "T", "b", &[], 1);
    let _ = node.set_kind(Some("concept"));
    let _ = node.set_todo_state(Some("NEXT"));
    let _ = node.set_aliases(&["a".to_string()]);
    let props: std::collections::HashMap<String, String> =
        [("K".to_string(), "v".to_string())].into();
    let _ = node.set_properties(&props);

    let before = node.encode();
    let _ = node.set_kind(Some("concept"));
    let _ = node.set_todo_state(Some("NEXT"));
    let _ = node.set_aliases(&["a".to_string()]);
    let _ = node.set_properties(&props);

    assert_eq!(
        node.encode().len(),
        before.len(),
        "re-setting unchanged metadata must not grow the document"
    );
}
