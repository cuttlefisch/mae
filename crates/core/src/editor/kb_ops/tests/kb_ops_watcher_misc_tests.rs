use super::*;

#[test]
fn watcher_starts_on_register() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    assert!(
        editor.kb.watchers.contains_key(&result.uuid),
        "watcher should start on register"
    );
}

#[test]
fn watcher_removed_on_unregister() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();
    assert!(editor.kb.watchers.contains_key(&uuid));
    editor.kb_unregister("TestNotes");
    assert!(!editor.kb.watchers.contains_key(&uuid));
}

#[test]
fn watcher_drains_new_file() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();

    // Write a new org file
    std::fs::write(
        dir.path().join("new-note.org"),
        ":PROPERTIES:\n:ID: watch-test-new\n:END:\n#+title: Watched Note\n\nNew.\n",
    )
    .unwrap();

    // Poll until watcher picks it up (filesystem events are async)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        editor.drain_kb_watchers();
        if editor.kb.instances[&uuid].get("watch-test-new").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        editor.kb.instances[&uuid].get("watch-test-new").is_some(),
        "new org file should be auto-ingested by watcher"
    );
}

// --- W1: KB options tests ---

#[test]
fn kb_options_registered() {
    let editor = Editor::new();
    for name in &[
        "kb_watcher_enabled",
        "kb_watcher_debounce_ms",
        "kb_max_drain_events",
        "kb_search_excerpt_length",
        "kb_search_max_results",
        "kb_auto_register",
    ] {
        assert!(
            editor.option_registry.find(name).is_some(),
            "option '{}' not found in registry",
            name
        );
    }
    // Also check aliases
    assert!(editor.option_registry.find("kb-watcher-enabled").is_some());
    assert!(editor.option_registry.find("kb-max-drain-events").is_some());
}

#[test]
fn kb_options_get_set_roundtrip() {
    let mut editor = Editor::new();
    // Bool roundtrip
    assert_eq!(editor.get_option("kb_watcher_enabled").unwrap().0, "true");
    editor.set_option("kb_watcher_enabled", "false").unwrap();
    assert_eq!(editor.get_option("kb_watcher_enabled").unwrap().0, "false");
    // Int roundtrip
    editor.set_option("kb_watcher_debounce_ms", "1000").unwrap();
    assert_eq!(
        editor.get_option("kb_watcher_debounce_ms").unwrap().0,
        "1000"
    );
    editor.set_option("kb_max_drain_events", "50").unwrap();
    assert_eq!(editor.get_option("kb_max_drain_events").unwrap().0, "50");
    editor
        .set_option("kb_search_excerpt_length", "300")
        .unwrap();
    assert_eq!(
        editor.get_option("kb_search_excerpt_length").unwrap().0,
        "300"
    );
    editor.set_option("kb_search_max_results", "10").unwrap();
    assert_eq!(editor.get_option("kb_search_max_results").unwrap().0, "10");
    // Bool roundtrip
    editor.set_option("kb_auto_register", "true").unwrap();
    assert_eq!(editor.get_option("kb_auto_register").unwrap().0, "true");
}

// --- W4: Watcher hardening tests ---

#[test]
fn drain_debounce_skips_recent() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();

    // Write a file and wait for watcher to see it
    std::fs::write(
        dir.path().join("debounce-first.org"),
        ":PROPERTIES:\n:ID: debounce-first\n:END:\n#+title: First\n\ntest\n",
    )
    .unwrap();
    // Drain until first file is picked up (establishes timestamp)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        editor.drain_kb_watchers();
        if editor.kb.last_drain.contains_key(&uuid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(editor.kb.last_drain.contains_key(&uuid));

    // Now set a very long debounce
    editor.kb.watcher_debounce_ms = 60_000;

    // Write another file
    std::fs::write(
        dir.path().join("debounce-second.org"),
        ":PROPERTIES:\n:ID: debounce-second\n:END:\n#+title: Second\n\ntest\n",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));

    // This drain should be debounced — second node should NOT appear
    editor.drain_kb_watchers();
    assert!(
        editor.kb.instances[&uuid].get("debounce-second").is_none(),
        "debounce should have skipped the drain"
    );
}

