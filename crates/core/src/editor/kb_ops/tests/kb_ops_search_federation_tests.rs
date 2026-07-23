use super::*;

#[test]
fn kb_open_instance_store_defaults_to_sqlite_not_sled() {
    // Regression: kb_register/kb_reimport/the federation loader used the bare
    // `CozoKbStore::open()`, which is hardcoded to the sled engine — ignoring
    // `kb_storage_engine` (default sqlite) entirely. Every registered federated
    // instance was permanently stuck on sled's single-writer exclusive lock, so
    // a second mae frontend could never open the same instance concurrently —
    // regardless of the option the user configured. A sled store is a
    // directory; a sqlite store is a file — that's the discriminator.
    let editor = Editor::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("instance.cozo");
    editor.kb_open_instance_store(&path).unwrap();
    assert!(
        path.is_file(),
        "default engine must be sqlite (a file), not sled (a directory)"
    );
}

#[test]
fn kb_open_instance_store_migrates_an_existing_sled_instance() {
    // A pre-existing legacy sled federated instance (e.g. registered before
    // Phase 2c, or hand-created) must be auto-migrated to sqlite on next open —
    // matching the primary store's behavior — not opened as sled forever.
    let editor = Editor::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("instance.cozo");

    {
        let sled = mae_kb::CozoKbStore::open_with_engine(&path, "sled").unwrap();
        sled.seed_type_system().unwrap();
        sled.insert_node(&mae_kb::Node::new(
            "user:legacy",
            "Legacy",
            mae_kb::NodeKind::Note,
            "pre-migration content",
        ))
        .unwrap();
    }
    assert!(path.is_dir(), "sanity: sled store is a directory");

    let migrated = editor.kb_open_instance_store(&path).unwrap();
    assert!(path.is_file(), "path must be a sqlite file after migration");
    assert!(
        migrated.get_node("user:legacy").unwrap().is_some(),
        "migration must preserve existing nodes, not drop them"
    );
}

#[test]
fn kb_register_allows_a_second_concurrent_frontend_to_open_the_same_instance() {
    // The actual user-facing bug: two mae GUI frontends both pointed at the same
    // registered KB instance. Before the fix, the FIRST frontend's kb_register
    // opened the instance as sled and kept the handle open for the process
    // lifetime; a SECOND frontend's attempt to open the same instance store hit
    // sled's exclusive dir lock and failed (silently falling back to a
    // non-persistent in-memory import — the exact bug reported against the
    // "arisnova" KB). With sqlite as the engine, a second handle must succeed
    // while the first is still open — the same topology that already lets N
    // daemon-less processes share the primary store.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();
    // The first "frontend"'s handle is still held open here (in instance_stores).
    assert!(editor.kb.instance_stores.contains_key(&uuid));

    let db_path = editor.kb.registry.find(&uuid).unwrap().db_path.clone();
    let second_frontend = mae_kb::CozoKbStore::open_with_engine(&db_path, "sqlite");
    assert!(
        second_frontend.is_ok(),
        "a second frontend must be able to open the same registered instance \
             concurrently: {:?}",
        second_frontend.err()
    );
    assert!(
        second_frontend
            .unwrap()
            .get_node("test-note-1")
            .unwrap()
            .is_some(),
        "the second frontend must see the first frontend's imported nodes"
    );
}

#[test]
fn kb_federated_search_finds_across_instances() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path());

    // Search should find nodes from federated instance
    let results = editor.kb_federated_search("Note");
    let federated: Vec<_> = results.iter().filter(|(name, _)| name.is_some()).collect();
    assert!(!federated.is_empty());
}

