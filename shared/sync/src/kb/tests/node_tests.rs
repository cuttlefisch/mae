//! `KbNodeDoc` tests.

use super::*;

// --- KbNodeDoc tests ---

#[test]
fn new_node_schema() {
    let node = KbNodeDoc::new(
        "concept:test",
        "Test Node",
        "Some body text",
        &["tag1".to_string(), "tag2".to_string()],
    );
    assert_eq!(node.id(), "concept:test");
    assert_eq!(node.title(), "Test Node");
    assert_eq!(node.body(), "Some body text");
    assert_eq!(node.tags(), vec!["tag1", "tag2"]);
    assert!(node.links().is_empty());
}

#[test]
fn set_tags_replaces_and_syncs() {
    // B-18: set_tags produces a real CRDT delta that converges a peer's tags.
    let mut owner = KbNodeDoc::new("n1", "T", "b", &["a".to_string(), "b".to_string()]);
    // Peer shares the lineage (loaded from the owner's encoded state).
    let mut peer = KbNodeDoc::from_bytes(&owner.encode()).unwrap();
    let sv = peer.state_vector();
    assert_eq!(peer.tags(), vec!["a", "b"]);

    // Owner replaces the tag set → diff → peer applies → converges.
    owner.set_tags(&["a".to_string(), "c".to_string()]);
    assert_eq!(owner.tags(), vec!["a", "c"]);
    let diff = owner.encode_diff(&sv).unwrap();
    peer.apply_update(&diff).unwrap();
    assert_eq!(
        peer.tags(),
        vec!["a", "c"],
        "peer must converge on the owner's set_tags delta"
    );
}

#[test]
fn set_title_generates_update() {
    let mut node = KbNodeDoc::new("n1", "Old Title", "", &[]);
    let update = node.set_title("New Title");
    assert!(!update.is_empty());
    assert_eq!(node.title(), "New Title");
}

#[test]
fn set_body_generates_update() {
    let mut node = KbNodeDoc::new("n1", "T", "old body", &[]);
    let update = node.set_body("new body content");
    assert!(!update.is_empty());
    assert_eq!(node.body(), "new body content");
}

#[test]
fn tag_operations() {
    let mut node = KbNodeDoc::new("n1", "T", "", &["a".to_string()]);
    assert_eq!(node.tags(), vec!["a"]);

    let _ = node.add_tag("b");
    assert_eq!(node.tags(), vec!["a", "b"]);

    node.remove_tag("a");
    assert_eq!(node.tags(), vec!["b"]);
}

#[test]
fn two_clients_merge_body() {
    let mut node_a = KbNodeDoc::new("n1", "T", "hello", &[]);
    let state = node_a.encode();

    let mut node_b = KbNodeDoc::from_bytes(&state).unwrap();
    assert_eq!(node_b.body(), "hello");

    let update_a = node_a.set_body("from A");
    let update_b = node_b.set_body("from B");

    node_a.apply_update(&update_b).unwrap();
    node_b.apply_update(&update_a).unwrap();

    // Both converge to the same result
    assert_eq!(node_a.body(), node_b.body());
}

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

#[test]
fn encode_decode_roundtrip() {
    let node = KbNodeDoc::new(
        "concept:arch",
        "Architecture",
        "The system uses...",
        &["core".to_string(), "design".to_string()],
    );
    let bytes = node.encode();

    let restored = KbNodeDoc::from_bytes(&bytes).unwrap();
    assert_eq!(restored.id(), "concept:arch");
    assert_eq!(restored.title(), "Architecture");
    assert_eq!(restored.body(), "The system uses...");
    assert_eq!(restored.tags(), vec!["core", "design"]);
}

// --- UTF-16 offset tests ---