#[test]
fn watcher_disabled_skips_drain() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb.watcher_enabled = false;
    // Register should skip watcher creation
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    assert!(
        !editor.kb.watchers.contains_key(&result.uuid),
        "watcher should not be created when disabled"
    );
    // drain should be a no-op
    editor.drain_kb_watchers();
}

#[test]
fn watcher_error_count_exposed() {
    let dir = create_test_org_dir();
    let watcher = mae_kb::watch::OrgDirWatcher::new(dir.path()).unwrap();
    // Initial error count should be 0
    assert_eq!(watcher.error_count(), 0);
}

/// `docs/INOTIFY_INSTANCE_EXHAUSTION.md`, asserted end-to-end through the real
/// `kb_register` path rather than at the `mae-kb` API: registering N KBs must
/// not cost N inotify instances. `max_user_instances` is 128 per USER (not per
/// process), so an editor that spends one per registered KB starves every other
/// application on the machine — the reported symptom.
///
/// Measured as a delta (other tests in this process hold watchers of their own)
/// and skipped where instances aren't a concept (macOS/FSEvents), per
/// `mae_kb::watch::inotify_instance_count`'s contract.
#[test]
fn registering_many_kbs_does_not_multiply_inotify_instances() {
    let Some(before) = mae_kb::watch::inotify_instance_count() else {
        return; // not Linux
    };
    let dirs: Vec<TempDir> = (0..5).map(|_| create_test_org_dir()).collect();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    for (i, d) in dirs.iter().enumerate() {
        assert!(
            editor.kb_register(&format!("Notes{i}"), d.path()).is_some(),
            "registration {i} must succeed"
        );
    }
    assert_eq!(
        editor.kb.watchers.len(),
        dirs.len(),
        "all watchers attached"
    );
    let after = mae_kb::watch::inotify_instance_count().unwrap();
    let spent = after.saturating_sub(before);
    // The design bound is 1 (see the mae-kb twin of this test); 2 only absorbs
    // a concurrent test in this binary re-creating the shared watcher mid-measure.
    assert!(
        spent <= 2,
        "5 registered KBs must cost ~1 inotify instance, not one each \
         (before={before}, after={after}, spent={spent})"
    );
}

#[test]
fn kb_federated_search_deduplicates() {
    let mut editor = Editor::new();
    // Insert a node locally
    editor
        .kb_create_node("dedup-test", "Dedup", "body", mae_kb::NodeKind::Note)
        .unwrap();
    // Insert same node in a federated instance
    let mut inst = mae_kb::KnowledgeBase::new();
    inst.insert(mae_kb::Node::new(
        "dedup-test",
        "Dedup",
        mae_kb::NodeKind::Note,
        "body",
    ));
    editor.kb.instances.insert("inst-1".to_string(), inst);

    let results = editor.kb_federated_search("Dedup");
    let dedup_count = results.iter().filter(|(_, n)| n.id == "dedup-test").count();
    assert_eq!(dedup_count, 1, "same node ID should appear only once");
    // Local result should win (instance_name is None)
    let (inst_name, _) = results.iter().find(|(_, n)| n.id == "dedup-test").unwrap();
    assert!(
        inst_name.is_none(),
        "local result should win over federated"
    );
}

// --- W5: Observability tests ---

#[test]
fn kb_watcher_stats_update_on_drain() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();

    // Write a new file and wait for watcher
    std::fs::write(
        dir.path().join("stats-test.org"),
        ":PROPERTIES:\n:ID: stats-test\n:END:\n#+title: Stats\n\ntest\n",
    )
    .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        editor.drain_kb_watchers();
        if editor.kb.instances[&uuid].get("stats-test").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    assert!(
        editor.kb.watcher_stats.events_upserted > 0,
        "events_upserted should be positive after drain"
    );
}

