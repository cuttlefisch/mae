//! Project root detection and management tests.

use super::*;
use std::fs;

#[test]
fn active_project_root_falls_back_to_editor_project() {
    let mut editor = Editor::new();
    // No project set anywhere
    assert!(editor.active_project_root().is_none());

    // Set editor-wide project
    editor.project = Some(crate::project::Project::from_root(
        std::path::PathBuf::from("/tmp"),
    ));
    assert_eq!(
        editor.active_project_root().unwrap(),
        std::path::Path::new("/tmp")
    );
}

#[test]
fn active_project_root_prefers_buffer_project() {
    let mut editor = Editor::new();
    editor.project = Some(crate::project::Project::from_root(
        std::path::PathBuf::from("/editor-wide"),
    ));
    editor.buffers[0].project_root = Some(std::path::PathBuf::from("/buffer-specific"));
    assert_eq!(
        editor.active_project_root().unwrap(),
        std::path::Path::new("/buffer-specific")
    );
}

#[test]
fn set_project_root_command() {
    let mut editor = Editor::new();
    // Valid directory
    editor.execute_command("set-project-root /tmp");
    assert_eq!(
        editor.buffers[0].project_root,
        Some(std::path::PathBuf::from("/tmp"))
    );
    assert!(editor.status_msg.contains("Project root set"));

    // Invalid directory
    editor.execute_command("set-project-root /nonexistent_mae_test_xyz");
    assert!(editor.status_msg.contains("Not a directory"));

    // No args
    editor.execute_command("set-project-root");
    assert!(editor.status_msg.contains("Usage"));
}

#[test]
fn git_or_project_root_finds_git_above_subcrate() {
    // Create a temp dir structure: root/.git + root/crates/core/Cargo.toml
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::create_dir_all(root.join("crates/core")).unwrap();
    fs::write(root.join("crates/core/Cargo.toml"), "[package]").unwrap();

    let mut editor = Editor::new();
    editor.project = Some(crate::project::Project {
        name: "test".to_string(),
        root: root.join("crates/core"),
        config: None,
    });

    let result = editor.git_or_project_root().unwrap();
    assert_eq!(result, root.to_path_buf());
}

#[test]
fn git_or_project_root_falls_back_to_project_root_without_git() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("crates/core")).unwrap();

    let mut editor = Editor::new();
    editor.project = Some(crate::project::Project {
        name: "test".to_string(),
        root: root.join("crates/core"),
        config: None,
    });

    let result = editor.git_or_project_root().unwrap();
    assert_eq!(result, root.join("crates/core"));
}

#[test]
fn open_file_does_not_switch_to_subcrate() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Workspace root with .git
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]").unwrap();
    // Subcrate
    let subcrate = root.join("crates/core");
    fs::create_dir_all(subcrate.join("src")).unwrap();
    fs::write(subcrate.join("Cargo.toml"), "[package]").unwrap();
    let file = subcrate.join("src/lib.rs");
    fs::write(&file, "// test").unwrap();

    let mut editor = Editor::new();
    // Set project to workspace root
    editor.project = Some(crate::project::Project::from_root(root.to_path_buf()));

    // Open a file inside the subcrate
    editor.open_file(file.display().to_string());

    // Project should NOT have switched to the subcrate
    assert_eq!(editor.project.as_ref().unwrap().root, root.to_path_buf());
}

#[test]
fn open_file_from_different_project_does_not_switch_global() {
    use std::io::Write;
    // Project A
    let dir_a = tempfile::tempdir().unwrap();
    fs::File::create(dir_a.path().join("Cargo.toml"))
        .unwrap()
        .write_all(b"[package]\nname = \"proj-a\"")
        .unwrap();
    let src_a = dir_a.path().join("main.rs");
    fs::File::create(&src_a)
        .unwrap()
        .write_all(b"fn a() {}")
        .unwrap();

    // Project B
    let dir_b = tempfile::tempdir().unwrap();
    fs::File::create(dir_b.path().join("Cargo.toml"))
        .unwrap()
        .write_all(b"[package]\nname = \"proj-b\"")
        .unwrap();
    let src_b = dir_b.path().join("lib.rs");
    fs::File::create(&src_b)
        .unwrap()
        .write_all(b"fn b() {}")
        .unwrap();

    let mut editor = Editor::new();
    // Open file A — sets global project (first file, project is None).
    editor.open_file(src_a.to_str().unwrap());
    let original_root = editor.project.as_ref().unwrap().root.clone();
    editor.pending_lsp_root_change = None;

    // Open file from a different project.
    editor.open_file(src_b.to_str().unwrap());

    // Global project unchanged.
    assert_eq!(editor.project.as_ref().unwrap().root, original_root);
    assert!(editor.pending_lsp_root_change.is_none());
    // But the new buffer knows its own project root.
    let buf_b = editor.buffers.last().unwrap();
    assert!(
        buf_b.project_root.is_some(),
        "buffer should have its own project_root"
    );
    assert_eq!(
        buf_b.project_root.as_ref().unwrap(),
        &dir_b.path().canonicalize().unwrap()
    );
}
