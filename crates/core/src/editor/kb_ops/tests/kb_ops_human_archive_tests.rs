//! The HUMAN paths into a detached KB's stale archive.
//!
//! The agent paths were guarded first; every human path reached the same
//! primitives with no check, so the human got exactly the silent data loss the
//! agent was protected from. These pin the two that lose work — `:w` and
//! autosave — and the read path that now opens read-only instead.

use super::*;
use mae_kb::federation::{IngestPolicy, KbInstance, KbInstanceKind};
use mae_kb::CozoKbStore;
use std::sync::Arc;

/// A detached KB whose store records `imported` as a source file.
///
/// Recording is load-bearing, not decoration: the guard only claims a file the
/// KB actually imported, because a KB's `org_dir` is routinely a whole project
/// repo. A fixture that skips it models a KB that imported nothing, and every
/// refusal assertion would pass vacuously.
fn editor_with_archive(dir: &std::path::Path, imported: &[&std::path::Path]) -> Editor {
    let mut editor = Editor::new();
    let mut inst = KbInstance::local(
        "uuid-archive".into(),
        "Archived".into(),
        dir.to_path_buf(),
        dir.join("kb.sqlite"),
    );
    inst.kind = KbInstanceKind::UserRegistered;
    inst.ingest_policy = IngestPolicy::StoreIsTruth;

    let store = CozoKbStore::open_mem().expect("in-memory store");
    for p in imported {
        store
            .record_source_file(&p.to_string_lossy(), "hash-for-test", 0, &[])
            .expect("record source file");
    }
    editor
        .kb
        .instance_stores
        .insert(inst.uuid.clone(), Arc::new(store));
    editor.kb.registry.instances.push(inst);
    editor
}

/// **`:w` used to succeed and lose the edit.** The bytes reached disk, and
/// `kb_reimport_file` correctly skips a detached instance — so the store never
/// saw them and the status line said `"written"`.
///
/// The load-bearing assertion is the FILE CONTENT, not the message: a refusal
/// that still wrote would satisfy a message-only oracle while doing the exact
/// damage this guards.
#[test]
fn saving_into_a_detached_kbs_archive_is_refused_and_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let note = tmp.path().join("note.org");
    std::fs::write(&note, "ORIGINAL").unwrap();
    let mut editor = editor_with_archive(tmp.path(), &[&note]);

    let idx = editor.open_file_hidden(&note).expect("archive still opens");
    editor.buffers[idx].replace_contents("EDITED — MUST NOT REACH DISK");
    editor.buffers[idx].modified = true;
    editor.display_buffer(idx);

    editor.save_current_buffer();

    assert_eq!(
        std::fs::read_to_string(&note).unwrap(),
        "ORIGINAL",
        "the edit was WRITTEN to a file nothing reads — this is the data loss"
    );
    assert!(
        editor.status_msg.contains("stale archive"),
        "and the user must be told why: {}",
        editor.status_msg
    );
}

/// The same refusal must hold for autosave, which is worse: unattended, on a
/// timer, with no user action to suspect.
#[test]
fn autosave_refuses_the_archive_too() {
    let tmp = TempDir::new().unwrap();
    let note = tmp.path().join("note.org");
    std::fs::write(&note, "ORIGINAL").unwrap();
    let mut editor = editor_with_archive(tmp.path(), &[&note]);

    let idx = editor.open_file_hidden(&note).unwrap();
    editor.buffers[idx].replace_contents("AUTOSAVED — MUST NOT REACH DISK");
    editor.buffers[idx].modified = true;

    let (saved, errors) = editor.save_all_modified_buffers();

    assert_eq!(
        std::fs::read_to_string(&note).unwrap(),
        "ORIGINAL",
        "autosave silently wrote into the archive"
    );
    assert_eq!(saved, 0, "nothing should have been saved");
    assert!(
        errors.iter().any(|e| e.contains("stale archive")),
        "the refusal must be reported, not swallowed: {errors:?}"
    );
}

