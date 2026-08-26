//! KB cutover, Phase 1: a detached instance's store is the source of truth, and
//! **no** `.org` ingest may overwrite it.
//!
//! The audit behind #103 found eleven ingest paths, several of which need no user
//! intent — a buffer save, a watcher tick, a 500 ms daemon timer, a startup
//! agenda pass. Ingest is a destructive whole-row `:put` (`update_node` IS
//! `insert_node`, with no merge anywhere), so any path that skips the policy
//! silently reverts the user's store to a stale archive.
//!
//! These tests are therefore written per PATH, not per happy case: the failure
//! mode is "one path nobody remembered", and a single end-to-end test would pass
//! while nine paths stayed open.

use super::*;
use mae_kb::federation::{IngestPolicy, KbInstance, KbInstanceKind};

use super::super::daily::DailyBacking;

const UUID: &str = "uuid-detached";

/// A detached instance whose store holds content the `.org` file does not.
///
/// The divergence is the whole point: if the store and the file agreed, an
/// ingest would be undetectable and the test would pass vacuously.
fn detached_instance_with_divergent_store(editor: &mut Editor, dir: &std::path::Path) {
    let path = dir.join("note.org");
    std::fs::write(
        &path,
        ":PROPERTIES:\n:ID: note-a\n:END:\n#+title: note-a\n\nSTALE ARCHIVE CONTENT.\n",
    )
    .unwrap();

    let mut kb = mae_kb::KnowledgeBase::new();
    let mut node = mae_kb::Node::new(
        "note-a",
        "note-a",
        mae_kb::NodeKind::Note,
        "AUTHORITATIVE STORE CONTENT",
    );
    node.source_file = Some(path.clone());
    kb.insert(node);
    // A node that exists ONLY in the store — an ingest would retract it.
    kb.insert(mae_kb::Node::new(
        "store-only",
        "store-only",
        mae_kb::NodeKind::Note,
        "created in the store, never written to disk",
    ));
    editor.kb.instances.insert(UUID.into(), kb);

    let org_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    editor.kb.registry.instances.push(KbInstance {
        uuid: UUID.into(),
        name: "detached".into(),
        org_dir,
        db_path: dir.join("kb.db"),
        primary: false,
        enabled: true,
        last_import: None,
        collab_id: None,
        shared: false,
        remote_peers: Vec::new(),
        last_sync: None,
        ai_residency: mae_kb::federation::AiResidency::default(),
        project_root: None,
        project_key: None,
        kind: KbInstanceKind::default(),
        ingest_policy: IngestPolicy::StoreIsTruth,
        priority: 0,
        remote_hub: None,
    });
}

/// Assert the store still disagrees with the file — i.e. nothing ingested.
fn assert_store_intact(editor: &Editor, context: &str) {
    let kb = editor.kb.instances.get(UUID).expect("instance present");
    assert_eq!(
        kb.get("note-a").map(|n| n.body.as_str()),
        Some("AUTHORITATIVE STORE CONTENT"),
        "{context}: the .org file overwrote a detached instance's store"
    );
    assert!(
        kb.get("store-only").is_some(),
        "{context}: an ingest retracted a store-only node from a detached instance"
    );
}

/// PATH: buffer save. `:w` on any file inside a KB directory reimports it.
#[test]
fn saving_a_file_does_not_ingest_into_a_detached_instance() {
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    detached_instance_with_divergent_store(&mut editor, dir.path());

    editor.kb_reimport_file(&dir.path().join("note.org"));

    assert_store_intact(&editor, "buffer save");
}

/// PATH: explicit `:kb-reimport`, the one ingest a user actually asks for.
/// Refused rather than obeyed — the store is truth, so "reimport" would mean
/// "discard my content", which is never what the word implies.
#[test]
fn explicit_reimport_is_refused_for_a_detached_instance() {
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _tmp = with_test_dirs(&mut editor);
    detached_instance_with_divergent_store(&mut editor, dir.path());

    assert!(
        editor.kb_reimport("detached", None).is_none(),
        "an explicit reimport of a detached instance must be refused"
    );
    assert_store_intact(&editor, "explicit :kb-reimport");
}

