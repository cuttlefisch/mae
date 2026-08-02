use std::path::{Path, PathBuf};

use mae_core::Editor;

pub fn execute_open_file(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let raw_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;
    let path = mae_core::file_picker::expand_tilde(raw_path);

    // Check if file is already open in a buffer
    let file_path = PathBuf::from(&path);
    let canonical = file_path.canonicalize().ok();
    let existing_idx = editor.buffers.iter().enumerate().find_map(|(i, buf)| {
        buf.file_path().and_then(|bp| {
            if bp == file_path || canonical.as_deref() == bp.canonicalize().ok().as_deref() {
                Some(i)
            } else {
                None
            }
        })
    });
    if let Some(idx) = existing_idx {
        let name = editor.buffers[idx].name.clone();
        editor.display_buffer_for_agent(idx);
        return Ok(format!(
            "Switched to existing buffer '{}' (already open)",
            name
        ));
    }

    // Open new buffer. `open_file_non_conversation` returns the real success/failure
    // of the open (ADR-086) — no more deciding outcome by sniffing `status_msg` for
    // the word "Error", which fails open the moment that UI string is reworded.
    let new_idx = editor.open_file_non_conversation(&path)?;
    let target_name = editor.buffers[new_idx].name.clone();
    let line_count = editor.buffers[new_idx].line_count();
    Ok(format!("Opened '{}' ({} lines)", target_name, line_count))
}

pub fn execute_switch_buffer(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' argument")?;

    let idx = editor
        .find_buffer_by_name(name)
        .ok_or_else(|| format!("No buffer named '{}'", name))?;

    editor.display_buffer_for_agent(idx);
    Ok(format!("Switched to buffer '{}'", name))
}

pub fn execute_close_buffer(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let idx = if let Some(name) = args.get("name").and_then(|v| v.as_str()) {
        editor
            .find_buffer_by_name(name)
            .ok_or_else(|| format!("No buffer named '{}'", name))?
    } else {
        editor.active_buffer_idx()
    };

    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

    if editor.buffers[idx].modified && !force {
        return Err(format!(
            "Buffer '{}' has unsaved changes (use force=true to close anyway)",
            editor.buffers[idx].name
        ));
    }

    let name = editor.buffers[idx].name.clone();
    // Switch to this buffer first so kill-buffer acts on it
    editor.switch_to_buffer(idx);
    if force {
        editor.dispatch_builtin("force-kill-buffer");
    } else {
        editor.dispatch_builtin("kill-buffer");
    }
    Ok(format!("Closed buffer '{}'", name))
}

pub fn execute_ai_save(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;

    let expanded = mae_core::file_picker::expand_tilde(path);
    let p = Path::new(&expanded);

    // If the path has no directory component or points directly into $HOME,
    // redirect to the XDG transcripts directory so test runs and casual saves
    // don't litter the home directory.
    let resolved = if should_redirect_to_transcripts(p) {
        let transcripts_dir = transcripts_dir();
        let _ = std::fs::create_dir_all(&transcripts_dir);
        let filename = p
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("conversation.json"));
        transcripts_dir.join(filename)
    } else {
        PathBuf::from(p)
    };

    match editor.ai_save(&resolved) {
        Ok(n) => Ok(format!("Saved {} entries to {}", n, resolved.display())),
        Err(e) => Err(e),
    }
}

/// Returns true if the save path should be redirected to the transcripts dir.
/// Catches: bare filenames, `~/foo.json`, `$HOME/foo.json` (no subdirectory).
fn should_redirect_to_transcripts(p: &Path) -> bool {
    // Bare filename with no directory component → redirect.
    if p.parent().is_none_or(|parent| parent == Path::new("")) {
        return true;
    }
    // Direct child of $HOME (e.g. ~/foo.json) → redirect.
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(&home);
        if let Some(parent) = p.parent() {
            if parent == home_path {
                return true;
            }
        }
    }
    false
}

/// XDG-compliant transcripts directory.
fn transcripts_dir() -> PathBuf {
    if let Ok(data) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(data).join("mae/transcripts")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local/share/mae/transcripts")
    } else {
        PathBuf::from("/tmp/mae-transcripts")
    }
}

pub fn execute_ai_load(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;

    let p = Path::new(path);
    match editor.ai_load(p) {
        Ok(n) => Ok(format!("Loaded {} entries from {}", n, p.display())),
        Err(e) => Err(e),
    }
}

pub fn execute_rename_file(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let raw_new_path = args
        .get("new_path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'new_path' argument")?;
    let new_path = mae_core::file_picker::expand_tilde(raw_new_path);

    let idx = editor.active_buffer_idx();
    let old_path = editor.buffers[idx]
        .file_path()
        .map(|p| p.to_path_buf())
        .ok_or("Buffer has no file path")?;

    let new = PathBuf::from(&new_path);

    // @ai-caution: [security] Rename is a write in disguise — moving a buffer onto
    // `.mae/init.scm` reaches the same escalation `create_file` is guarded against
    // (ADR-089 D4). Both source and destination are checked: the destination so a
    // file cannot be moved *into* config, the source so config cannot be moved out
    // of the way to defeat a subsequent check.
    for candidate in [&old_path, &new] {
        if mae_core::workspace_trust::is_protected_config_path(candidate) {
            return Err(format!(
                "Refused: '{}' is MAE configuration, which governs what tools are permitted. \
                 Renaming it is not allowed at this permission tier.",
                candidate.display()
            ));
        }
    }

    std::fs::rename(&old_path, &new).map_err(|e| format!("Rename failed: {}", e))?;

    editor.buffers[idx].set_file_path(new.clone());
    editor.buffers[idx].name = new
        .file_name()
        .map_or(new_path.to_string(), |n| n.to_string_lossy().to_string());
    editor.redetect_language_for(idx);

    Ok(format!(
        "Renamed: {} → {}",
        old_path.display(),
        new.display()
    ))
}