#[test]
fn utf16_offset_cjk_roundtrip() {
    let node = KbNodeDoc::new("n1", "CJK", "", &[]);
    // CJK characters are multi-byte in UTF-8 but single code unit in UTF-16 (BMP)
    let mut n = KbNodeDoc::from_bytes(&node.encode()).unwrap();
    let _ = n.set_body("Hello 世界 and more text after");
    let bytes = n.encode();
    let restored = KbNodeDoc::from_bytes(&bytes).unwrap();
    assert_eq!(restored.body(), "Hello 世界 and more text after");
}

#[test]
fn utf16_offset_emoji_roundtrip() {
    // Emoji above BMP (U+1F600) are 2 UTF-16 code units (surrogate pairs)
    let mut node = KbNodeDoc::new("n1", "Emoji Test 😀", "Body with 🎉 emoji", &[]);
    let _ = node.set_title("Updated 🌍 title");
    let bytes = node.encode();
    let restored = KbNodeDoc::from_bytes(&bytes).unwrap();
    assert_eq!(restored.title(), "Updated 🌍 title");
    assert_eq!(restored.body(), "Body with 🎉 emoji");
}

#[test]
fn utf16_two_client_cjk_merge() {
    let mut node_a = KbNodeDoc::new_with_client_id("n1", "T", "你好", &[], 1);
    let state = node_a.encode();
    let mut node_b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();

    let update_a = node_a.set_body("你好世界");
    let update_b = node_b.set_body("你好朋友");

    node_a.apply_update(&update_b).unwrap();
    node_b.apply_update(&update_a).unwrap();

    assert_eq!(node_a.body(), node_b.body());
}

// --- Client ID tests ---

#[test]
fn new_with_client_id_preserves_identity() {
    let node = KbNodeDoc::new_with_client_id("n1", "T", "B", &[], 42);
    assert_eq!(node.id(), "n1");
    assert_eq!(node.title(), "T");
    // Verify client_id is set on the yrs Doc
    assert_eq!(node.doc().client_id().get(), 42);
}

#[test]
fn from_bytes_with_client_id_preserves_identity() {
    let original = KbNodeDoc::new_with_client_id("n1", "T", "B", &[], 10);
    let bytes = original.encode();
    let restored = KbNodeDoc::from_bytes_with_client_id(&bytes, 20).unwrap();
    assert_eq!(restored.id(), "n1");
    assert_eq!(restored.doc().client_id().get(), 20);
}

// --- encode_diff tests ---

#[test]
fn encode_diff_produces_valid_update() {
    let mut node = KbNodeDoc::new("n1", "T", "hello", &[]);
    let sv_before = node.state_vector();
    let _ = node.set_body("hello world");
    let diff = node.encode_diff(&sv_before).unwrap();
    assert!(!diff.is_empty());

    // Apply the diff to a copy from before the change
    let mut old = KbNodeDoc::from_bytes(&{
        let orig = KbNodeDoc::new("n1", "T", "hello", &[]);
        orig.encode()
    })
    .unwrap();
    old.apply_update(&diff).unwrap();
    // After applying diff, old should have "hello world"
    // (The diff contains the set_body which replaces the entire text)
    assert!(old.body().contains("hello"));
}

// --- materialize tests ---

#[test]
fn materialize_extracts_all_fields() {
    let mut node = KbNodeDoc::new(
        "concept:test",
        "Test",
        "Body",
        &["tag1".to_string(), "tag2".to_string()],
    );
    let _ = node.add_link("concept:other");
    let mat = node.materialize();
    assert_eq!(mat.id, "concept:test");
    assert_eq!(mat.title, "Test");
    assert_eq!(mat.body, "Body");
    assert_eq!(mat.tags, vec!["tag1", "tag2"]);
    assert_eq!(mat.links, vec!["concept:other"]);
}

// --- content_hash tests ---

#[test]
fn content_hash_changes_on_edit() {
    let mut node = KbNodeDoc::new("n1", "T", "hello", &[]);
    let hash1 = node.content_hash();
    let _ = node.set_body("world");
    let hash2 = node.content_hash();
    assert_ne!(hash1, hash2);
}

