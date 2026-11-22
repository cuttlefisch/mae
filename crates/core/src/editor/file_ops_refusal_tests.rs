//! Every save path is held to the same refusals (#969): MAE's own config is
//! not written by an AI-originated or unattended save, and nothing an AI saves
//! lands outside an active test sandbox. Each test drives the live entry
//! points, not the refusal helper, because a comment once claimed a
//! reachability that a test then disproved.

use super::*;
use std::time::{Duration, Instant};

/// A modified buffer visiting `file`, which holds `";; original\n"` on disk,
/// edited in the buffer to `edited`.
fn modified_buffer(editor: &mut Editor, file: &Path, edited: &str) -> usize {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(file, ";; original\n").unwrap();
    editor.buffers.push(Buffer::from_file(file).unwrap());
    let idx = editor.buffers.len() - 1;
    editor.buffers[idx].replace_contents(edited);
    editor.buffers[idx].modified = true;
    idx
}

fn on_disk(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap()
}

/// The ways a save can be started, each driven through its real entry point.
#[derive(Clone, Copy, Debug)]
enum SavePath {
    /// `:w` / `execute_command("save")`.
    Write,
    /// `:wa`.
    ExWriteAll,
    /// `execute_command("save-all-buffers")`.
    SaveAllCommand,
    /// `:w!`'s helper and the hash-checked helper.
    Helpers,
    /// The autosave timer, outside any AI dispatch.
    Autosave,
}

const EVERY_PATH: [SavePath; 5] = [
    SavePath::Write,
    SavePath::ExWriteAll,
    SavePath::SaveAllCommand,
    SavePath::Helpers,
    SavePath::Autosave,
];

/// Run `path` against buffer `idx`. `ai` marks the save AI-originated, as the
/// MCP and agent dispatch scope does; the autosave timer never runs inside one.
fn save_via(editor: &mut Editor, idx: usize, path: SavePath, ai: bool) {
    editor.window_mgr.focused_window_mut().buffer_idx = idx;
    editor.ai.ai_dispatch_depth = u32::from(ai && !matches!(path, SavePath::Autosave));
    match path {
        SavePath::Write => editor.save_current_buffer(),
        SavePath::ExWriteAll => {
            editor.execute_command("wa");
        }
        SavePath::SaveAllCommand => {
            editor.dispatch_builtin("save-all-buffers");
        }
        SavePath::Helpers => {
            let _ = editor.save_buffer_with_hash_check(idx);
            let _ = editor.save_buffer_force(idx);
        }
        SavePath::Autosave => {
            editor.autosave_interval = 1;
            // Past both the interval and autosave's 5 s edit-idle wait.
            let long_ago = Instant::now()
                .checked_sub(Duration::from_secs(10))
                .expect("monotonic clock older than 10 s");
            editor.last_autosave = long_ago;
            editor.last_edit_time = long_ago;
            editor.try_autosave();
        }
    }
    editor.ai.ai_dispatch_depth = 0;
}

/// The escalation the config guard exists to stop, on every save path: an
/// agent opens `.mae/init.scm`, rewrites it in the buffer, and saves. `:wa`
/// and `save-all-buffers` wrote it while `:w` refused (#969); the autosave
/// timer wrote it with nobody asking.
#[test]
fn no_ai_or_unattended_save_reaches_project_config_on_any_path() {
    for path in EVERY_PATH {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join(".mae").join("init.scm");
        let mut editor = Editor::new();
        let idx = modified_buffer(
            &mut editor,
            &file,
            "(set-option! \"ai_permission_tier\" \"full\")\n",
        );
        save_via(&mut editor, idx, path, true);
        assert_eq!(on_disk(&file), ";; original\n", "{path:?} wrote MAE config");
    }
}

