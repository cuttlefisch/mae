//! Story D / R8 — the import audit's operator-facing surfaces.
//!
//! Every test here targets the failure mode obsidian-importer#547 demonstrated:
//! an importer whose own counters read clean through a ~10% loss. So the
//! assertions are about what the SOURCE and the DESTINATION say, never about
//! what the importer reported about itself.

use super::*;

/// The report is a durable artifact **in the destination**, not a toast — so it
/// must be there afterwards, reachable the same way any other node is.
#[test]
fn a_reimport_writes_the_loss_report_into_the_destination() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("AuditNotes", dir.path()).unwrap();

    editor.kb_reimport("AuditNotes", None).unwrap();

    let node = editor.kb.instances[&reg.uuid]
        .get(mae_kb::import_plan::LOSS_REPORT_ID)
        .expect("the loss report must live in the destination KB");
    assert!(
        node.body.contains("source file(s)"),
        "the census line leads, because it is the number the importer does NOT \
         self-report: {}",
        node.body
    );
}

/// **"Nothing was lost" must be distinguishable from "nothing was checked".**
/// A clean import still files a report, and that report still names the file it
/// deliberately skipped — the ID-less one in the fixture.
#[test]
fn a_clean_reimport_still_files_a_report_that_explains_its_skip() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("AuditNotes", dir.path()).unwrap();

    editor.kb_reimport("AuditNotes", None).unwrap();
    let body = editor.kb.instances[&reg.uuid]
        .get(mae_kb::import_plan::LOSS_REPORT_ID)
        .unwrap()
        .body
        .clone();

    assert!(body.contains("0 unaccounted"), "{body}");
    assert!(body.contains("no-id.org"), "the skip must be NAMED: {body}");
    assert!(
        body.contains(":ID:"),
        "and explained, so it is not read as loss: {body}"
    );
}

/// **The binding contract.** A plan the operator read and approved is the file
/// set that gets imported, or the import does not run. Without this the preview
/// is scoping, and every other notes importer's "dry run" is exactly that.
#[test]
fn an_import_is_refused_when_the_corpus_moved_since_the_saved_plan() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("AuditNotes", dir.path()).unwrap();

    editor
        .kb_import_plan(dir.path().to_str().unwrap())
        .expect("planning a real directory must succeed");

    // The corpus moves after the operator approved the plan.
    std::fs::write(
        dir.path().join("surprise.org"),
        ":PROPERTIES:\n:ID: surprise\n:END:\n#+title: Surprise\n\nunseen\n",
    )
    .unwrap();

    let refused = editor.kb_reimport("AuditNotes", None);

    assert!(
        refused.is_none(),
        "the import must refuse rather than silently import a file the operator \
         never saw in the preview"
    );
    assert!(
        editor.status_msg.contains("import plan"),
        "and say why: {:?}",
        editor.status_msg
    );
}

/// The gate binds an operator who ASKED for a plan. Inventing it for one who did
/// not would break every existing import, which is the kind of overreach that
/// gets a safety mechanism disabled wholesale.
#[test]
fn with_no_saved_plan_the_import_proceeds_exactly_as_before() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("AuditNotes", dir.path()).unwrap();

    std::fs::write(
        dir.path().join("note3.org"),
        ":PROPERTIES:\n:ID: test-note-3\n:END:\n#+title: Three\n\nnew\n",
    )
    .unwrap();

    let result = editor
        .kb_reimport("AuditNotes", None)
        .expect("no plan means no gate");
    assert!(result.report.nodes_imported + result.report.nodes_updated >= 3);
}

