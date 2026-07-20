//! ADR-018 (#73): `load_collection` must migrate a legacy v1 (label-based) collection
//! to the v2 fingerprint-anchored schema via `authorized_keys` resolution — not only
//! on owner re-share. Exercises the REAL `load_collection` path (not `migrate_if_legacy`
//! in isolation, which `shared/sync`'s own unit tests already cover) with a genuine
//! legacy-encoded collection fixture, per CLAUDE.md principle #14: the failure mode
//! under test is "a legacy collection silently fails to migrate (or migrates only
//! in-memory and doesn't survive a reload) in production," not "the migration
//! function is theoretically correct."

use super::*;
use mae_mcp::identity::{AuthorizedKeys, Identity};

#[tokio::test]
async fn load_collection_migrates_legacy_v1_via_authorized_keys_and_persists_it() {
    let store = test_doc_store();

    let alice = Identity::generate("alice");
    let alice_fp = alice.fingerprint();
    let bob = Identity::generate("bob");
    let bob_fp = bob.fingerprint();

    let dir = tempfile::tempdir().unwrap();
    let ak_path = dir.path().join("authorized_keys");
    let mut authorized = AuthorizedKeys::load(&ak_path);
    authorized.add(alice.public()).unwrap();
    authorized.add(bob.public()).unwrap();
    store.set_authorized_keys_path(ak_path);

    // A genuine legacy v1 collection: label creator + members YArray, no schema key —
    // exactly what a pre-ADR-018 daemon would have persisted.
    let mut legacy = KbCollectionDoc::new("Legacy KB", "alice");
    legacy.add_member("bob");
    assert_eq!(
        legacy.schema_version(),
        0,
        "fixture must actually be legacy"
    );
    store
        .apply_update("kbc:legacy-kb", &legacy.encode_state(), None)
        .await
        .expect("seed the legacy collection directly into storage");

    let migrated = load_collection(&store, "legacy-kb")
        .await
        .expect("load_collection must succeed on a legacy collection");
    assert_eq!(migrated.schema_version(), 2, "migrated to v2 in-memory");
    assert_eq!(
        migrated.owner(),
        alice_fp,
        "creator label resolved to its fingerprint"
    );
    assert_eq!(migrated.role_of(&alice_fp), Some(SyncRole::Owner));
    assert_eq!(migrated.role_of(&bob_fp), Some(SyncRole::Editor));

    // The adversarial crux: a legacy collection that migrates only in the returned,
    // throwaway in-memory copy but never touches storage would pass every assertion
    // above and still be broken in production — every OTHER daemon process (or this
    // one after a restart) reading the same doc would see the untouched v1 bytes and
    // silently re-derive `legacy:*` fallback principals forever, exactly the bug this
    // issue reports. Decode straight from storage, bypassing `load_collection` entirely.
    let (raw_state, _sv) = store
        .encode_state_and_sv("kbc:legacy-kb")
        .await
        .expect("collection doc must exist in storage");
    let persisted = KbCollectionDoc::from_bytes(&raw_state).expect("persisted bytes decode");
    assert_eq!(
        persisted.schema_version(),
        2,
        "migration must be durably persisted, not merely returned in-memory"
    );
    assert_eq!(persisted.owner(), alice_fp);
    assert_eq!(persisted.role_of(&bob_fp), Some(SyncRole::Editor));

    // Idempotent: loading again must not error or double-migrate.
    let reloaded = load_collection(&store, "legacy-kb")
        .await
        .expect("re-loading an already-migrated collection must succeed");
    assert_eq!(reloaded.schema_version(), 2);
}

#[tokio::test]
async fn load_collection_falls_back_to_legacy_principal_for_an_unresolvable_label() {
    let store = test_doc_store();

    // authorized_keys knows about nobody from this legacy collection — the resolver
    // must fall back to `legacy:<label>` (matching `shared/sync`'s own unit test),
    // NOT panic, NOT silently drop the member, and NOT leave the collection at v1.
    let dir = tempfile::tempdir().unwrap();
    let ak_path = dir.path().join("authorized_keys");
    let _ = AuthorizedKeys::load(&ak_path); // creates the (empty) trust store
    store.set_authorized_keys_path(ak_path);

    let legacy = KbCollectionDoc::new("Ghost KB", "ghost-creator");
    store
        .apply_update("kbc:ghost-kb", &legacy.encode_state(), None)
        .await
        .expect("seed the legacy collection");

    let migrated = load_collection(&store, "ghost-kb")
        .await
        .expect("an unresolvable creator label must still migrate, not error");
    assert_eq!(migrated.schema_version(), 2);
    assert_eq!(migrated.owner(), "legacy:ghost-creator");
    assert_eq!(
        migrated.role_of("legacy:ghost-creator"),
        Some(SyncRole::Owner)
    );
}

#[tokio::test]
async fn load_collection_leaves_v2_collections_untouched_when_no_authorized_keys_path_is_set() {
    // Mirrors production psk/none auth modes, where `set_authorized_keys_path` is
    // never called: `load_collection` must behave exactly as before this fix (no
    // migration attempted, no panic on a missing trust store).
    let store = test_doc_store();
    let coll = KbCollectionDoc::new_owned("Owned KB", &fp("alice"), "alice");
    store
        .apply_update("kbc:owned-kb", &coll.encode_state(), None)
        .await
        .expect("seed an already-v2 collection");

    let loaded = load_collection(&store, "owned-kb")
        .await
        .expect("load_collection must succeed with no authorized_keys_path configured");
    assert_eq!(loaded.schema_version(), 2);
    assert_eq!(loaded.owner(), fp("alice"));
}
