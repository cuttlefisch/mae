use std::path::{Path, PathBuf};

use mae_core::Editor;

pub fn execute_open_file(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;

    // Check if file is already open in a buffer
    let file_path = PathBuf::from(path);
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
        editor.switch_to_buffer_non_conversation(idx);
        return Ok(format!(
            "Switched to existing buffer '{}' (already open)",
            name
        ));
    }

    // Open new buffer
    editor.open_file_non_conversation(path);
    if editor.status_msg.contains("Error") {
        Err(editor.status_msg.clone())
    } else {
        let idx = editor.active_buffer_idx();
        editor.ai_target_buffer_idx = Some(idx);
        Ok(format!(
            "Opened '{}' ({} lines)",
            editor.active_buffer().name,
            editor.active_buffer().line_count()
        ))
    }
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

    editor.switch_to_buffer_non_conversation(idx);
    editor.ai_target_buffer_idx = Some(idx);
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

    let p = Path::new(path);
    match editor.ai_save(p) {
        Ok(n) => Ok(format!("Saved {} entries to {}", n, p.display())),
        Err(e) => Err(e),
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
    let new_path = args
        .get("new_path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'new_path' argument")?;

    let idx = editor.active_buffer_idx();
    let old_path = editor.buffers[idx]
        .file_path()
        .map(|p| p.to_path_buf())
        .ok_or("Buffer has no file path")?;

    let new = PathBuf::from(new_path);
    std::fs::rename(&old_path, &new).map_err(|e| format!("Rename failed: {}", e))?;

    editor.buffers[idx].set_file_path(new.clone());
    editor.buffers[idx].name = new
        .file_name()
        .map_or(new_path.to_string(), |n| n.to_string_lossy().to_string());

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
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

    let file_path = Path::new(path);

    // Create parent directories if needed
    if let Some(parent) = file_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }
    }

    // Write the file
    std::fs::write(file_path, content).map_err(|e| format!("Failed to create file: {}", e))?;

    // Open it as a buffer
    editor.open_file_non_conversation(path);
    if editor.status_msg.contains("Error") {
        Err(editor.status_msg.clone())
    } else {
        Ok(format!(
            "Created '{}' ({} bytes) and opened as buffer",
            path,
            content.len()
        ))
    }
}