/// The archive stays READABLE — it is still the only copy of what the store
/// lost at ingest — but read-only, so an edit cannot be stranded.
#[test]
fn a_detached_kbs_archive_opens_read_only() {
    let tmp = TempDir::new().unwrap();
    let note = tmp.path().join("note.org");
    std::fs::write(&note, "ARCHIVED CONTENT").unwrap();
    let mut editor = editor_with_archive(tmp.path(), &[&note]);

    let idx = editor.open_file_hidden(&note).expect("must still open");
    assert!(
        editor.buffers[idx].text().contains("ARCHIVED CONTENT"),
        "the archive must remain readable"
    );
    assert!(
        editor.buffers[idx].read_only,
        "but not editable — a stranded edit is the failure this prevents"
    );
    assert!(
        editor.status_msg.contains("NOT the KB"),
        "and the user must be told what they are looking at: {}",
        editor.status_msg
    );
}

/// The paired negative, and the regression that shipped once already: an
/// ordinary project file beside the imported one is NOT the KB's source. A
/// KB's `org_dir` is often a whole repo, so claiming everything under it made
/// Terraform and ansible files unopenable.
#[test]
fn an_ordinary_project_file_in_the_same_repo_is_untouched() {
    let tmp = TempDir::new().unwrap();
    let note = tmp.path().join("note.org");
    let cfg = tmp.path().join("ansible.cfg");
    std::fs::write(&note, "KB SOURCE").unwrap();
    std::fs::write(&cfg, "ORIGINAL CFG").unwrap();
    let mut editor = editor_with_archive(tmp.path(), &[&note]);

    let idx = editor.open_file_hidden(&cfg).expect("must open normally");
    assert!(
        !editor.buffers[idx].read_only,
        "an ordinary project file must stay editable"
    );

    editor.buffers[idx].replace_contents("EDITED CFG");
    editor.buffers[idx].modified = true;
    editor.display_buffer(idx);
    editor.save_current_buffer();

    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        "EDITED CFG",
        "and must still be savable — otherwise the guard has eaten the repo"
    );
}

/// A NEW `.org` file in a detached KB's directory is the create-side twin: it
/// looks exactly like adding a note and would never reach the KB. The
/// source-files check cannot catch it — a brand-new file was never imported —
/// which is why this is a separate rule.
#[test]
fn creating_a_new_org_file_in_a_detached_kbs_dir_is_refused() {
    let tmp = TempDir::new().unwrap();
    let editor = editor_with_archive(tmp.path(), &[]);

    let msg = editor
        .kb_orphan_org_target(&tmp.path().join("brand-new-note.org"))
        .expect("a new .org here would be invisible to the KB");
    assert!(msg.contains("kb-create"), "must redirect: {msg}");
    assert!(
        msg.contains("invisible"),
        "must name the consequence: {msg}"
    );
}

/// And the control that keeps the rule from eating the repo: an ordinary new
/// project file in the same directory is none of the KB's business. This is
/// the case that was broken once already by keying on the directory alone.
#[test]
fn creating_an_ordinary_new_file_in_the_same_dir_is_allowed() {
    let tmp = TempDir::new().unwrap();
    let editor = editor_with_archive(tmp.path(), &[]);

    for name in ["main.tf", "playbook.yml", "README.md", "notes.txt"] {
        assert!(
            editor
                .kb_orphan_org_target(&tmp.path().join(name))
                .is_none(),
            "{name} is an ordinary project file and must be creatable"
        );
    }
}

/// An attached KB's directory is untouched — a new `.org` there WILL be
/// ingested, which is the whole point of an attached KB.
#[test]
fn creating_a_new_org_file_in_an_attached_kbs_dir_is_allowed() {
    let tmp = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let mut inst = KbInstance::local(
        "uuid-attached".into(),
        "Attached".into(),
        tmp.path().to_path_buf(),
        tmp.path().join("kb.sqlite"),
    );
    inst.ingest_policy = IngestPolicy::FromOrgDir;
    editor.kb.registry.instances.push(inst);

    assert!(
        editor
            .kb_orphan_org_target(&tmp.path().join("new-note.org"))
            .is_none(),
        "an attached KB still ingests new .org files"
    );
}
