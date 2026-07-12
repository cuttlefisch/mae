//! `KbCollectionDoc` basic-CRUD tests (mirrors `collection_core.rs`).

use super::*;

// --- KbCollectionDoc tests ---

#[test]
fn collection_basic_creation() {
    let coll = KbCollectionDoc::new("Research Notes", "alice");
    assert_eq!(coll.name(), "Research Notes");
    assert_eq!(coll.creator(), "alice");
    assert_eq!(coll.members(), vec!["alice"]);
    assert_eq!(coll.node_count(), 0);
}

#[test]
fn collection_add_remove_nodes() {
    let mut coll = KbCollectionDoc::new("Test", "alice");
    coll.add_node("concept:buffer", "Buffer");
    coll.add_node("concept:window", "Window");
    assert_eq!(coll.node_count(), 2);

    let nodes = coll.list_nodes();
    assert!(nodes.iter().any(|(id, _)| id == "concept:buffer"));
    assert!(nodes.iter().any(|(id, _)| id == "concept:window"));

    coll.remove_node("concept:buffer");
    assert_eq!(coll.node_count(), 1);
}

/// #156 F5: the enable-time manifest-title scrub. Blanks every cleartext title in
/// ONE delta, preserves the node ids, leaves already-blank titles alone, and is
/// idempotent (a second call has nothing to do → empty delta). The delta, applied to
/// a fresh replica, reproduces the blanked manifest (round-trip).
#[test]
fn blank_node_titles_delta_scrubs_manifest_and_is_idempotent() {
    let mut coll = KbCollectionDoc::new("Test", "alice");
    coll.add_node("concept:a", "Secret Alpha");
    coll.add_node("concept:b", "Secret Beta");
    coll.add_node("concept:c", ""); // already blank
    assert!(coll.list_nodes().iter().any(|(_, t)| t == "Secret Alpha"));

    // A replica that SHARES this collection's lineage (built from its state — the
    // daemon applies the delta to the same `kbc:` doc, never an independent rebuild,
    // which would mint a divergent yrs client_id that wouldn't merge — the #179 rule).
    let mut replica = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();

    let delta = coll.blank_node_titles_delta();
    assert!(!delta.is_empty(), "produced a blanking delta");

    let nodes = coll.list_nodes();
    assert_eq!(nodes.len(), 3, "node ids preserved (only titles blanked)");
    assert!(
        nodes.iter().all(|(_, t)| t.is_empty()),
        "every manifest title is blank after the scrub: {nodes:?}"
    );

    // Idempotent — nothing left to blank.
    assert!(
        coll.blank_node_titles_delta().is_empty(),
        "second scrub is a no-op (empty delta)"
    );

    // The delta is a real applicable collection update: the lineage-sharing replica
    // replays it and sees the blanked manifest (round-trip), not the cleartext titles.
    replica.apply_update(&delta).unwrap();
    assert!(
        replica.list_nodes().iter().all(|(_, t)| t.is_empty()),
        "applying the delta blanks the titles on a replica too: {:?}",
        replica.list_nodes()
    );
}

/// #156 F5 — the AT-REST oracle (the attacker's test). Blanking the manifest title
/// must not leave the ORIGINAL cleartext title recoverable in the `kbc:` doc's
/// persisted state bytes (a yrs overwrite can keep the old value as a tombstone). The
/// daemon stores `encode_state()`, so an attacker greps exactly these bytes.
#[test]
fn blank_node_titles_delta_purges_old_title_from_state_bytes() {
    let canary = b"SECRET-TITLE-CANARY-do-not-survive";
    let mut coll = KbCollectionDoc::new("Test", "alice");
    coll.add_node("concept:a", std::str::from_utf8(canary).unwrap());
    // Precondition (non-vacuous): the cleartext title IS in the state before scrub.
    assert!(
        coll.encode_state()
            .windows(canary.len())
            .any(|w| w == canary),
        "precondition: the title is in the state before blanking (else the test is vacuous)"
    );

    coll.blank_node_titles_delta();

    assert!(
        !coll
            .encode_state()
            .windows(canary.len())
            .any(|w| w == canary),
        "the original cleartext title MUST NOT survive in the kbc: state bytes after blanking"
    );
}

#[test]
fn collection_members() {
    let mut coll = KbCollectionDoc::new("Test", "alice");
    coll.add_member("bob");
    coll.add_member("bob"); // duplicate — should not be added
    assert_eq!(coll.members(), vec!["alice", "bob"]);

    coll.remove_member("alice");
    assert_eq!(coll.members(), vec!["bob"]);
}

#[test]
fn collection_set_creator_restamps_and_seeds_member() {
    // A collection built with a client-claimed creator...
    let mut coll = KbCollectionDoc::new("Test", "client-name");
    assert_eq!(coll.creator(), "client-name");
    // ...is re-stamped to the authenticated identity, which becomes a member.
    coll.set_creator("alice");
    assert_eq!(coll.creator(), "alice", "creator overridden");
    assert!(
        coll.members().contains(&"alice".to_string()),
        "creator seeded as member"
    );
    // Idempotent: no duplicate member on re-stamp.
    coll.set_creator("alice");
    assert_eq!(
        coll.members().iter().filter(|m| *m == "alice").count(),
        1,
        "no duplicate member"
    );
}
