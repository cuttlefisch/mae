//! When the KB registry cannot be updated, the editor says so and changes
//! nothing — it does not apply the change in memory and report success.
//!
//! `KbRegistry::update` refuses (nothing saved, nothing mutated) when the file
//! cannot be read or another process holds its lock (federation_durability
//! tests). These pin the other half: every editor operation built on it
//! surfaces the refusal and leaves its own state as it was, because a change
//! that exists only in memory looks done and silently reverts on restart.
//! Exercised with an unreadable registry, the failure a test can cause
//! deterministically; lock contention reaches callers through the same `Err`.

use super::*;

/// An editor with one native KB `a`, then its registry file made unreadable.
/// Returns the editor, the guard for its dirs, and the registry path + bytes.
fn editor_with_broken_registry() -> (Editor, TempDir, std::path::PathBuf, Vec<u8>) {
    let mut editor = Editor::new();
    let dirs = with_test_dirs(&mut editor);
    editor.kb_new("a").expect("fixture: native KB");
    let path = editor
        .mae_data_dir()
        .expect("data dir")
        .join("kb-registry.toml");
    assert!(path.exists(), "fixture: the registry was saved");
    let broken = b"[[instances]]\nname = \"a\"\nthis is not toml".to_vec();
    std::fs::write(&path, &broken).unwrap();
    (editor, dirs, path, broken)
}

fn names(editor: &Editor) -> Vec<String> {
    let mut v: Vec<String> = editor
        .kb
        .registry
        .instances
        .iter()
        .map(|i| i.name.clone())
        .collect();
    v.sort();
    v
}

#[test]
fn kb_new_fails_and_reports_why_when_the_registry_cannot_be_read() {
    let (mut editor, _dirs, path, broken) = editor_with_broken_registry();
    let before = names(&editor);
    let r = editor.kb_new("b");
    let err = r.expect_err("kb_new must fail, not report 'Created'");
    assert!(
        err.contains("could not be read"),
        "the error must say why: {err}"
    );
    assert_eq!(
        names(&editor),
        before,
        "the KB was registered in memory only"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        broken,
        "the registry file was rewritten"
    );
}

#[test]
fn kb_unregister_keeps_the_kb_when_the_registry_cannot_be_saved() {
    let (mut editor, _dirs, path, broken) = editor_with_broken_registry();
    let uuid = editor.kb.registry.find("a").unwrap().uuid.clone();
    editor.kb_unregister("a");
    assert!(
        editor.kb.registry.find("a").is_some(),
        "the KB left the in-memory registry though the file still lists it"
    );
    assert!(
        editor.kb.instance_stores.contains_key(&uuid),
        "the KB's store was dropped though the unregister failed"
    );
    assert!(
        editor.status_msg.contains("failed"),
        "the failure must be reported: {}",
        editor.status_msg
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        broken,
        "the registry file was rewritten"
    );
}

/// Residency bounds what an agent may read (ADR-048); a change that only
/// exists in memory reverts on restart while the user believes it applied.
#[test]
fn kb_set_ai_residency_fails_when_the_registry_cannot_be_saved() {
    let (mut editor, _dirs, path, broken) = editor_with_broken_registry();
    let before = editor.kb.registry.find("a").unwrap().ai_residency;
    let r = editor.kb_set_ai_residency("a", mae_kb::federation::AiResidency::LocalModelsOnly);
    assert!(r.is_err(), "the residency change must fail, got {r:?}");
    assert_eq!(
        editor.kb.registry.find("a").unwrap().ai_residency,
        before,
        "the residency changed in memory only"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        broken,
        "the registry file was rewritten"
    );
}

/// `:kb-detach` on an ATTACHED KB is the path that really writes the ingest
/// policy (an attach of a native KB is refused before the registry is touched,
/// which would make this test vacuous).
#[test]
fn kb_detach_fails_when_the_registry_cannot_be_saved() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    let org = TempDir::new().unwrap();
    std::fs::write(
        org.path().join("n.org"),
        ":PROPERTIES:\n:ID: n:1\n:END:\n#+title: N\n\nBody.\n",
    )
    .unwrap();
    editor
        .kb_register("att", org.path())
        .expect("fixture: attached KB");
    let before = editor.kb.registry.find("att").unwrap().ingest_policy;
    assert_eq!(
        before,
        mae_kb::federation::IngestPolicy::FromOrgDir,
        "fixture: the KB is attached"
    );
    let path = editor.mae_data_dir().unwrap().join("kb-registry.toml");
    let broken = b"this is not toml [".to_vec();
    std::fs::write(&path, &broken).unwrap();

    editor.dispatch_ex_command("kb-detach", Some("att"));
    assert_eq!(
        editor.kb.registry.find("att").unwrap().ingest_policy,
        before,
        "the ingest policy changed in memory only"
    );
    assert!(
        editor.status_msg.contains("could not be read"),
        "the failure must be reported: {}",
        editor.status_msg
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        broken,
        "the registry file was rewritten"
    );
}

/// Real contention at the caller: another (older-version) MAE process holds the
/// registry's lock. `kb_new` must fail with a message saying so and change
/// nothing — before, it wrote the registry without the lock after ~600 ms.
#[cfg(unix)]
#[test]
fn kb_new_fails_while_another_process_holds_the_registry_lock() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    editor.kb_new("a").expect("fixture: native KB");
    let path = editor.mae_data_dir().unwrap().join("kb-registry.toml");
    let before = std::fs::read(&path).unwrap();
    let holder = mae_mcp::file_lock::LockInfo {
        pid: std::os::unix::process::parent_id(),
        ..mae_mcp::file_lock::LockInfo::current()
    };
    std::fs::write(
        mae_mcp::file_lock::lock_path(&path),
        serde_json::to_string(&holder).unwrap(),
    )
    .unwrap();
    let err = editor
        .kb_new("b")
        .expect_err("kb_new must fail under contention");
    assert!(err.contains("locked by another MAE process"), "{err}");
    assert!(
        editor.kb.registry.find("b").is_none(),
        "registered in memory only"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the registry was written"
    );
}
