use super::*;

#[test]
fn drain_kb_preload_populates_mirror_from_background_channel() {
    // Phase 1a: the idle-tick drain consumes the background loader's node set and
    // populates the mirror, then clears the pending channel.
    let mut editor = Editor::new();
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok(vec![mae_kb::Node::new(
        "preload:1",
        "One",
        mae_kb::NodeKind::Note,
        "b",
    )]))
    .unwrap();
    editor.kb.pending_preload = Some(rx);
    assert!(editor.kb.primary.get("preload:1").is_none());
    editor.drain_kb_preload();
    assert!(
        editor.kb.primary.get("preload:1").is_some(),
        "preload must populate the mirror"
    );
    assert!(
        editor.kb.pending_preload.is_none(),
        "channel cleared once drained"
    );
}

#[test]
fn drain_kb_preload_is_noop_while_still_loading() {
    // Empty channel = loader still running: drain must be a no-op and keep the
    // pending handle so the next tick retries.
    let mut editor = Editor::new();
    let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<mae_kb::Node>, String>>();
    editor.kb.pending_preload = Some(rx);
    editor.drain_kb_preload();
    assert!(
        editor.kb.pending_preload.is_some(),
        "still-loading must remain pending"
    );
    drop(tx);
}

#[test]
fn sqlite_multi_instance_concurrent_writes_converge() {
    // Phase 2 hard gate (adversarial, #14): two CozoKbStore handles on the SAME
    // sqlite file — two DbInstances → two independent process-local locks, the same
    // lock topology as two daemon-less processes. cozo 0.7 sets no busy_timeout, so
    // without the busy-retry ~14% of concurrent writes fail with SQLITE_BUSY. This
    // asserts that with the retry, N-way concurrent writers ALL succeed and the
    // store converges to the union of their writes (no lost writes, no corruption).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.sqlite");
    let a = std::sync::Arc::new(mae_kb::CozoKbStore::open_with_engine(&path, "sqlite").unwrap());
    a.seed_type_system().unwrap();
    let b = std::sync::Arc::new(mae_kb::CozoKbStore::open_with_engine(&path, "sqlite").unwrap());

    // Cross-visibility of sequential writes.
    a.insert_node(&mae_kb::Node::new(
        "a:seq",
        "A",
        mae_kb::NodeKind::Note,
        "x",
    ))
    .unwrap();
    b.insert_node(&mae_kb::Node::new(
        "b:seq",
        "B",
        mae_kb::NodeKind::Note,
        "x",
    ))
    .unwrap();
    assert!(
        a.get_node("b:seq").unwrap().is_some(),
        "A must see B's write"
    );
    assert!(
        b.get_node("a:seq").unwrap().is_some(),
        "B must see A's write"
    );

    // Concurrent writers on disjoint id sets — every write MUST succeed.
    let n = 50;
    let mk = |store: std::sync::Arc<mae_kb::CozoKbStore>, prefix: &'static str| {
        std::thread::spawn(move || {
            for i in 0..n {
                store
                    .insert_node(&mae_kb::Node::new(
                        format!("{prefix}:{i}"),
                        prefix,
                        mae_kb::NodeKind::Note,
                        "x",
                    ))
                    .unwrap_or_else(|e| {
                        panic!("{prefix}:{i} write must not fail under contention: {e}")
                    });
            }
        })
    };
    let ta = mk(a.clone(), "wa");
    let tb = mk(b.clone(), "wb");
    ta.join().unwrap();
    tb.join().unwrap();

    // Convergence: both writers' full id sets are present + readable from either handle.
    for i in 0..n {
        assert!(
            a.get_node(&format!("wa:{i}")).unwrap().is_some()
                && a.get_node(&format!("wb:{i}")).unwrap().is_some(),
            "store must converge to the union of both writers' nodes"
        );
    }
}

