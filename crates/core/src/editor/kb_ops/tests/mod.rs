// Test modules split from monolithic kb_ops_tests.rs (2,987 lines, ~98 tests).

pub(crate) use super::*;
pub(crate) use tempfile::TempDir;

mod kb_ops_activity_tests;
mod kb_ops_collab_sync_tests;
mod kb_ops_concurrency_tests;
mod kb_ops_crud_tests;
mod kb_ops_daemon_tests;
mod kb_ops_durability_tests;
mod kb_ops_registry_tests;
mod kb_ops_search_federation_tests;
mod kb_ops_watcher_misc_tests;

// Shared test helpers used across multiple test modules

pub(crate) fn create_test_org_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    // File with :ID:
    std::fs::write(
        dir.path().join("note1.org"),
        ":PROPERTIES:\n:ID: test-note-1\n:END:\n#+title: Note One\n\nBody of note one.\n",
    )
    .unwrap();
    // File with :ID: in subdir
    let sub = dir.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(
            sub.join("note2.org"),
            ":PROPERTIES:\n:ID: test-note-2\n:END:\n#+title: Note Two\n\nLinks to [[id:test-note-1][Note One]].\n",
        )
        .unwrap();
    // File without :ID: (should be skipped)
    std::fs::write(
        dir.path().join("no-id.org"),
        "#+title: No ID\n\nJust a note without an ID property.\n",
    )
    .unwrap();
    dir
}

/// Set config/data dir overrides to a tempdir so tests never touch
/// real user directories (~/.config/mae, ~/.local/share/mae).
pub(crate) fn with_test_dirs(editor: &mut Editor) -> TempDir {
    let tmp = TempDir::new().unwrap();
    editor.config_dir_override = Some(tmp.path().join("config"));
    editor.data_dir_override = Some(tmp.path().join("data"));
    tmp
}