/// PATH: the startup agenda ingest, which runs on EVERY launch and writes
/// `kb.primary` from `org_agenda_files` — the one nobody associates with
/// ingestion because it is spelled "agenda".
#[test]
fn the_startup_agenda_pass_does_not_ingest_into_a_detached_primary() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("agenda.org"),
        ":PROPERTIES:\n:ID: agenda-node\n:END:\n#+title: agenda-node\n\nFROM DISK.\n",
    )
    .unwrap();

    let mut editor = Editor::new();
    editor.kb.registry.primary_ingest_policy = IngestPolicy::StoreIsTruth;
    editor.org_agenda_files = vec![dir.path().display().to_string()];

    let before = editor.kb.primary.list_ids(None).len();
    editor.ingest_agenda_files();

    assert_eq!(
        editor.kb.primary.list_ids(None).len(),
        before,
        "the agenda pass ingested into a detached primary"
    );
    assert!(
        editor.kb.primary.get("agenda-node").is_none(),
        "the agenda pass must not add nodes to a detached primary"
    );
}

/// The policy must not leak across instances: detaching one KB must not stop
/// another from ingesting. A guard implemented as a global flag would pass every
/// test above and fail this one.
#[test]
fn detaching_one_instance_does_not_stop_another_from_ingesting() {
    let detached_dir = TempDir::new().unwrap();
    let attached_dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    detached_instance_with_divergent_store(&mut editor, detached_dir.path());

    let path = attached_dir.path().join("live.org");
    std::fs::write(
        &path,
        ":PROPERTIES:\n:ID: live-node\n:END:\n#+title: live-node\n\nINGESTED FROM DISK.\n",
    )
    .unwrap();
    let mut kb = mae_kb::KnowledgeBase::new();
    let mut node = mae_kb::Node::new("live-node", "live-node", mae_kb::NodeKind::Note, "old");
    node.source_file = Some(path.clone());
    kb.insert(node);
    editor.kb.instances.insert("uuid-attached".into(), kb);
    let org_dir = attached_dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| attached_dir.path().to_path_buf());
    editor.kb.registry.instances.push(KbInstance {
        uuid: "uuid-attached".into(),
        name: "attached".into(),
        org_dir,
        db_path: attached_dir.path().join("kb.db"),
        primary: false,
        enabled: true,
        last_import: None,
        collab_id: None,
        shared: false,
        remote_peers: Vec::new(),
        last_sync: None,
        ai_residency: mae_kb::federation::AiResidency::default(),
        project_root: None,
        project_key: None,
        kind: KbInstanceKind::default(),
        ingest_policy: IngestPolicy::FromOrgDir,
        priority: 0,
        remote_hub: None,
    });

    editor.kb_reimport_file(&path);

    let ingested = editor
        .kb
        .instances
        .get("uuid-attached")
        .and_then(|kb| kb.get("live-node"))
        .map(|n| n.body.clone())
        .unwrap_or_default();
    // Containment, not equality: an ingested body is currently the WHOLE FILE
    // including its `:PROPERTIES:` drawer (#655 — `org.rs` assigns whole-file
    // content to `body`). Asserting equality here would encode that defect as
    // expected behaviour and break when #655 is fixed; what this test is about
    // is that the ingest HAPPENED.
    assert!(
        ingested.contains("INGESTED FROM DISK."),
        "an ATTACHED instance must still ingest normally — the policy is \
         per-instance. Body was: {ingested:?}"
    );
    assert_store_intact(&editor, "sibling instance ingesting");
}

