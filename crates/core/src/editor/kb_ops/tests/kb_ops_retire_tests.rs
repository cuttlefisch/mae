//! `:kb-retire-archive` — the step that finishes a cutover.
//!
//! The gate is the whole point: moving a file the store does not represent
//! destroys the only copy. These tests are written against the specific ways
//! that can be true, not against a generic "it works" idea.

use super::*;
use mae_kb::federation::{IngestPolicy, KbInstance};
use mae_kb::{CozoKbStore, KbStore, Node, NodeKind};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn hash_of(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// A detached KB with a real store. `imported` are written to disk AND
/// recorded with a matching hash and a real node, i.e. genuinely represented.
fn detached_kb(dir: &std::path::Path, imported: &[(&str, &str)]) -> (Editor, TempDir) {
    let mut editor = Editor::new();
    // Persist the instance the way a real one is: `KbRegistry::update` reloads
    // from disk, so an in-memory-only fixture vanishes the moment retirement
    // saves. That is not a quirk to work around — it is how the registry works.
    let dirs = with_test_dirs(&mut editor);
    let mut inst = KbInstance::local(
        "uuid-retire".into(),
        "Retiring".into(),
        dir.to_path_buf(),
        dir.join("kb.sqlite"),
    );
    inst.ingest_policy = IngestPolicy::StoreIsTruth;

    let store = CozoKbStore::open_mem().unwrap();
    for (rel, content) in imported {
        // Build the path COMPONENT-WISE. `dir.join("sub/b.org")` embeds a
        // forward slash on Windows, producing `…\\sub/b.org`, while the gate's
        // `read_dir` walk yields `…\\sub\\b.org` — and the `source_files` key is
        // a string compare, so the lookup misses and the file looks
        // never-imported. Real ingest walks the directory too, so both sides
        // are OS-native there; only a hand-built test path can diverge.
        let p = rel
            .split('/')
            .fold(dir.to_path_buf(), |acc, part| acc.join(part));
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        let id = format!("note:{rel}");
        store
            .insert_node(&Node::new(&id, *rel, NodeKind::Note, *content))
            .unwrap();
        store
            .record_source_file(&p.to_string_lossy(), &hash_of(content), 0, &[id])
            .unwrap();
    }
    editor
        .kb
        .instance_stores
        .insert(inst.uuid.clone(), Arc::new(store));
    editor.kb.registry.instances.push(inst);
    let data_dir = editor.mae_data_dir().expect("test data dir");
    std::fs::create_dir_all(&data_dir).unwrap();
    editor
        .kb
        .registry
        .save(&data_dir)
        .expect("persist registry");
    (editor, dirs)
}

/// The happy path, and what "native" means afterwards: the files are gone from
/// the origin, present in the holding dir, and `org_dir` is cleared — which is
/// what makes every read-only guard go quiet without a separate state flag.
#[test]
fn retiring_a_verified_archive_moves_the_files_and_makes_the_kb_native() {
    let tmp = TempDir::new().unwrap();
    let (mut editor, _dirs) = detached_kb(tmp.path(), &[("a.org", "AAA"), ("sub/b.org", "BBB")]);

    let plan = editor.kb_retire_plan("Retiring").expect("plan");
    assert!(plan.is_clean(), "{}", plan.describe());
    assert_eq!(plan.files.len(), 2);

    let msg = editor.kb_retire_archive("Retiring").expect("retire");

    assert!(
        !tmp.path().join("a.org").exists(),
        "origin file must be gone"
    );
    assert!(
        !tmp.path().join("sub").join("b.org").exists(),
        "nested origin file must be gone"
    );
    let inst = editor.kb.registry.find("Retiring").unwrap();
    assert!(
        inst.org_dir.as_os_str().is_empty(),
        "org_dir must be cleared — that is what makes the KB native"
    );
    let rec = inst.import_record.as_ref().expect("import record");
    assert!(rec.retired_at.is_some(), "retirement must be recorded");
    let dest = rec.retired_to.clone().expect("holding dir recorded");
    assert_eq!(
        std::fs::read_to_string(dest.join("a.org")).unwrap(),
        "AAA",
        "the archive must be recoverable from the holding dir"
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("sub").join("b.org")).unwrap(),
        "BBB"
    );
    assert!(msg.contains("native"), "{msg}");
}

/// **The blocker that matters most.** A file with no `:ID:` is skipped by
/// ingest BEFORE `record_source_file`, so it is absent from `source_files` —
/// exactly how a whole daily note sat invisible in a real primary KB. Moving
/// it would destroy the only copy.
#[test]
fn a_file_the_store_never_imported_blocks_retirement_and_nothing_moves() {
    let tmp = TempDir::new().unwrap();
    let (mut editor, _dirs) = detached_kb(tmp.path(), &[("imported.org", "IN THE STORE")]);
    let orphan = tmp.path().join("never-imported.org");
    std::fs::write(&orphan, "ONLY COPY OF THIS").unwrap();

    let plan = editor.kb_retire_plan("Retiring").unwrap();
    assert!(!plan.is_clean());
    assert!(
        plan.blockers.iter().any(|b| b.path == orphan),
        "the un-imported file must be a blocker"
    );

    let err = editor
        .kb_retire_archive("Retiring")
        .expect_err("must refuse while anything is unrepresented");
    assert!(err.contains("never imported"), "{err}");

    // Refusing is not enough — it must be ALL-or-nothing.
    assert!(
        orphan.exists() && tmp.path().join("imported.org").exists(),
        "a refused retirement must move NOTHING, not just skip the blocker"
    );
}

/// A file edited after the KB was detached never reached the store, so its
/// content exists only on disk. Moving it would lose that edit.
#[test]
fn a_file_modified_since_import_blocks_retirement() {
    let tmp = TempDir::new().unwrap();
    let (mut editor, _dirs) = detached_kb(tmp.path(), &[("note.org", "AS IMPORTED")]);
    std::fs::write(tmp.path().join("note.org"), "EDITED AFTER DETACHING").unwrap();

    let plan = editor.kb_retire_plan("Retiring").unwrap();
    assert!(
        plan.blockers.iter().any(|b| b.reason.contains("modified")),
        "{}",
        plan.describe()
    );
    assert!(editor.kb_retire_archive("Retiring").is_err());
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("note.org")).unwrap(),
        "EDITED AFTER DETACHING",
        "the edit must survive the refusal"
    );
}

/// An attached KB is not retirable — its org dir is still the source of truth.
#[test]
fn an_attached_kb_cannot_be_retired() {
    let tmp = TempDir::new().unwrap();
    let (mut editor, _dirs) = detached_kb(tmp.path(), &[("a.org", "AAA")]);
    editor
        .kb
        .registry
        .set_ingest_policy("Retiring", IngestPolicy::FromOrgDir);

    let err = editor.kb_retire_plan("Retiring").expect_err("must refuse");
    assert!(err.contains("still attached"), "{err}");
}