#[test]
fn content_hash_stable_for_same_content() {
    let node1 = KbNodeDoc::new("n1", "T", "hello", &["a".to_string()]);
    let node2 = KbNodeDoc::new("n1", "T", "hello", &["a".to_string()]);
    assert_eq!(node1.content_hash(), node2.content_hash());
}

// --- apply_update returns changed flag ---

#[test]
fn apply_update_returns_changed_flag() {
    let mut node_a = KbNodeDoc::new_with_client_id("n1", "T", "hello", &[], 1);
    let state = node_a.encode();
    let mut node_b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();

    let update = node_b.set_body("changed");
    let changed = node_a.apply_update(&update).unwrap();
    assert!(changed, "content changed, flag should be true");

    // Apply same update again — no content change
    // (yrs deduplicates, so the flag should be false)
    let update2 = node_b.set_body("changed"); // no-op — same content
    let changed2 = node_a.apply_update(&update2).unwrap();
    // The body is still "changed" so hash should match
    assert!(!changed2, "same content, flag should be false");
}

// --- 3-client convergence ---

#[test]
fn three_client_concurrent_edits_converge() {
    let mut a = KbNodeDoc::new_with_client_id("n1", "T", "base", &[], 1);
    let state = a.encode();
    let mut b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();
    let mut c = KbNodeDoc::from_bytes_with_client_id(&state, 3).unwrap();

    // All three concurrently edit different fields
    let u_a = a.set_title("Title from A");
    let u_b = b.add_tag("tag-from-b");
    let u_c = c.add_link("link-from-c");

    // Apply all updates to all clients
    a.apply_update(&u_b).unwrap();
    a.apply_update(&u_c).unwrap();
    b.apply_update(&u_a).unwrap();
    b.apply_update(&u_c).unwrap();
    c.apply_update(&u_a).unwrap();
    c.apply_update(&u_b).unwrap();

    // All three should converge
    assert_eq!(a.title(), b.title());
    assert_eq!(b.title(), c.title());
    assert_eq!(a.title(), "Title from A");
    assert_eq!(a.tags(), b.tags());
    assert_eq!(b.tags(), c.tags());
    assert!(a.tags().contains(&"tag-from-b".to_string()));
    assert_eq!(a.links(), b.links());
    assert_eq!(b.links(), c.links());
    assert!(a.links().contains(&"link-from-c".to_string()));
}

// --- Multi-field concurrent edits ---

#[test]
fn concurrent_title_and_body_edits() {
    let mut a = KbNodeDoc::new_with_client_id("n1", "T", "B", &[], 1);
    let state = a.encode();
    let mut b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();

    let u_a = a.set_title("New Title");
    let u_b = b.set_body("New Body");

    a.apply_update(&u_b).unwrap();
    b.apply_update(&u_a).unwrap();

    assert_eq!(a.title(), "New Title");
    assert_eq!(a.body(), "New Body");
    assert_eq!(a.title(), b.title());
    assert_eq!(a.body(), b.body());
}

// --- Link and meta operations ---

#[test]
fn link_operations() {
    let mut node = KbNodeDoc::new("n1", "T", "", &[]);
    let _ = node.add_link("target1");
    let _ = node.add_link("target2");
    assert_eq!(node.links(), vec!["target1", "target2"]);

    node.remove_link("target1");
    assert_eq!(node.links(), vec!["target2"]);
}

#[test]
fn meta_operations() {
    let mut node = KbNodeDoc::new("n1", "T", "", &[]);
    node.set_meta("author", "alice");
    node.set_meta("version", "2");
    assert_eq!(node.get_meta("author"), Some("alice".to_string()));
    assert_eq!(node.get_meta("version"), Some("2".to_string()));
    assert_eq!(node.get_meta("missing"), None);
}