#[test]
fn kb_federated_search_scope_filters_instances() {
    use mae_kb::KbScope;
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path());

    let count_federated =
        |r: &[(Option<String>, mae_kb::Node)]| r.iter().filter(|(name, _)| name.is_some()).count();

    // All: includes the federated TestNotes instance.
    let all = editor.kb_federated_search_scoped("Note", &KbScope::All);
    assert!(count_federated(&all) > 0, "All should include federated");

    // LocalOnly: drops every federated result.
    let local = editor.kb_federated_search_scoped("Note", &KbScope::LocalOnly);
    assert_eq!(count_federated(&local), 0, "LocalOnly excludes federated");

    // Named: selects exactly the named instance's results.
    let named = editor.kb_federated_search_scoped("Note", &KbScope::Named("TestNotes".into()));
    assert!(count_federated(&named) > 0, "Named selects the instance");
    assert!(
        named
            .iter()
            .all(|(name, _)| name.is_none() || name.as_deref() == Some("TestNotes")),
        "Named yields only that instance (+ local)"
    );

    // RemoteOnly: TestNotes is a local import (not shared), so no results.
    let remote = editor.kb_federated_search_scoped("Note", &KbScope::RemoteOnly);
    assert_eq!(
        count_federated(&remote),
        0,
        "RemoteOnly excludes non-shared local imports"
    );
}

#[test]
fn kb_search_recency_floats_visited_to_top() {
    let mut editor = Editor::new();
    editor.kb.search_sort = "recency".to_string();

    // Pick two nodes that both match a common query but aren't the top
    // relevance hit, then visit the second one and confirm it leads.
    let baseline = editor.kb_federated_search("buffer");
    assert!(baseline.len() >= 2, "need ≥2 matches for the query");
    // A match that is NOT already first under relevance.
    let promote = baseline[1].1.id.clone();

    // No visits yet → recency order == relevance order (stable).
    let ids_before: Vec<String> = editor
        .kb_federated_search("buffer")
        .iter()
        .map(|(_, n)| n.id.clone())
        .collect();
    assert_eq!(ids_before.first(), Some(&baseline[0].1.id.clone()));

    // Visit the promoted node; it should now sort first.
    editor.kb.record_visit(&promote);
    let ids_after: Vec<String> = editor
        .kb_federated_search("buffer")
        .iter()
        .map(|(_, n)| n.id.clone())
        .collect();
    assert_eq!(
        ids_after.first(),
        Some(&promote),
        "visited node should float to the top under recency sort"
    );
}

#[test]
fn kb_search_sort_option_accepts_recency() {
    let mut editor = Editor::new();
    assert!(editor.set_option("kb_search_sort", "recency").is_ok());
    assert_eq!(editor.kb.search_sort, "recency");
    assert_eq!(
        editor.get_option("kb_search_sort").map(|(v, _)| v),
        Some("recency".to_string())
    );
    // Invalid value is rejected and leaves the setting unchanged.
    assert!(editor.set_option("kb_search_sort", "bogus").is_err());
    assert_eq!(editor.kb.search_sort, "recency");
}

#[test]
fn kb_search_scope_option_round_trip() {
    let mut editor = Editor::new();
    // Keywords always validate.
    for kw in ["all", "local", "remote"] {
        assert!(editor.set_option("kb_search_scope", kw).is_ok());
        assert_eq!(editor.kb.search_scope, kw);
    }
    // An unknown instance name is rejected (no instance registered).
    assert!(editor.set_option("kb_search_scope", "NoSuchKB").is_err());
    // A registered instance name validates.
    let dir = create_test_org_dir();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path());
    assert!(editor.set_option("kb_search_scope", "TestNotes").is_ok());
    assert_eq!(
        editor.get_option("kb_search_scope").map(|(v, _)| v),
        Some("TestNotes".to_string())
    );
}

