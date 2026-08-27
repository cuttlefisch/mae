//! Creating a KB that never had an org directory.
//!
//! Before this, `kb_register` hard-required an existing directory and the only
//! other creation path was joining a peer — so "start native and never see an
//! org directory" was not possible at all.

use super::*;
use mae_kb::federation::IngestPolicy;

/// A native KB is `StoreIsTruth` with an EMPTY `org_dir` — the third state, and
/// what makes every archive guard stay quiet for it.
#[test]
fn kb_new_creates_a_store_only_instance() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    let msg = editor.kb_new("Fresh").expect("native creation must work");
    assert!(msg.contains("no org directory"), "{msg}");

    let inst = editor.kb.registry.find("Fresh").expect("registered");
    assert!(
        inst.org_dir.as_os_str().is_empty(),
        "a native KB has no org directory"
    );
    assert_eq!(
        inst.ingest_policy,
        IngestPolicy::StoreIsTruth,
        "native means the store is truth — not a migration waiting to happen"
    );
}

/// **The blocker this had to fix first.** `register`'s duplicate check matches
/// on `org_dir`, and every native KB's is empty — as is every RETIRED KB's,
/// since retirement clears it. Reusing that check would have made the second
/// native KB silently return the first one's uuid.
#[test]
fn two_native_kbs_are_distinct_not_silently_the_same() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    editor.kb_new("First").expect("first");
    editor.kb_new("Second").expect("second");

    let a = editor
        .kb
        .registry
        .find("First")
        .expect("First exists")
        .uuid
        .clone();
    let b = editor
        .kb
        .registry
        .find("Second")
        .expect("Second exists")
        .uuid
        .clone();
    assert_ne!(
        a, b,
        "two native KBs collapsed into one — the org_dir duplicate check matched \
         empty against empty"
    );
    assert_eq!(
        editor
            .kb
            .registry
            .instances
            .iter()
            .filter(|i| i.org_dir.as_os_str().is_empty())
            .count(),
        2,
        "both must exist as separate rows"
    );
}

/// A native KB must not silently adopt a name that belongs to a file-backed
/// one — "the same KB" would be a different claim.
#[test]
fn kb_new_refuses_a_name_already_backed_by_a_directory() {
    let tmp = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    let mut inst = mae_kb::federation::KbInstance::local(
        "uuid-existing".into(),
        "Taken".into(),
        tmp.path().to_path_buf(),
        tmp.path().join("kb.sqlite"),
    );
    inst.ingest_policy = IngestPolicy::FromOrgDir;
    editor.kb.registry.instances.push(inst);
    let data_dir = editor.mae_data_dir().unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();
    editor.kb.registry.save(&data_dir).unwrap();

    let err = editor.kb_new("Taken").expect_err("must refuse");
    assert!(err.contains("already exists"), "{err}");
}

/// Reserved system-KB names are refused here too — the check must not be
/// something only `kb_register` happens to do.
#[test]
fn kb_new_refuses_a_reserved_system_name() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    let reserved = mae_kb::system_kb::SYSTEM_KBS[0].name;
    let err = editor
        .kb_new(reserved)
        .expect_err("must refuse a reserved name");
    assert!(err.contains("reserved"), "{err}");
}
