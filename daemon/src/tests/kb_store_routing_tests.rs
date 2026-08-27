//! `resolve_kb_store` must route by instance uuid, never by the `primary` flag.
//!
//! **The bug this pins.** `resolve_kb_store` branched on `inst.primary` and
//! returned the DAEMON's own store for any instance carrying that flag. But
//! `primary: bool` does not mean "the machine's primary KB" — `federation.rs`
//! says it means *"this was the first-ever `KbInstance` row registered on this
//! machine — an artifact of registration order"*, and that *"the real,
//! machine-global primary KB has no `KbInstance` row at all"*.
//!
//! So on any machine whose first-registered KB is an ordinary user KB, every
//! write the daemon made for that KB went into `daemon-kb.cozo` while the KB's
//! own store was never opened and never touched. Measured on a real machine:
//! the watcher logged `updated=198 errors=0` every tick while that store's
//! mtime had not moved in days, and content written "successfully" was findable
//! only in the daemon's store. The editor had the identical defect and it was
//! fixed there; this was the unfixed copy.

use crate::handler::{resolve_kb_store, DaemonState};
use mae_kb::federation::KbInstance;
use mae_kb::CozoKbStore;
use std::sync::Arc;
use tempfile::TempDir;

fn store_at(dir: &std::path::Path, name: &str) -> Arc<CozoKbStore> {
    Arc::new(CozoKbStore::open_with_engine(dir.join(name), "sqlite").unwrap())
}

fn instance(uuid: &str, name: &str, db: &std::path::Path, primary: bool) -> KbInstance {
    let mut inst = KbInstance::local(
        uuid.to_string(),
        name.to_string(),
        db.parent().unwrap().to_path_buf(),
        db.to_path_buf(),
    );
    inst.primary = primary;
    inst
}

/// The load-bearing case: an instance flagged `primary` resolves to **its own**
/// store, not the daemon's.
#[test]
fn a_primary_flagged_instance_resolves_to_its_own_store() {
    let tmp = TempDir::new().unwrap();
    let daemon_store = store_at(tmp.path(), "daemon-kb.cozo");
    let instance_store = store_at(tmp.path(), "user-kb.sqlite");

    let mut st = DaemonState::new();
    st.store = Some(Arc::clone(&daemon_store));
    st.registry.instances.push(instance(
        "uuid-first-registered",
        "FirstRegistered",
        &tmp.path().join("user-kb.sqlite"),
        true, // <- the artifact-of-registration-order flag
    ));
    st.instance_stores
        .insert("uuid-first-registered".into(), Arc::clone(&instance_store));

    let resolved = resolve_kb_store(&st, "FirstRegistered").expect("must resolve to a store");

    assert!(
        Arc::ptr_eq(&resolved, &instance_store),
        "a `primary`-flagged instance must resolve to ITS OWN store"
    );
    assert!(
        !Arc::ptr_eq(&resolved, &daemon_store),
        "routing it to the daemon's own store is the bug: the KB's real store \
         then never receives a write while every tick reports success"
    );
}

/// The paired ordinary case, so the test above cannot pass by resolving
/// everything to the instance map for the wrong reason.
#[test]
fn an_ordinary_instance_still_resolves_to_its_store() {
    let tmp = TempDir::new().unwrap();
    let instance_store = store_at(tmp.path(), "other-kb.sqlite");

    let mut st = DaemonState::new();
    st.store = Some(store_at(tmp.path(), "daemon-kb.cozo"));
    st.registry.instances.push(instance(
        "uuid-ordinary",
        "Ordinary",
        &tmp.path().join("other-kb.sqlite"),
        false,
    ));
    st.instance_stores
        .insert("uuid-ordinary".into(), Arc::clone(&instance_store));

    let resolved = resolve_kb_store(&st, "Ordinary").expect("must resolve");
    assert!(Arc::ptr_eq(&resolved, &instance_store));
}

/// An instance with no open store resolves to `None` — NOT to the daemon's
/// store. Falling back to some other KB's store is precisely how the original
/// defect stayed invisible for so long: it looked like a success.
#[test]
fn an_instance_with_no_open_store_resolves_to_none_not_the_daemons() {
    let tmp = TempDir::new().unwrap();
    let daemon_store = store_at(tmp.path(), "daemon-kb.cozo");

    let mut st = DaemonState::new();
    st.store = Some(Arc::clone(&daemon_store));
    st.registry.instances.push(instance(
        "uuid-unopened",
        "Unopened",
        &tmp.path().join("never-opened.sqlite"),
        true,
    ));

    assert!(
        resolve_kb_store(&st, "Unopened").is_none(),
        "a missing store must be a visible absence, never a silent substitution"
    );
}