#[test]
fn ai_guidance_kb_option_round_trip() {
    let mut editor = Editor::new();
    // Empty (disabled, the default) always validates.
    assert!(editor.set_option("ai_guidance_kb", "").is_ok());
    assert_eq!(editor.ai_guidance_kb, "");
    // "primary" always validates.
    assert!(editor.set_option("ai_guidance_kb", "primary").is_ok());
    assert_eq!(editor.ai_guidance_kb, "primary");
    // Issue #370 drift fix: unlike `kb_search_scope`, an unknown/not-yet-registered
    // instance name is intentionally ACCEPTED, not rejected -- init.scm evaluates
    // BEFORE KB federation populates `self.kb.registry`, so the shipped default
    // ("MaePractices") would always fail eager validation here even though it
    // resolves correctly moments later. Resolution is deliberately deferred to
    // read time (`crates/ai/src/guidance.rs::read_guidance_kb_context`, which is
    // already best-effort and silently no-ops for an unresolvable name).
    assert!(editor.set_option("ai_guidance_kb", "no-such-kb").is_ok());
    assert_eq!(editor.ai_guidance_kb, "no-such-kb");
    // A registered instance name also validates, same as before.
    let dir = create_test_org_dir();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("dev-practices", dir.path());
    assert!(editor.set_option("ai_guidance_kb", "dev-practices").is_ok());
    assert_eq!(
        editor.get_option("ai_guidance_kb").map(|(v, _)| v),
        Some("dev-practices".to_string())
    );
}

#[test]
fn kb_find_candidates_small_kb_returns_all() {
    let editor = Editor::new();
    // The seed manual is well under the lazy threshold.
    assert!(editor.kb_loaded_node_count() <= Editor::KB_FIND_LAZY_THRESHOLD);
    let all = editor.kb_all_node_triples();
    let cands = editor.kb_find_candidates("");
    assert_eq!(cands.len(), all.len(), "small KB should return every node");
}

#[test]
fn kb_find_candidates_large_kb_is_bounded_but_query_reachable() {
    let mut editor = Editor::new();
    // Push past the lazy threshold with synthetic nodes, including one
    // distinctive node far beyond the empty-query window.
    for i in 0..(Editor::KB_FIND_LAZY_THRESHOLD + 500) {
        editor.kb.primary.insert(mae_kb::Node::new(
            format!("note:bulk{i}"),
            format!("Bulk Note {i}"),
            mae_kb::NodeKind::Note,
            "filler body",
        ));
    }
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:zebra-marker",
        "Zebra Marker",
        mae_kb::NodeKind::Note,
        "uniquely findable",
    ));
    assert!(editor.kb_loaded_node_count() > Editor::KB_FIND_LAZY_THRESHOLD);

    // Empty query: bounded window, not the whole KB.
    let empty = editor.kb_find_candidates("");
    assert!(
        empty.len() <= Editor::KB_FIND_LAZY_LIMIT,
        "large-KB window should be bounded, got {}",
        empty.len()
    );

    // A targeted query still reaches a node outside the empty window — the
    // ranker scans the whole KB, so lazy completion stays full-KB-reachable.
    let hits = editor.kb_find_candidates("zebra marker");
    assert!(
        hits.iter().any(|(id, _, _)| id == "note:zebra-marker"),
        "targeted query must find the distinctive node at scale"
    );
}

#[test]
fn kb_find_candidates_empty_query_defaults_to_activity_order_not_alphabetical() {
    let mut editor = Editor::new();
    assert_eq!(
        editor.kb.search_sort, "relevance",
        "sanity check: default sort"
    );
    // "note:zzz-recent" sorts LAST alphabetically among these three, but is
    // the most recently accessed -- an empty-query kb-find must default to
    // activity order (most-recently-active first), not let the meaningless
    // "relevance" default silently degenerate to alphabetical-by-id.
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:aaa-old",
        "Old note",
        mae_kb::NodeKind::Note,
        "body",
    ));
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:mmm-mid",
        "Mid note",
        mae_kb::NodeKind::Note,
        "body",
    ));
    let (y, m, d) = today_ymd();
    let today = mae_kb::activity::format_date(y, m, d);
    let mut recent = mae_kb::Node::new(
        "note:zzz-recent",
        "Recent note",
        mae_kb::NodeKind::Note,
        "body",
    );
    recent.properties.insert("last-accessed".to_string(), today);
    editor.kb.primary.insert(recent);

    let cands = editor.kb_find_candidates("");
    let ids: Vec<&str> = cands.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(
        ids.first(),
        Some(&"note:zzz-recent"),
        "most recently accessed node should be first for an empty query, got {ids:?}"
    );
}