#[test]
fn perf_stats_kb_fields_default_zero() {
    let editor = Editor::new();
    assert_eq!(editor.perf_stats.kb_search_latency_us, 0);
    assert_eq!(editor.perf_stats.kb_watcher_drain_us, 0);
    assert_eq!(editor.perf_stats.kb_watcher_events, 0);
}

#[test]
fn kb_register_does_not_clobber_user_dirs() {
    // Resolve real user dirs the same way the production code does.
    let home = std::env::var("HOME").unwrap();
    let real_config = PathBuf::from(&home).join(".config/mae/kb-registry.toml");
    let real_data = PathBuf::from(&home).join(".local/share/mae/kb-registry.toml");

    // Record mtimes before
    let config_mtime = real_config.metadata().ok().and_then(|m| m.modified().ok());
    let data_mtime = real_data.metadata().ok().and_then(|m| m.modified().ok());

    // Run a register + unregister cycle with test dirs
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("IsolationTest", dir.path()).unwrap();
    editor.kb_unregister(&result.uuid);

    // Verify mtimes unchanged
    let config_mtime_after = real_config.metadata().ok().and_then(|m| m.modified().ok());
    let data_mtime_after = real_data.metadata().ok().and_then(|m| m.modified().ok());
    assert_eq!(
        config_mtime, config_mtime_after,
        "config dir kb-registry.toml was modified by test"
    );
    assert_eq!(
        data_mtime, data_mtime_after,
        "data dir kb-registry.toml was modified by test"
    );
}

/// CF1 (SECURITY_REVIEW §6.3): enabling E2E MUST surface the honesty advisory at
/// the point of action — and a non-e2e mode MUST NOT. Selective oracle: the WARN
/// message names the *actual* caveats (no forward secrecy, metadata visible), not
/// an incidental string; and the negative `mode="none"` case must produce no
/// advisory (the failure mode that would let the label silently oversell).
#[test]
fn enabling_e2e_surfaces_the_caveat_advisory_at_point_of_action() {
    use crate::editor::KbCollabAction;

    // Enable E2E → exactly one WARN advisory, naming the real caveats.
    let mut editor = Editor::new();
    editor.queue_kb_collab_action(KbCollabAction::SetEncryption {
        kb_id: "kb-cf1".into(),
        mode: "e2e".into(),
    });
    let warns = editor
        .message_log
        .entries_filtered(crate::messages::MessageLevel::Warn);
    let advisory: Vec<_> = warns
        .iter()
        .filter(|e| e.target == "kb-encryption")
        .collect();
    assert_eq!(
        advisory.len(),
        1,
        "exactly one E2E enable advisory expected, got {}",
        advisory.len()
    );
    let msg = &advisory[0].message;
    // Selective oracle: the meaningful caveats, not an incidental token.
    assert!(
        msg.contains("No forward secrecy"),
        "advisory must disclose the no-FS caveat"
    );
    assert!(
        msg.to_lowercase().contains("metadata is visible"),
        "advisory must disclose metadata exposure"
    );
    assert!(
        msg.contains("NOT retroactive"),
        "advisory must warn enable-before-sharing"
    );
    // The intent is still queued (the advisory doesn't block the action).
    assert!(matches!(
        editor.collab.pending_intent,
        Some(crate::editor::CollabIntent::KbSetEncryption { .. })
    ));

    // Negative: a non-e2e mode must NOT emit the advisory (the oversell failure mode).
    let mut editor2 = Editor::new();
    editor2.queue_kb_collab_action(KbCollabAction::SetEncryption {
        kb_id: "kb-cf1".into(),
        mode: "none".into(),
    });
    let advisory2 = editor2
        .message_log
        .entries()
        .into_iter()
        .filter(|e| e.target == "kb-encryption")
        .count();
    assert_eq!(
        advisory2, 0,
        "no advisory should fire for a non-e2e SetEncryption mode"
    );
}