pub fn execute_create_file(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let raw_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;
    let path = mae_core::file_picker::expand_tilde(raw_path);
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

    let file_path = Path::new(&path);

    // @ai-caution: [security] MAE's own config governs what the agent may do, so the
    // agent may not write it (ADR-089 D4). Without this, a write-tier agent plants
    // `.mae/init.scm` and escalates to arbitrary code execution on the next start —
    // the shape of CVE-2025-53773. The check resolves symlinks and `..` first.
    if mae_core::workspace_trust::is_protected_config_path(file_path) {
        return Err(format!(
            "Refused: '{}' is MAE configuration, which governs what tools are permitted. \
             Writing it is not allowed at this permission tier. Ask the user to edit it.",
            path
        ));
    }

    // Create parent directories if needed
    if let Some(parent) = file_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }
    }

    // Write the file
    std::fs::write(file_path, content).map_err(|e| format!("Failed to create file: {}", e))?;

    // If a buffer already has this file open, reload it from disk so
    // the editor sees the freshly written content (not stale buffer state).
    // A failed reload must not be silently swallowed (ADR-086): the open step
    // below short-circuits to this SAME buffer without re-reading if it's
    // already open, so a discarded reload failure here would leave stale
    // buffer content behind a reported success.
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&path)
        .to_string();
    if let Some(existing) = editor.find_buffer_by_name(&file_name) {
        if let Err(e) = editor.buffers[existing].reload_from_disk() {
            return Err(format!(
                "Created '{}' ({} bytes) on disk, but the already-open buffer '{}' failed to \
                 reload the new content: {}",
                path,
                content.len(),
                file_name,
                e
            ));
        }
    }

    // Open it as a buffer (reuses existing if present). The file is already durably
    // written above (`std::fs::write` succeeded), so a failure here is a partial
    // success: report both halves rather than a bare error that loses the fact the
    // write happened, and rather than a bare success that hides the open failed
    // (ADR-086 D5 — no partial success collapsed into unqualified prose).
    match editor.open_file_non_conversation(&path) {
        Ok(new_idx) => Ok(format!(
            "Created '{}' ({} bytes) and opened as buffer '{}'",
            path,
            content.len(),
            editor.buffers[new_idx].name
        )),
        Err(e) => Err(format!(
            "Created '{}' ({} bytes) on disk, but failed to open it as a buffer: {}",
            path,
            content.len(),
            e
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-086 / CLAUDE.md #14: `open_file` on a path that cannot actually be
    /// opened must be `Err`, and that `Err` must come from the real
    /// `std::io::Error` `Buffer::from_file` hits (attempting to read a
    /// directory as a file), NOT from re-inspecting `editor.status_msg` for
    /// the substring "Error" — the exact defect this ADR fixes. A directory
    /// is used because it is a portable, permission-independent way to make
    /// `fs::read_to_string` fail (unlike a chmod-based permission-denied
    /// case, which behaves inconsistently when tests run as root).
    #[test]
    fn execute_open_file_on_a_directory_is_err_with_the_real_io_error() {
        let dir = std::env::temp_dir().join("mae_test_open_file_on_directory");
        std::fs::create_dir_all(&dir).unwrap();
        let mut editor = Editor::new();

        let result = execute_open_file(
            &mut editor,
            &serde_json::json!({"path": dir.to_str().unwrap()}),
        );

        let err = result.expect_err("opening a directory as a file must fail, not succeed");
        assert!(
            !err.is_empty(),
            "the error must carry the real io::Error text, not an empty/generic placeholder"
        );
        // No buffer for the directory should have been created — the refusal
        // must be complete, not a partial buffer-creation-then-fail.
        assert!(
            !editor
                .buffers
                .iter()
                .any(|b| b.file_path() == Some(dir.as_path())),
            "a failed open must not leave a half-created buffer behind"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ADR-086 D2 guard: the fix above must not over-correct into breaking
    /// the legitimate idempotent-retry path — opening the SAME already-open
    /// file a second time is a no-op success (the early "already open"
    /// return in `open_file_hidden`), not an error.
    #[test]
    fn execute_open_file_twice_on_the_same_file_both_succeed() {
        let dir = std::env::temp_dir().join("mae_test_open_file_twice");
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("reopen.txt");
        std::fs::write(&file_path, "hello\n").unwrap();
        let mut editor = Editor::new();
        let args = serde_json::json!({"path": file_path.to_str().unwrap()});

        execute_open_file(&mut editor, &args).expect("first open must succeed");
        execute_open_file(&mut editor, &args)
            .expect("re-opening the same already-open file must still succeed (idempotent)");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ADR-086 / CLAUDE.md #14: `create_file` targeting a path that IS a
    /// directory can never satisfy its postcondition (write a file at that
    /// path) — `std::fs::write` fails with `EISDIR`, and that must surface
    /// as `Err`, not a status-string-sniffed success.
    #[test]
    fn execute_create_file_onto_an_existing_directory_is_err() {
        let dir = std::env::temp_dir().join("mae_test_create_file_onto_directory");
        std::fs::create_dir_all(&dir).unwrap();
        let mut editor = Editor::new();

        let result = execute_create_file(
            &mut editor,
            &serde_json::json!({"path": dir.to_str().unwrap(), "content": "x"}),
        );

        assert!(
            result.is_err(),
            "creating a file at a path that is already a directory must fail: {result:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