#[test]
fn kb_find_candidates_respects_explicit_alphabetical_override_even_on_empty_query() {
    let mut editor = Editor::new();
    editor.set_option("kb_search_sort", "alphabetical").unwrap();
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:aaa-old",
        "Old note",
        mae_kb::NodeKind::Note,
        "body",
    ));
    let (y, m, d) = today_ymd();
    let today = mae_kb::activity::format_date(y, m, d);
    let mut recent = mae_kb::Node::new(
        "note:zzz-recent",
        "Recent note",
        mae_kb::NodeKind::Note,
        "body",
    );
    recent.properties.insert("last-accessed".to_string(), today);
    editor.kb.primary.insert(recent);

    let cands = editor.kb_find_candidates("");
    let ids: Vec<&str> = cands.iter().map(|(id, _, _)| id.as_str()).collect();
    // Editor::new() seeds ~1000 manual-KB nodes, so "note:aaa-old" won't be
    // globally first -- check its position RELATIVE to "note:zzz-recent"
    // instead: alphabetically "aaa" sorts before "zzz", so if this held,
    // the explicit alphabetical choice was correctly left untouched. Under
    // the (wrong) activity default, zzz-recent's non-zero score would put
    // it first instead.
    let pos_old = ids.iter().position(|&id| id == "note:aaa-old").unwrap();
    let pos_recent = ids.iter().position(|&id| id == "note:zzz-recent").unwrap();
    assert!(
        pos_old < pos_recent,
        "an explicit alphabetical sort choice must stay alphabetical on an \
         empty query, not be silently overridden by the activity default \
         (note:aaa-old at {pos_old}, note:zzz-recent at {pos_recent})"
    );
}

#[test]
fn kb_find_candidates_nonempty_query_behavior_unchanged_by_empty_query_default() {
    // Regression guard: the empty-query activity default must only apply
    // when query.is_empty() -- a non-empty query's candidate set is
    // unaffected (same nodes as kb_all_node_triples, no filtering here;
    // ranking/filtering for non-empty queries happens client-side via the
    // palette's fuzzy filter).
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:aaa-old",
        "Old note",
        mae_kb::NodeKind::Note,
        "body",
    ));
    let (y, m, d) = today_ymd();
    let today = mae_kb::activity::format_date(y, m, d);
    let mut recent = mae_kb::Node::new(
        "note:zzz-recent",
        "Recent note",
        mae_kb::NodeKind::Note,
        "body",
    );
    recent.properties.insert("last-accessed".to_string(), today);
    editor.kb.primary.insert(recent);

    let all = editor.kb_all_node_triples();
    let queried = editor.kb_find_candidates("note");
    assert_eq!(
        all.len(),
        queried.len(),
        "a non-empty query must return the same candidate set as kb_all_node_triples \
         (small-KB path), unaffected by the empty-query activity default"
    );
}

#[test]
fn kb_find_candidates_reaches_federated_instance_nodes_at_scale() {
    let mut editor = Editor::new();
    let mut inst = mae_kb::KnowledgeBase::new();
    inst.insert(mae_kb::Node::new(
        "note:federated-zebra",
        "Federated Zebra Marker",
        mae_kb::NodeKind::Note,
        "uniquely findable in a federated instance",
    ));
    editor.kb.instances.insert("test-instance".into(), inst);

    // Push primary past the lazy threshold, same as the sibling test above.
    for i in 0..(Editor::KB_FIND_LAZY_THRESHOLD + 500) {
        editor.kb.primary.insert(mae_kb::Node::new(
            format!("note:bulk{i}"),
            format!("Bulk Note {i}"),
            mae_kb::NodeKind::Note,
            "filler body",
        ));
    }
    assert!(editor.kb_loaded_node_count() > Editor::KB_FIND_LAZY_THRESHOLD);

    // A targeted query must still reach a node that lives ONLY in a
    // federated instance, not primary — this is exactly the bug
    // kb_find_candidates had: the lazy branch searched primary alone,
    // making federated content permanently unreachable through kb-find
    // once the KB tipped past the threshold, regardless of query.
    let hits = editor.kb_find_candidates("federated zebra");
    assert!(
        hits.iter().any(|(id, _, _)| id == "note:federated-zebra"),
        "targeted query must find a federated-instance-only node at scale"
    );
}

