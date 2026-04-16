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
        editor.switch_to_buffer(idx);
        return Ok(format!(
            "Switched to existing buffer '{}' (already open)",
            name
        ));
    }

    // Open new buffer
    editor.open_file(path);
    if editor.status_msg.contains("Error") {
        Err(editor.status_msg.clone())
    } else {
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

    editor.switch_to_buffer(idx);
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

    if editor.buffers[idx].modified {
        return Err(format!(
            "Buffer '{}' has unsaved changes",
            editor.buffers[idx].name
        ));
    }

    let name = editor.buffers[idx].name.clone();
    // Switch to this buffer first so kill-buffer acts on it
    editor.switch_to_buffer(idx);
    editor.dispatch_builtin("kill-buffer");
    Ok(format!("Closed buffer '{}'", name))
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
    editor.open_file(path);
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
