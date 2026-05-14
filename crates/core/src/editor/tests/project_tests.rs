//! Project root detection and management tests.

use super::*;
use std::fs;

#[test]
fn active_project_root_falls_back_to_editor_project() {
    let mut ed = Editor::new();
    // No project set anywhere
    assert!(ed.active_project_root().is_none());

    // Set editor-wide project
    ed.project = Some(crate::project::Project::from_root(
        std::path::PathBuf::from("/tmp"),
    ));
    assert_eq!(
        ed.active_project_root().unwrap(),
        std::path::Path::new("/tmp")
    );
}

#[test]
fn active_project_root_prefers_buffer_project() {
    let mut ed = Editor::new();
    ed.project = Some(crate::project::Project::from_root(
        std::path::PathBuf::from("/editor-wide"),
    ));
    ed.buffers[0].project_root = Some(std::path::PathBuf::from("/buffer-specific"));
    assert_eq!(
        ed.active_project_root().unwrap(),
        std::path::Path::new("/buffer-specific")
    );
}

#[test]
fn set_project_root_command() {
    let mut ed = Editor::new();
    // Valid directory
    ed.execute_command("set-project-root /tmp");
    assert_eq!(
        ed.buffers[0].project_root,
        Some(std::path::PathBuf::from("/tmp"))
    );
    assert!(ed.status_msg.contains("Project root set"));

    // Invalid directory
    ed.execute_command("set-project-root /nonexistent_mae_test_xyz");
    assert!(ed.status_msg.contains("Not a directory"));

    // No args
    ed.execute_command("set-project-root");
    assert!(ed.status_msg.contains("Usage"));
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

    let mut ed = Editor::new();
    // Set project to workspace root
    ed.project = Some(crate::project::Project::from_root(root.to_path_buf()));

    // Open a file inside the subcrate
    ed.open_file(file.display().to_string());

    // Project should NOT have switched to the subcrate
    assert_eq!(ed.project.as_ref().unwrap().root, root.to_path_buf());
}