/// The reconciliation pass reads both sides and reports the DESTINATION's
/// answer — so removing a node from the store must surface as loss even though
/// the import that put it there reported success at the time.
#[test]
fn verify_reports_a_node_the_destination_no_longer_holds() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("AuditNotes", dir.path()).unwrap();

    // Give the instance a real store, because the store is the side that
    // survives a restart and `kb_known_ids` must prefer it. Without one the
    // store-precedence branch is never executed and this test is vacuous —
    // which is what falsifying it revealed on the first attempt.
    let store = std::sync::Arc::new(mae_kb::CozoKbStore::open_mem().unwrap());
    store.seed_type_system().unwrap();
    for id in ["test-note-1", "test-note-2"] {
        let node = editor.kb.instances[&reg.uuid].get(id).unwrap().clone();
        store.update_node(&node).unwrap();
    }
    editor
        .kb
        .instance_stores
        .insert(reg.uuid.clone(), store.clone());

    let clean = editor.kb_import_verify("AuditNotes").unwrap();
    assert!(
        clean.contains("0 source-only") && clean.contains("0 error"),
        "a freshly-imported KB must reconcile clean: {clean}"
    );

    // Delete from the DESTINATION, which post-cutover means the store when the
    // instance has one — `kb_known_ids` reads the store in preference to the
    // in-memory mirror, and that precedence is the point: the mirror is a
    // rendering, the store is what survives a restart.
    store.delete_node("test-note-1").unwrap();

    let dirty = editor.kb_import_verify("AuditNotes").unwrap();
    assert!(
        dirty.contains("source-only: ") && dirty.contains("note1.org"),
        "the missing node's FILE must be named, so the operator can act: {dirty}"
    );
    assert!(
        editor.kb.instances[&reg.uuid].get("test-note-1").is_some(),
        "the in-memory mirror still holds it — so this result can ONLY have come \
         from reading the store, which is the precedence being pinned"
    );
}

/// A pre-flight that cannot be re-read is not an assessment, it is a print
/// statement. The persisted plan is also the artifact the binding check consumes.
#[test]
fn the_pre_flight_plan_is_persisted_where_the_import_will_look_for_it() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);

    let msg = editor.kb_import_plan(dir.path().to_str().unwrap()).unwrap();
    assert!(msg.contains("2 node(s)"), "{msg}");

    let path = editor.kb_import_plan_path(dir.path()).unwrap();
    assert!(
        path.exists(),
        "plan must be persisted at {}",
        path.display()
    );
    assert!(
        mae_kb::import_plan::ImportPlan::load(&path).is_ok(),
        "and be re-readable by the import that follows"
    );
}

/// **The audit commands were actively misleading on a detached KB.** Neither
/// checked ingest policy, so pointed at a frozen archive they described an
/// import that can never happen, and reported the divergence detaching created
/// as though it were loss.
#[test]
fn import_plan_refuses_a_detached_kbs_archive() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.org"), ":PROPERTIES:\n:ID: x\n:END:\n").unwrap();
    let mut editor = Editor::new();
    let mut inst = mae_kb::federation::KbInstance::local(
        "uuid-detached".into(),
        "Detached".into(),
        tmp.path().to_path_buf(),
        tmp.path().join("kb.sqlite"),
    );
    inst.ingest_policy = mae_kb::federation::IngestPolicy::StoreIsTruth;
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store
        .record_source_file(&tmp.path().join("a.org").to_string_lossy(), "hash", 0, &[])
        .unwrap();
    editor
        .kb
        .instance_stores
        .insert(inst.uuid.clone(), std::sync::Arc::new(store));
    editor.kb.registry.instances.push(inst);

    let err = editor
        .kb_import_plan(&tmp.path().to_string_lossy())
        .expect_err("must refuse a frozen archive");
    assert!(err.contains("kb-retire-archive"), "must redirect: {err}");

    let err = editor
        .kb_import_verify("Detached")
        .expect_err("must refuse to reconcile against a frozen archive");
    assert!(err.contains("expected post-detach divergence"), "{err}");
}

/// The control: an ATTACHED KB's directory is exactly what these commands are
/// for, and must still work.
#[test]
fn the_audit_commands_still_work_on_an_attached_kb() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.org"), ":PROPERTIES:\n:ID: x\n:END:\n").unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    let mut inst = mae_kb::federation::KbInstance::local(
        "uuid-attached".into(),
        "Attached".into(),
        tmp.path().to_path_buf(),
        tmp.path().join("kb.sqlite"),
    );
    inst.ingest_policy = mae_kb::federation::IngestPolicy::FromOrgDir;
    editor.kb.registry.instances.push(inst);

    assert!(editor.kb_import_plan(&tmp.path().to_string_lossy()).is_ok());
    assert!(editor.kb_import_verify("Attached").is_ok());
}
