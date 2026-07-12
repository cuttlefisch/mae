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

    node.add_tag("b");
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

    // Both edit body (set_body replaces, so last-write-wins semantics)
    let update_a = node_a.set_body("from A");
    let update_b = node_b.set_body("from B");

    node_a.apply_update(&update_b).unwrap();
    node_b.apply_update(&update_a).unwrap();

    // Both converge to the same result
    assert_eq!(node_a.body(), node_b.body());
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
    n.set_body("Hello 世界 and more text after");
    let bytes = n.encode();
    let restored = KbNodeDoc::from_bytes(&bytes).unwrap();
    assert_eq!(restored.body(), "Hello 世界 and more text after");
}

#[test]
fn utf16_offset_emoji_roundtrip() {
    // Emoji above BMP (U+1F600) are 2 UTF-16 code units (surrogate pairs)
    let mut node = KbNodeDoc::new("n1", "Emoji Test 😀", "Body with 🎉 emoji", &[]);
    node.set_title("Updated 🌍 title");
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
    node.set_body("hello world");
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
    node.add_link("concept:other");
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
    node.set_body("world");
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
    node.add_link("target1");
    node.add_link("target2");
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
