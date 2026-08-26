//! Instance path-matching tests.
//!
//! Split out of `kb_ops_registry_tests.rs` (which is about registry
//! lifecycle and data-dir resolution) to keep that file under its size
//! ceiling and because "does this path belong to this instance" is a
//! distinct concern with its own failure modes.

use super::*;

/// A registry instance with an EMPTY `org_dir` must not claim every path on the
/// filesystem.
///
/// `Path::starts_with("")` is `true` — an empty component iterator is a prefix
/// of everything — so an unguarded `path.starts_with(&inst.org_dir)` matches
/// unconditionally once any dir-less instance is registered. Dir-less instances
/// are not hypothetical: the bundled guidance KBs (MaePractices, DevPractices)
/// and joined collab KBs are registered exactly that way by design, and they
/// sort ahead of user-registered KBs in the live registry.
///
/// Two consequences this pins:
///   1. `kb_path_in_instance` returns `true` for arbitrary paths, so the save
///      hook fires the KB reimport + activity-tracking branch on every buffer
///      save of any file anywhere.
///   2. `kb_reimport_file` iterates in registry order and returns on first
///      match, so a dir-less instance ordered ahead of a real one SHADOWS it.
///
/// The ordering below is deliberately hostile — dir-less FIRST — because that
/// is the live layout, and because a test putting the real instance first would
/// pass without the fix.
#[test]
fn an_empty_org_dir_instance_does_not_claim_every_path() {
    fn inst(uuid: &str, dir: std::path::PathBuf) -> mae_kb::federation::KbInstance {
        mae_kb::federation::KbInstance {
            uuid: uuid.into(),
            name: uuid.into(),
            org_dir: dir,
            db_path: std::path::PathBuf::new(),
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
            kind: mae_kb::federation::KbInstanceKind::default(),
            ingest_policy: Default::default(),
            priority: 0,
            remote_hub: None,
        }
    }

    let real = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();

    let mut editor = Editor::new();
    editor
        .kb
        .registry
        .instances
        .push(inst("dirless", std::path::PathBuf::new()));
    editor
        .kb
        .registry
        .instances
        .push(inst("real", real.path().to_path_buf()));

    // Precondition, asserted explicitly: if this ever stops holding, the bug
    // guarded here has changed shape and the test needs rewriting, not deleting.
    assert!(
        std::path::Path::new("/any/path").starts_with(std::path::Path::new("")),
        "precondition: Path::starts_with(\"\") is true"
    );

    let foreign = outside.path().join("notes.org");
    assert!(
        !editor.kb_path_in_instance(&foreign),
        "a path under NO registered instance must not be claimed just because a \
         dir-less instance is registered: {}",
        foreign.display()
    );

    // Positive control: the guard must not pass by rejecting everything.
    let inside = real.path().join("notes.org");
    assert!(
        editor.kb_path_in_instance(&inside),
        "a path genuinely under a registered instance must still match: {}",
        inside.display()
    );
}