/// Round-trip through the real setter, including persistence, and prove the
/// policy actually takes effect afterwards rather than merely being stored.
#[test]
fn detach_then_attach_round_trips_and_takes_effect() {
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _tmp = with_test_dirs(&mut editor);
    detached_instance_with_divergent_store(&mut editor, dir.path());
    // The setter goes through `KbRegistry::update`, which reads the registry
    // from DISK — that is the point (the policy must survive a restart, since
    // the startup agenda ingest is one of the paths it stops). So the instance
    // has to be persisted before the setter can find it.
    let data_dir = editor.mae_data_dir().expect("test data dir");
    std::fs::create_dir_all(&data_dir).unwrap();
    editor
        .kb
        .registry
        .save(&data_dir)
        .expect("persist registry");
    // Start attached so the transition under test is real.
    editor
        .kb_set_ingest_policy("detached", IngestPolicy::FromOrgDir)
        .unwrap();

    editor
        .kb_set_ingest_policy("detached", IngestPolicy::StoreIsTruth)
        .unwrap();
    editor.kb_reimport_file(&dir.path().join("note.org"));
    assert_store_intact(&editor, "after :kb-detach");

    // Re-attaching restores ingest — otherwise detach would be a one-way door.
    editor
        .kb_set_ingest_policy("detached", IngestPolicy::FromOrgDir)
        .unwrap();
    editor.kb_reimport_file(&dir.path().join("note.org"));
    // Containment for the same reason as above (#655): an ingested body is
    // currently the whole file. The claim under test is that the ARCHIVE won —
    // i.e. ingest resumed — not what shape the body has.
    let reattached = editor
        .kb
        .instances
        .get(UUID)
        .and_then(|kb| kb.get("note-a"))
        .map(|n| n.body.clone())
        .unwrap_or_default();
    assert!(
        reattached.contains("STALE ARCHIVE CONTENT."),
        "after :kb-attach the org directory must be authoritative again. \
         Body was: {reattached:?}"
    );

    assert!(
        editor
            .kb_set_ingest_policy("no-such-kb", IngestPolicy::StoreIsTruth)
            .is_err(),
        "an unknown KB name must be reported, not silently ignored"
    );
}

/// The primary KB has no `KbInstance` row, so its policy lives on the registry.
/// Both alias spellings must reach it — `kb_set_ai_residency` shipped with
/// exactly this bug, where "default" silently did nothing while "primary" worked.
#[test]
fn both_primary_aliases_set_the_primary_policy() {
    for alias in ["primary", "default"] {
        let mut editor = Editor::new();
        let _tmp = with_test_dirs(&mut editor);
        editor
            .kb_set_ingest_policy(alias, IngestPolicy::StoreIsTruth)
            .unwrap_or_else(|e| panic!("alias '{alias}' rejected: {e}"));
        assert_eq!(
            editor.kb.registry.primary_ingest_policy,
            IngestPolicy::StoreIsTruth,
            "alias '{alias}' did not reach the primary policy"
        );
    }
}

/// A registry written before this field existed must load as `FromOrgDir`, or
/// the upgrade would silently detach every KB a user has.
#[test]
fn a_registry_without_the_field_defaults_to_ingesting() {
    let toml = r#"
[[instances]]
uuid = "u1"
name = "legacy"
org_dir = "/tmp/legacy"
db_path = "/tmp/legacy.db"
primary = false
enabled = true
"#;
    let reg: mae_kb::federation::KbRegistry = toml::from_str(toml).expect("legacy registry parses");
    assert_eq!(
        reg.instances[0].ingest_policy,
        IngestPolicy::FromOrgDir,
        "a pre-existing registry entry must keep ingesting exactly as before"
    );
    assert_eq!(
        reg.primary_ingest_policy,
        IngestPolicy::FromOrgDir,
        "and so must the primary"
    );
}

// ---------------------------------------------------------------------------
// Phase 5: `source-file` follow mode is retired for a DETACHED KB.
// ---------------------------------------------------------------------------

/// **The trap this closes.** A detached instance's `.org` files are a stale
/// archive, but they still exist and still open — so following a link into one
/// hands the user a document that LOOKS live while being silently disconnected
/// from their KB.
///
/// Worse than a broken link: a broken link is visible.
#[test]
fn a_detached_instance_reports_the_store_as_truth() {
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _tmp = with_test_dirs(&mut editor);
    detached_instance_with_divergent_store(&mut editor, dir.path());

    assert!(
        editor.kb_store_is_truth_for("note-a"),
        "a detached instance's node must report store-is-truth, so link follow \
         stops opening its stale archive"
    );
}