#[test]
fn external_store_change_arms_a_background_reload() {
    // Phase 4: when another process commits to the shared sqlite store, the store
    // watcher fires and `drain_kb_store_watch` arms a background mirror reload.
    let mut editor = Editor::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.cozo");
    let store =
        std::sync::Arc::new(mae_kb::CozoKbStore::open_with_engine(&path, "sqlite").unwrap());
    store.seed_type_system().unwrap();
    editor.kb.primary_cozo = Some(store.clone());
    editor.kb.store_watcher = Some(mae_kb::watch::StoreWatcher::new(&path).unwrap());
    assert!(
        editor.kb.last_local_store_write.is_none(),
        "no cooldown active"
    );

    // Another "process" commits to the store (modifies the file).
    store
        .insert_node(&mae_kb::Node::new(
            "user:ext",
            "Ext",
            mae_kb::NodeKind::Note,
            "b",
        ))
        .unwrap();

    // Poll: the external change must arm a background reload (notify is async).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut armed = false;
    while std::time::Instant::now() < deadline {
        editor.drain_kb_store_watch();
        if editor.kb.pending_preload.is_some() {
            armed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        armed,
        "external store change must arm a background mirror reload"
    );
}

#[test]
fn store_watch_reload_suppressed_within_local_write_cooldown() {
    // Phase 4: a reload must NOT fire when WE just wrote (cooldown) — otherwise
    // local edits would churn the mirror. With a fresh local-write timestamp and a
    // changed store, drain must leave `pending_preload` unset.
    let mut editor = Editor::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.cozo");
    let store =
        std::sync::Arc::new(mae_kb::CozoKbStore::open_with_engine(&path, "sqlite").unwrap());
    store.seed_type_system().unwrap();
    editor.kb.primary_cozo = Some(store.clone());
    editor.kb.store_watcher = Some(mae_kb::watch::StoreWatcher::new(&path).unwrap());
    // Pretend WE just wrote.
    editor.kb.last_local_store_write = Some(std::time::Instant::now());

    store
        .insert_node(&mae_kb::Node::new(
            "user:x",
            "X",
            mae_kb::NodeKind::Note,
            "b",
        ))
        .unwrap();

    // Give notify time to deliver, draining each tick; the cooldown must keep the
    // reload from arming for the whole window.
    for _ in 0..20 {
        editor.drain_kb_store_watch();
        assert!(
            editor.kb.pending_preload.is_none(),
            "reload must be suppressed within the local-write cooldown"
        );
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

#[test]
fn external_registry_change_adopts_new_instance() {
    // Live-refresh (section D): another mae process registers a KB
    // org-dir directly against the shared kb-registry.toml; this
    // process's registry watcher must pick it up on a later idle tick
    // without any local KB operation being run first.
    let mut editor = Editor::new();
    let data_dir = tempfile::tempdir().unwrap();
    editor.data_dir_override = Some(data_dir.path().to_path_buf());

    // Seed an empty registry file so StoreWatcher has something to watch
    // (mirrors the real startup wiring in main.rs).
    mae_kb::federation::KbRegistry::default()
        .save(data_dir.path())
        .unwrap();
    editor.kb.registry_watcher =
        Some(mae_kb::watch::StoreWatcher::new(data_dir.path().join("kb-registry.toml")).unwrap());
    assert!(
        editor.kb.last_local_registry_write.is_none(),
        "no cooldown active"
    );

    // "Another process" registers a KB directly against the same data dir
    // — this editor's in-memory registry never saw it.
    let org_dir = tempfile::tempdir().unwrap();
    let (_, uuid, saved) = mae_kb::federation::KbRegistry::update(data_dir.path(), |reg| {
        reg.register(
            "External".to_string(),
            org_dir.path().to_path_buf(),
            data_dir.path(),
            None,
        )
    });
    saved.unwrap();

    // Poll: the external change must get adopted (notify is async).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut adopted = false;
    while std::time::Instant::now() < deadline {
        editor.drain_kb_registry_watch();
        if editor.kb.instances.contains_key(&uuid) {
            adopted = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        adopted,
        "external KB registration must be adopted via the registry watcher"
    );
    assert!(
        editor.kb.registry.find(&uuid).is_some(),
        "in-memory registry must also reflect the externally-registered instance"
    );
}

#[test]
fn registry_watch_reload_suppressed_within_local_write_cooldown() {
    // A reload must NOT fire when WE just wrote (cooldown) — otherwise a
    // command that just registered/unregistered a KB would immediately
    // re-adopt/re-scan against its own fresh write.
    let mut editor = Editor::new();
    let data_dir = tempfile::tempdir().unwrap();
    editor.data_dir_override = Some(data_dir.path().to_path_buf());
    mae_kb::federation::KbRegistry::default()
        .save(data_dir.path())
        .unwrap();
    editor.kb.registry_watcher =
        Some(mae_kb::watch::StoreWatcher::new(data_dir.path().join("kb-registry.toml")).unwrap());
    // Pretend WE just wrote.
    editor.kb.last_local_registry_write = Some(std::time::Instant::now());

    let org_dir = tempfile::tempdir().unwrap();
    let (_, uuid, saved) = mae_kb::federation::KbRegistry::update(data_dir.path(), |reg| {
        reg.register(
            "External".to_string(),
            org_dir.path().to_path_buf(),
            data_dir.path(),
            None,
        )
    });
    saved.unwrap();

    for _ in 0..20 {
        editor.drain_kb_registry_watch();
        assert!(
            !editor.kb.instances.contains_key(&uuid),
            "reload must be suppressed within the local-write cooldown"
        );
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}