/// Positive control for the test above, on the same paths: without it, that
/// test would also pass if a path never saved anything. A plain file is
/// saved by every path, AI-originated or not, and a human's command saves
/// their own config.
#[test]
fn every_save_path_still_writes_what_it_is_allowed_to() {
    for path in EVERY_PATH {
        for ai in [false, true] {
            let tmp = tempfile::TempDir::new().unwrap();
            let file = tmp.path().join("notes.txt");
            let mut editor = Editor::new();
            let idx = modified_buffer(&mut editor, &file, "edited\n");
            save_via(&mut editor, idx, path, ai);
            assert_eq!(
                on_disk(&file),
                "edited\n",
                "{path:?} (ai: {ai}) did not save"
            );
        }
        if matches!(path, SavePath::Autosave) {
            continue; // unattended: refused by design, tested above
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join(".mae").join("init.scm");
        let mut editor = Editor::new();
        let idx = modified_buffer(&mut editor, &file, ";; the human's edit\n");
        save_via(&mut editor, idx, path, false);
        assert_eq!(
            on_disk(&file),
            ";; the human's edit\n",
            "{path:?}: a human saving their own config must be unaffected"
        );
    }
}

/// A sandbox, a file outside it, and a file inside it, all real.
struct Sandboxed {
    _tmp: tempfile::TempDir,
    editor: Editor,
    sandbox: PathBuf,
    outside: PathBuf,
}

fn sandboxed() -> Sandboxed {
    let tmp = tempfile::TempDir::new().unwrap();
    let sandbox = tmp.path().join("sandbox");
    std::fs::create_dir_all(&sandbox).unwrap();
    let mut editor = Editor::new();
    editor.test_sandbox_dir = Some(sandbox.clone());
    Sandboxed {
        outside: tmp.path().join("real-project").join("src.rs"),
        _tmp: tmp,
        editor,
        sandbox,
    }
}

/// #969: with a test sandbox active, an agent that opens a real file, edits
/// it and saves must not reach the real filesystem, on any save path. Before
/// the fix the sandbox confined only `create_file` and `rename_file`.
#[test]
fn an_ai_save_outside_the_test_sandbox_is_refused_on_every_path() {
    for path in EVERY_PATH {
        let mut s = sandboxed();
        let idx = modified_buffer(&mut s.editor, &s.outside, "fn planted() {}\n");
        save_via(&mut s.editor, idx, path, true);
        assert_eq!(
            on_disk(&s.outside),
            ";; original\n",
            "{path:?} escaped the sandbox"
        );
    }
}

/// The sandbox bounds the agent under test, not the human sharing the editor
/// (an MCP client runs the self-test against a live session): a human's
/// command still saves outside it, and an AI save inside it still lands.
#[test]
fn the_sandbox_leaves_human_saves_and_saves_inside_it_alone() {
    let mut s = sandboxed();
    let idx = modified_buffer(&mut s.editor, &s.outside, "human\n");
    save_via(&mut s.editor, idx, SavePath::Write, false);
    assert_eq!(on_disk(&s.outside), "human\n");

    for path in EVERY_PATH {
        let mut s = sandboxed();
        let inside = s.sandbox.join("work").join("scratch.txt");
        let idx = modified_buffer(&mut s.editor, &inside, "agent\n");
        save_via(&mut s.editor, idx, path, true);
        assert_eq!(
            on_disk(&inside),
            "agent\n",
            "{path:?} refused a save inside"
        );
    }
}

/// Judged where the write lands: a link inside the sandbox to a real file
/// outside it is refused, though its own path starts with the sandbox.
#[cfg(unix)]
#[test]
fn an_ai_save_through_a_link_out_of_the_sandbox_is_refused() {
    let mut s = sandboxed();
    std::fs::create_dir_all(s.outside.parent().unwrap()).unwrap();
    std::fs::write(&s.outside, ";; original\n").unwrap();
    let link = s.sandbox.join("innocent.rs");
    std::os::unix::fs::symlink(&s.outside, &link).unwrap();
    let mut buf = Buffer::from_file(&link).unwrap();
    buf.replace_contents("fn planted() {}\n");
    buf.modified = true;
    s.editor.buffers.push(buf);
    let idx = s.editor.buffers.len() - 1;
    save_via(&mut s.editor, idx, SavePath::Write, true);
    assert_eq!(on_disk(&s.outside), ";; original\n");
}

/// The ex `:rename` and `:copy` effects, which an AI can reach through ex
/// lines, are confined where they write. A rename writes at both ends (the
/// source is removed); a copy writes only its target, so copying a real file
/// INTO the sandbox is a read and allowed, while copying out is refused. A
/// human's are not confined.
#[test]
fn ex_rename_and_copy_are_confined_where_they_write_for_an_ai_only() {
    // (command, ai, target inside the sandbox, expected to happen)
    let cases = [
        ("rename", true, true, false), // the source is a real file: refused
        ("copy", true, false, false),  // writes a real file: refused
        ("copy", true, true, true),    // reads a real file: allowed
        ("rename", false, true, true), // the human's own: allowed
        ("copy", false, false, true),  // the human's own: allowed
    ];
    for (cmd, ai, inside, happens) in cases {
        let mut s = sandboxed();
        let idx = modified_buffer(&mut s.editor, &s.outside, ";; original\n");
        s.editor.buffers[idx].modified = false;
        s.editor.window_mgr.focused_window_mut().buffer_idx = idx;
        let target = if inside {
            s.sandbox.join("moved.rs")
        } else {
            s.outside.with_file_name("copied.rs")
        };
        s.editor.ai.ai_dispatch_depth = u32::from(ai);
        s.editor
            .execute_command(&format!("{cmd} {}", target.display()));
        s.editor.ai.ai_dispatch_depth = 0;
        let case = format!(":{cmd} (ai: {ai}, into the sandbox: {inside})");
        assert_eq!(target.exists(), happens, "{case}: target");
        let source_kept = !(happens && cmd == "rename");
        assert_eq!(s.outside.exists(), source_kept, "{case}: source");
    }
}