/// The paired positive: an ATTACHED instance is unchanged, so this narrows
/// behaviour and never widens it. Without this, the test above would pass on an
/// implementation that reported store-is-truth for everything -- which would
/// silently disable `source-file` follow mode for every existing user.
#[test]
fn an_attached_instance_is_unaffected() {
    let mut editor = Editor::new();
    let _tmp = with_test_dirs(&mut editor);
    assert!(
        !editor.kb_store_is_truth_for("concept:anything"),
        "an ordinary (attached) KB must keep today's behaviour exactly"
    );
}

/// **The second door into the same trap.** `kb-edit-source` opened a node's
/// `.org` file just as link-follow did — and for a detached KB that is the worst
/// outcome available: the file opens, the edit saves, and no ingest ever reads
/// it, so the work is silently lost while looking successful.
///
/// Closing one door is not closing the trap, which is why both go through the
/// same helper.
///
/// **Updated for ADR-092 D5.** This used to assert a refusal whose message told
/// the user to "edit the node here instead" — advice that was *not true*, because
/// the KB view is read-only and no node edit surface existed. Now it does, so the
/// detached node opens as editable org source text. The invariant this test was
/// filed for is unchanged and still asserted: **the stale archive is not opened.**
#[test]
fn editing_a_detached_kbs_node_opens_the_node_not_its_stale_archive() {
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _tmp = with_test_dirs(&mut editor);
    detached_instance_with_divergent_store(&mut editor, dir.path());

    // Land in a KB view for the detached node, which is what `kb-edit-source`
    // acts on.
    editor.open_help_at("note-a");
    editor.help_edit_source();

    let name = editor.buffers[editor.active_buffer_idx()].name.clone();
    assert_eq!(
        crate::editor::kb_ops::node_buffer::node_id_from_buffer_name(&name).as_deref(),
        Some("note-a"),
        "expected an editable node buffer, got {name:?} (status: {:?})",
        editor.status_msg
    );
    assert!(
        !editor
            .buffers
            .iter()
            .any(|b| b.file_path().is_some_and(|p| p.starts_with(dir.path()))),
        "the stale .org archive must NOT be opened — that is the trap this test \
         was filed for"
    );
}

// -- The primary is resolved two different ways, and one of them is wrong -----

/// **`:kb-detach primary` writes one field and two consumers read another.**
///
/// `KbRegistry::set_ingest_policy` special-cases the primary aliases and writes
/// `registry.primary_ingest_policy`, returning early — it never touches any
/// instance's `ingest_policy`. But `kb_daily_backing` and `kb_store_is_truth_for`
/// both resolved "the primary" as `instances.iter().find(|i| i.primary)`, which
/// `KbInstance`'s own doc calls *"an artifact of registration order"* — the first
/// row ever registered on this machine, not the primary KB, which *"has no
/// `KbInstance` row at all."*
///
/// So detaching the primary left dailies writing `.org` files and
/// `kb-edit-source` opening stale archives, while detaching one unrelated user KB
/// would have flipped both **for the primary**.
#[test]
fn detaching_the_primary_reaches_dailies_and_the_edit_surface() {
    let tmp = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    editor.kb.notes_dir = Some(tmp.path().to_path_buf());

    assert!(
        matches!(editor.kb_daily_backing(), DailyBacking::Files(_)),
        "attached: dailies are file-backed"
    );

    editor
        .kb
        .registry
        .set_ingest_policy("primary", IngestPolicy::StoreIsTruth);

    assert!(
        matches!(editor.kb_daily_backing(), DailyBacking::Store),
        "after :kb-detach primary, dailies must stop writing .org files"
    );

    editor.kb.primary.insert(mae_kb::Node::new(
        "note:primary-owned",
        "Primary owned",
        mae_kb::NodeKind::Note,
        "body",
    ));
    assert!(
        editor.kb_store_is_truth_for("note:primary-owned"),
        "and a primary-owned node's source file must be treated as a stale archive"
    );
}

