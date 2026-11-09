//! `KbCollectionDoc` `body_format` marker tests (mirrors `collection_format.rs`).

use super::*;

/// A collection that never set the key reads as MAE's own format — the
/// property every pre-existing KB depends on to stay writable.
#[test]
fn body_format_absent_by_default() {
    let coll = KbCollectionDoc::new("KB1", "alice");
    assert_eq!(coll.body_format(), None);
    let owned = KbCollectionDoc::new_owned("KB1", "alicefp", "alice");
    assert_eq!(owned.body_format(), None);
}

/// The marker crosses the wire: set on one replica, delivered to another as the
/// returned delta alone (not a full-state copy), read back there.
#[test]
fn body_format_delta_reaches_a_second_replica() {
    let mut a = KbCollectionDoc::new_with_client_id("KB1", "alice", 11);
    let mut b = KbCollectionDoc::from_bytes(&a.encode_state()).unwrap();
    assert_eq!(b.body_format(), None);

    let delta = a.set_body_format(BODY_FORMAT_ORG_LITERAL_V1);
    assert!(!delta.is_empty());
    b.apply_update(&delta).unwrap();

    assert_eq!(b.body_format().as_deref(), Some(BODY_FORMAT_ORG_LITERAL_V1));
    // And the full state round-trips too.
    let c = KbCollectionDoc::from_bytes(&b.encode_state()).unwrap();
    assert_eq!(c.body_format().as_deref(), Some("org-literal-v1"));
}

/// The key is additive: setting it leaves every other collection field as it
/// was, so an older peer that ignores it sees the same KB it saw before.
#[test]
fn body_format_does_not_disturb_other_collection_fields() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.add_node("n:1", "One");
    coll.set_embedding_model("m");
    let before = (
        coll.name(),
        coll.list_nodes(),
        coll.members(),
        coll.embedding_model(),
    );

    coll.set_body_format(BODY_FORMAT_ORG_LITERAL_V1);

    let after = (
        coll.name(),
        coll.list_nodes(),
        coll.members(),
        coll.embedding_model(),
    );
    assert_eq!(before, after);
}

/// The empty string is not a format: it reads as `None`, so it can clear the
/// marker without a separate delete operation.
#[test]
fn empty_body_format_reads_as_absent() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.set_body_format(BODY_FORMAT_ORG_LITERAL_V1);
    coll.set_body_format("");
    assert_eq!(coll.body_format(), None);
}

/// An unrecognised format is returned verbatim, not normalised to `None` or to
/// the one format this build knows: a reader must be able to tell "a format I
/// do not write" from "MAE's own format".
#[test]
fn unknown_body_format_is_reported_verbatim() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.set_body_format("org-literal-v9");
    assert_eq!(coll.body_format().as_deref(), Some("org-literal-v9"));
}