#[test]
fn kb_find_palette_lazy_refresh_repopulates_on_query() {
    let mut editor = Editor::new();
    for i in 0..(Editor::KB_FIND_LAZY_THRESHOLD + 100) {
        editor.kb.primary.insert(mae_kb::Node::new(
            format!("note:bulk{i}"),
            format!("Bulk Note {i}"),
            mae_kb::NodeKind::Note,
            "filler",
        ));
    }
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:platypus",
        "Platypus",
        mae_kb::NodeKind::Note,
        "distinctive",
    ));

    // Open kb-find: bounded initial window.
    assert!(editor.dispatch_builtin("kb-find"));
    let initial = editor.command_palette.as_ref().unwrap().entries.len();
    assert!(initial <= Editor::KB_FIND_LAZY_LIMIT);

    // Type a query, then refresh: the distinctive node is now reachable.
    if let Some(p) = editor.command_palette.as_mut() {
        p.query = "platypus".to_string();
    }
    editor.kb_find_palette_query_changed();
    let entries: Vec<String> = editor
        .command_palette
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(
        entries.iter().any(|id| id == "note:platypus"),
        "lazy refresh should surface the queried node"
    );
}

#[test]
fn kb_set_search_scope_command_opens_picker() {
    let mut editor = Editor::new();
    assert!(editor.command_palette.is_none());
    assert!(editor.dispatch_builtin("kb-set-search-scope"));
    let palette = editor.command_palette.as_ref().expect("picker should open");
    assert_eq!(
        palette.purpose,
        crate::command_palette::PalettePurpose::SetKbSearchScope
    );
    // Keyword scopes are always present (no instances registered here).
    let names: Vec<&str> = palette.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["all", "local", "remote"]);
}

#[test]
fn kb_visit_log_is_monotonic() {
    let mut editor = Editor::new();
    editor.kb.record_visit("concept:buffer");
    editor.kb.record_visit("concept:window");
    editor.kb.record_visit("concept:buffer"); // re-visit bumps ahead
    assert!(editor.kb.visit_rank("concept:buffer") > editor.kb.visit_rank("concept:window"));
    assert_eq!(editor.kb.visit_rank("never-visited"), 0);
}

#[test]
fn kb_federated_get_local_first() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path());

    // Get from federated instance
    let result = editor.kb_federated_get("test-note-1");
    assert!(result.is_some());
    let (inst_name, node) = result.unwrap();
    assert_eq!(inst_name, Some("TestNotes".to_string()));
    assert_eq!(node.title, "Note One");
}

#[test]
fn kb_register_nonexistent_path() {
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("Bad", Path::new("/nonexistent/path"));
    assert!(result.is_none());
    assert!(editor.status_msg.contains("does not exist"));
}

#[test]
fn kb_register_canonicalizes_org_dir() {
    // #303: registering with a non-canonical path (here, a redundant
    // `subdir/..` component) must store the canonical form so a later
    // comparison/re-derivation against `org_dir` doesn't drift from
    // what was actually walked at import time.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);

    let canonical = dir.path().canonicalize().unwrap();
    let noncanonical = dir.path().join("subdir").join("..");
    assert_ne!(
        noncanonical, canonical,
        "test setup must actually be non-canonical"
    );

    let result = editor
        .kb_register("TestNotes", &noncanonical)
        .expect("registration should succeed");
    let instance = editor.kb.registry.find(&result.uuid).unwrap();
    assert_eq!(
        instance.org_dir, canonical,
        "registry must store the canonicalized org_dir, not the literal argument"
    );
}