/// **The cross-talk half.** Detaching an ordinary registered instance must not
/// change how the *primary's* dailies are backed. Before the fix, the first-ever
/// registered row stood in for "the primary", so detaching it silently flipped
/// the primary too.
#[test]
fn detaching_an_unrelated_instance_does_not_flip_the_primarys_dailies() {
    let tmp = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    editor.kb.notes_dir = Some(tmp.path().to_path_buf());

    // The `primary: true` flag means "first row ever registered", nothing more.
    let mut first = KbInstance::local(
        "uuid-first".into(),
        "FirstRegistered".into(),
        tmp.path().join("first"),
        tmp.path().join("first.db"),
    );
    first.primary = true;
    first.kind = KbInstanceKind::UserRegistered;
    editor.kb.registry.instances.push(first);

    editor
        .kb
        .registry
        .set_ingest_policy("FirstRegistered", IngestPolicy::StoreIsTruth);

    assert!(
        matches!(editor.kb_daily_backing(), DailyBacking::Files(_)),
        "the primary is still attached — detaching an unrelated KB must not \
         change how the primary's dailies are backed"
    );
}

// -- The watcher drain: the obvious post-cutover cleanup was the loss path ----

/// Register a real org dir so a live `OrgDirWatcher` is attached and its
/// path->ids mapping is built by the initial ingest — the mapping the `Removed`
/// arm depends on. Returns the instance uuid.
///
/// **Registering for real is not incidental.** A fixture that inserts nodes
/// straight into `kb.instances` leaves the watcher with no mapping, so deleting
/// the file emits nothing and a "the node survived" assertion passes while
/// exercising nothing. That is exactly how the first version of this test
/// passed: `events_removed=0, events_upserted=0`.
fn registered_with_live_watcher(editor: &mut Editor, dir: &std::path::Path) -> String {
    std::fs::write(
        dir.join("note.org"),
        ":PROPERTIES:\n:ID: note-a\n:END:\n#+title: note-a\n\nARCHIVE CONTENT.\n",
    )
    .unwrap();
    let uuid = editor.kb_register("WatchTest", dir).unwrap().uuid;
    editor.kb.watcher_debounce_ms = 0;
    uuid
}

/// Drain until `done` or a wall-clock deadline — never a fixed sleep, whose
/// duration depends on machine load (#769).
fn drain_until(editor: &mut Editor, done: impl Fn(&Editor) -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        editor.drain_kb_watchers();
        if done(editor) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

/// **Deleting a detached KB's stale `.org` archive must not delete its nodes.**
///
/// `drain_kb_watchers` had no `allows_ingest` check anywhere in it, and its
/// `Removed` arm calls `kb_persist_instance_delete` — writing the deletion
/// through to the durable store. So the obvious thing to do right after cutting
/// over ("the archive is stale now, let me clean it up") destroyed the KB.
///
/// The `search.rs` chokepoint comment claimed to cover "buffer save, **the
/// watcher drain**, and anything else"; the drain never called it.
///
/// The **attached** half is the positive control: it proves the delete actually
/// reaches the store, so the detached half cannot pass by the event never
/// arriving.
#[test]
fn deleting_an_archive_deletes_nodes_only_while_attached() {
    // Control: attached — the deletion propagates.
    let attached_dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    let uuid = registered_with_live_watcher(&mut editor, attached_dir.path());
    assert!(
        editor.kb.instances[&uuid].get("note-a").is_some(),
        "sanity: the initial ingest landed"
    );

    std::fs::remove_file(attached_dir.path().join("note.org")).unwrap();
    let propagated = drain_until(&mut editor, |e| {
        e.kb.instances[&uuid].get("note-a").is_none()
    });
    assert!(
        propagated,
        "positive control: on an ATTACHED KB the watcher must remove the node,          or the detached assertion below proves nothing (stats: removed={},          upserted={})",
        editor.kb.watcher_stats.events_removed, editor.kb.watcher_stats.events_upserted
    );

    // The real case: detached — the deletion must not reach the store.
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    let uuid = registered_with_live_watcher(&mut editor, dir.path());
    editor
        .kb
        .registry
        .set_ingest_policy("WatchTest", IngestPolicy::StoreIsTruth);

    std::fs::remove_file(dir.path().join("note.org")).unwrap();
    drain_until(&mut editor, |_| false); // drain for the full window
    assert!(
        editor.kb.instances[&uuid].get("note-a").is_some(),
        "the node must survive its stale archive being deleted — the store is          the source of truth for a detached KB"
    );
}

/// And the other arm: editing the stale archive must not overwrite the store.
#[test]
fn editing_a_detached_archive_does_not_overwrite_the_store() {
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    let uuid = registered_with_live_watcher(&mut editor, dir.path());
    editor
        .kb
        .registry
        .set_ingest_policy("WatchTest", IngestPolicy::StoreIsTruth);

    std::fs::write(
        dir.path().join("note.org"),
        ":PROPERTIES:\n:ID: note-a\n:END:\n#+title: note-a\n\nEDITED STALE ARCHIVE.\n",
    )
    .unwrap();
    drain_until(&mut editor, |_| false);

    // Assert on what the edit DID, not on the body's exact shape — registration
    // stores the file's text including its drawer, which is a separate concern.
    let body = editor.kb.instances[&uuid]
        .get("note-a")
        .map(|n| n.body.clone())
        .expect("the node is still there");
    assert!(
        !body.contains("EDITED STALE ARCHIVE"),
        "an edit to the stale archive must not reach the store: {body:?}"
    );
    assert!(
        body.contains("ARCHIVE CONTENT."),
        "and the original store content must be intact: {body:?}"
    );
}

/// **A system KB cannot be detached, and must say so by name.**
///
/// It has no `KbInstance` and therefore nowhere to record a policy, and its
/// corpus is rebuilt from the binary's embedded sources regardless. Before this
/// guard the call fell through to `Err("No such KB: DevPractices")` —
/// indistinguishable from a typo, and surfaced only as a free-text status line.
///
/// `kb_is_system`'s own doc asks that every lifecycle operation route through it
/// "so a fourth one cannot be added without the check". `kb_set_ingest_policy`
/// was that fourth one.
#[test]
fn detaching_a_system_kb_is_refused_by_name() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    let name = mae_kb::system_kb::SYSTEM_KBS[0].name;
    let err = editor
        .kb_set_ingest_policy(name, IngestPolicy::StoreIsTruth)
        .expect_err("a system KB must not be detachable");

    assert!(
        err.contains(name),
        "the refusal must name the KB, not read as a typo: {err:?}"
    );
    assert!(
        err.contains("system KB"),
        "and must say WHY it is refused: {err:?}"
    );
    assert!(
        !err.contains("No such KB"),
        "the old message was indistinguishable from a mistyped name: {err:?}"
    );
}