/// Pre-dogfood review: the Scheme/AI surface can lower several lifecycle
/// actions in ONE apply cycle (bulk member onboarding). The single
/// `pending_intent` slot used to keep only the LAST, silently dropping the
/// rest — an owner who scripted "add a, add b, add c" got only c, with no
/// error. Assert all N survive (1 in the slot + the rest fanned out through
/// `reconnect_intents`, the same one-per-tick queue the reconnect path drains).
#[test]
fn batched_kb_collab_actions_do_not_collapse_to_the_last() {
    use crate::editor::{CollabIntent, KbCollabAction};
    let mut editor = Editor::new();
    for fp in ["SHA256:a", "SHA256:b", "SHA256:c"] {
        editor.queue_kb_collab_action(KbCollabAction::AddMember {
            kb_id: "kb".into(),
            member: fp.into(),
            role: "editor".into(),
        });
    }
    // 1 in the active slot + 2 fanned out = 3 total, none dropped.
    assert!(
        editor.collab.pending_intent.is_some(),
        "first action in the slot"
    );
    assert_eq!(
        editor.collab.reconnect_intents.len(),
        2,
        "the other two batched actions must be queued, not overwritten"
    );

    // FIFO order preserved: slot = a, queue = [b, c].
    let members: Vec<String> = std::iter::once(editor.collab.pending_intent.clone().unwrap())
        .chain(editor.collab.reconnect_intents.iter().cloned())
        .map(|i| match i {
            CollabIntent::KbAddMember { member, .. } => member,
            other => panic!("expected KbAddMember, got {other:?}"),
        })
        .collect();
    assert_eq!(members, vec!["SHA256:a", "SHA256:b", "SHA256:c"]);
}

// --- Issue #474: kb_cleanup_orphans must use federation-reconciled orphan detection ---
//
// `kb_cleanup_orphans` is destructive (`Editor::kb.primary.remove(id)`), which is exactly why
// the `FederatedQuery::health_report` orphan-detection bug (a node whose only real incoming
// link lives in a sibling federated instance was wrongly reported orphaned) was a real
// data-loss vector, not just a cosmetic health-report inaccuracy. These are the first tests
// for `kb_cleanup_orphans` (previously zero coverage) and are deliberately adversarial (CLAUDE.md
// principle #14): the positive "must NOT delete" case is paired with a negative "must STILL
// delete a genuine orphan" case in the identical federated topology, so the fix can't be
// proven merely by becoming universally lenient.