/// **`:kb-register` on an already-detached dir must not reset its store.**
///
/// `KbRegistry::register` returns the EXISTING uuid for a dir it already knows
/// (`federation.rs`), so the policy survives — but the adopt tail then ran
/// `import_org_dir_to_store(…, IngestMode::Full)`, the mode whose own doc says it
/// "DELETES nodes whose source file is gone". Every store-only node went with it.
///
/// The same tail is what `drain_kb_registry_watch` calls when another `mae`
/// process touches the registry, so this closes both.
#[test]
fn re_registering_a_detached_dir_does_not_reset_its_store() {
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    std::fs::write(
        dir.path().join("note.org"),
        ":PROPERTIES:\n:ID: note-a\n:END:\n#+title: note-a\n\nARCHIVE CONTENT.\n",
    )
    .unwrap();
    let uuid = editor.kb_register("ReReg", dir.path()).unwrap().uuid;

    // A node that exists only in the store — exactly what a full re-ingest drops.
    let store_only = mae_kb::Node::new(
        "store-only",
        "store-only",
        mae_kb::NodeKind::Note,
        "created in the store, never written to disk",
    );
    editor
        .kb
        .instances
        .get_mut(&uuid)
        .unwrap()
        .insert(store_only.clone());
    if let Some(store) = editor.kb.instance_stores.get(&uuid) {
        store.update_node(&store_only).unwrap();
    }

    // Detach through the real setter, which PERSISTS. An in-memory-only
    // `registry.set_ingest_policy` would be wiped by `kb_register`'s own
    // reload-from-disk before the gate ever saw it.
    editor
        .kb_set_ingest_policy("ReReg", IngestPolicy::StoreIsTruth)
        .expect("detach succeeds");

    // The user re-registers the same directory — the innocuous-looking action.
    editor.kb_register("ReReg", dir.path());

    assert!(
        editor.kb.instances[&uuid].get("store-only").is_some(),
        "a store-only node must survive re-registering a detached dir"
    );
}