/// Build a primary CozoDB store + one federated instance store, wire them into `editor.kb`,
/// and rebuild the query layer — the minimum federation shape needed for `kb_cleanup_orphans`
/// to route its orphan detection through `FederatedQuery::health_report` (via
/// `self.kb.query_layer()`) instead of falling back to the primary's own un-federated
/// in-memory `health_report()`.
fn setup_federated_primary_and_instance(
    editor: &mut Editor,
    tmp: &TempDir,
) -> (
    std::sync::Arc<mae_kb::CozoKbStore>,
    std::sync::Arc<mae_kb::CozoKbStore>,
) {
    let primary_store =
        std::sync::Arc::new(mae_kb::CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
    let inst_store =
        std::sync::Arc::new(mae_kb::CozoKbStore::open(tmp.path().join("inst.cozo")).unwrap());
    editor.kb.primary_cozo = Some(primary_store.clone());
    editor
        .kb
        .instance_stores
        .insert("inst-uuid".to_string(), inst_store.clone());
    editor.kb.rebuild_query_layer();
    assert!(
        editor.kb.query_layer().is_some(),
        "sanity: federated query layer must be active for this test to exercise the real bug"
    );
    (primary_store, inst_store)
}

#[test]
fn kb_cleanup_orphans_does_not_delete_primary_node_with_genuine_cross_instance_incoming_link() {
    let mut editor = Editor::new();
    let tmp = TempDir::new().unwrap();
    let (primary_store, inst_store) = setup_federated_primary_and_instance(&mut editor, &tmp);

    // A primary node with zero LOCAL links, but a real inbound link recorded only in the
    // federated instance — the exact false-positive shape issue #474 fixes.
    let survivor = mae_kb::Node::new(
        "user:survivor",
        "Survivor",
        mae_kb::NodeKind::Note,
        "no local links",
    );
    primary_store.insert_node(&survivor).unwrap();
    editor.kb.primary.insert(survivor);

    inst_store
        .insert_node(&mae_kb::Node::new(
            "inst:linker",
            "Linker",
            mae_kb::NodeKind::Note,
            "",
        ))
        .unwrap();
    inst_store
        .add_typed_link("inst:linker", "user:survivor", "references", 1.0)
        .unwrap();

    editor.kb_cleanup_orphans();

    assert!(
        editor.kb.primary.contains("user:survivor"),
        "a primary node with a real incoming link from a federated instance must NOT be \
         deleted by kb_cleanup_orphans"
    );
}

#[test]
fn kb_cleanup_orphans_still_removes_a_genuine_orphan_in_a_federated_setup() {
    let mut editor = Editor::new();
    let tmp = TempDir::new().unwrap();
    let (primary_store, inst_store) = setup_federated_primary_and_instance(&mut editor, &tmp);

    // Adversarial complement, same federation topology: a DIFFERENT primary node that is
    // genuinely unlinked everywhere — proves the fix didn't make cleanup overly lenient.
    let genuine_orphan = mae_kb::Node::new(
        "user:genuine-orphan",
        "Genuinely alone",
        mae_kb::NodeKind::Note,
        "",
    );
    primary_store.insert_node(&genuine_orphan).unwrap();
    editor.kb.primary.insert(genuine_orphan);

    // Unrelated instance content, touching neither node above.
    inst_store
        .insert_node(&mae_kb::Node::new(
            "inst:unrelated-a",
            "A",
            mae_kb::NodeKind::Note,
            "",
        ))
        .unwrap();
    inst_store
        .insert_node(&mae_kb::Node::new(
            "inst:unrelated-b",
            "B",
            mae_kb::NodeKind::Note,
            "",
        ))
        .unwrap();
    inst_store
        .add_typed_link("inst:unrelated-a", "inst:unrelated-b", "references", 1.0)
        .unwrap();

    let removed = editor.kb_cleanup_orphans();

    assert!(
        !editor.kb.primary.contains("user:genuine-orphan"),
        "a node with genuinely zero links anywhere in the federation must still be removed \
         by kb_cleanup_orphans"
    );
    assert!(
        removed >= 1,
        "expected at least the genuine orphan to be counted as removed, got {removed}"
    );
}

// --- Issue #485: kb_cleanup_orphans must delete from the OWNING store, not always primary ---
//
// The two tests above (#474) only ever assert against `editor.kb.primary` — a federated-
// instance orphan slips straight through both: it's neither `user:survivor` (a primary node,
// not touched either way) nor would a bug leaving it un-deleted in `kb.instances` be caught
// by an assertion that only ever inspects `kb.primary`. This is exactly the gap #485 reports:
// `kb_cleanup_orphans` called `self.kb.primary.remove(id)` unconditionally for every orphan
// id, so a genuine orphan actually OWNED by a federated instance was never removed from
// `kb.instances` at all (silent no-op) — or, if primary happened to independently reuse the
// same bare id, the WRONG node was deleted. Adversarial per CLAUDE.md #14: this test must
// fail against the pre-fix code (which never touches `kb.instances`) and pass against the
// owner-aware fix.
#[test]
fn kb_cleanup_orphans_removes_a_genuine_orphan_owned_by_a_federated_instance() {
    let mut editor = Editor::new();
    let tmp = TempDir::new().unwrap();
    let (_primary_store, inst_store) = setup_federated_primary_and_instance(&mut editor, &tmp);

    // Register the in-memory copy too (`kb_owner_of` resolves ownership via
    // `kb.instances`, not `kb.instance_stores` — a real federated instance always has
    // both; the fix's federated branch must remove from both).
    let mut inst_mirror = mae_kb::KnowledgeBase::new();

    // The genuine orphan: lives ONLY in the federated instance, with zero links
    // anywhere in the federation.
    let orphan = mae_kb::Node::new(
        "inst:federated-orphan",
        "Federated Orphan",
        mae_kb::NodeKind::Note,
        "",
    );
    inst_store.insert_node(&orphan).unwrap();
    inst_mirror.insert(orphan);

    // Unrelated linked pair in the same instance, so the health report has real
    // non-orphan content to distinguish from (not a "delete everything" unicorn case).
    let a = mae_kb::Node::new("inst:linked-a", "A", mae_kb::NodeKind::Note, "");
    let b = mae_kb::Node::new("inst:linked-b", "B", mae_kb::NodeKind::Note, "");
    inst_store.insert_node(&a).unwrap();
    inst_store.insert_node(&b).unwrap();
    inst_store
        .add_typed_link("inst:linked-a", "inst:linked-b", "references", 1.0)
        .unwrap();
    inst_mirror.insert(a);
    inst_mirror.insert(b);

    editor
        .kb
        .instances
        .insert("inst-uuid".to_string(), inst_mirror);

    // Sanity: kb_owner_of must resolve this id to the federated instance, not
    // primary and not "not found" — otherwise this test would exercise the wrong
    // branch of the fix.
    assert_eq!(
        editor.kb_owner_of("inst:federated-orphan"),
        Some(Some("inst-uuid".to_string())),
        "sanity: the orphan must resolve as owned by the federated instance"
    );

    let removed = editor.kb_cleanup_orphans();

    assert!(
        removed >= 1,
        "expected the federated-instance orphan to be counted as removed, got {removed}"
    );
    assert!(
        !editor.kb.instances["inst-uuid"].contains("inst:federated-orphan"),
        "a genuine orphan owned by a federated instance must be removed from \
         kb.instances — the old primary-only deletion path would leave this present"
    );
    // The persisted store copy must also be gone (mirrors kb_delete_node's own
    // federated branch, which deletes from `instance_stores` in addition to the
    // in-memory `instances` copy).
    assert!(
        inst_store
            .get_node("inst:federated-orphan")
            .unwrap()
            .is_none(),
        "the federated instance's persisted store must also have the orphan deleted"
    );
    // The unrelated linked pair must survive untouched.
    assert!(editor.kb.instances["inst-uuid"].contains("inst:linked-a"));
    assert!(editor.kb.instances["inst-uuid"].contains("inst:linked-b"));
}

// --- Issue #485: kb_owner_of resolving to "not found anywhere" must be a graceful no-op ---
//
// The fix's removal loop has a `None` arm (id in the orphan list but unresolvable by
// `kb_owner_of`) that "shouldn't normally happen" in practice — an id only lands in
// `orphan_ids` because some store's health report found it — but must not panic if it ever
// does. There's no way to force the real orphan-detection path to report an id that
// `kb_owner_of` then fails to resolve without reaching into private internals, so this test
// instead pins `kb_owner_of`'s own contract directly (the precondition the loop's `None` arm
// depends on) and confirms `kb_cleanup_orphans` runs to completion without panicking on an
// otherwise-empty federation.
#[test]
fn kb_owner_of_returns_none_for_an_id_that_does_not_exist_anywhere() {
    let mut editor = Editor::new();
    let tmp = TempDir::new().unwrap();
    let _ = setup_federated_primary_and_instance(&mut editor, &tmp);
    editor
        .kb
        .instances
        .insert("inst-uuid".to_string(), mae_kb::KnowledgeBase::new());

    assert_eq!(
        editor.kb_owner_of("this-id-was-never-created-anywhere"),
        None,
        "kb_owner_of must resolve to None (not found) rather than defaulting to a store"
    );

    // Calling kb_cleanup_orphans in this state (nothing orphaned, federation set up but
    // empty) must not panic — the None-handling branch is graceful even when reachable.
    let removed = editor.kb_cleanup_orphans();
    assert_eq!(removed, 0, "empty federation has nothing to clean up");
}
