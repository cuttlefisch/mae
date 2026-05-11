//! Image metadata AI tool implementations.

use mae_core::Editor;
use std::path::PathBuf;

/// Read image metadata for a single image file.
pub fn execute_image_info(args: &serde_json::Value) -> Result<String, String> {
    let raw_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;
    let path = mae_core::file_picker::expand_tilde(raw_path);
    let path = PathBuf::from(&path);
    let meta = mae_core::image_meta::read_image_meta(&path)?;
    serde_json::to_string_pretty(&meta).map_err(|e| format!("Serialization error: {}", e))
}

/// List all image links in the current buffer with resolved paths.
pub fn execute_image_list(editor: &Editor) -> Result<String, String> {
    let idx = editor
        .ai_target_buffer_idx
        .unwrap_or_else(|| editor.active_buffer_idx());
    let buf = &editor.buffers[idx];

    let text: String = buf.rope().chars().collect();
    let extension = buf
        .file_path()
        .and_then(|p| p.extension().and_then(|e| e.to_str()).map(String::from));
    let base_dir = buf
        .file_path()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let empty = std::collections::HashSet::new();
    let regions = mae_core::display_region::compute_image_regions(
        &text,
        extension.as_deref(),
        base_dir.as_deref(),
        &empty,
    );

    if regions.is_empty() {
        return Ok("No image links found in the current buffer.".into());
    }

    let mut entries = Vec::new();
    for region in &regions {
        let mut entry = serde_json::json!({
            "link_text": &text[region.byte_start..region.byte_end],
            "target": region.link_target,
        });

        if let Some(ref img) = region.image {
            entry["resolved_path"] = serde_json::json!(img.path.display().to_string());
            if let Some(w) = img.width {
                entry["attr_width"] = serde_json::json!(w);
            }
            // Try to read metadata.
            if let Ok(meta) = mae_core::image_meta::read_image_meta(&img.path) {
                entry["width"] = serde_json::json!(meta.width);
                entry["height"] = serde_json::json!(meta.height);
                entry["format"] = serde_json::json!(meta.format);
                entry["file_size"] = serde_json::json!(meta.file_size);
            }
        } else {
            entry["error"] = serde_json::json!("file not found");
        }

        entries.push(entry);
    }

    serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_info_missing_file() {
        let args = serde_json::json!({"path": "/nonexistent/image.png"});
        let result = execute_image_info(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn image_info_missing_arg() {
        let args = serde_json::json!({});
        let result = execute_image_info(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing"));
    }

    #[test]
    fn image_list_empty_buffer() {
        let editor = Editor::new();
        let result = execute_image_list(&editor).unwrap();
        assert!(result.contains("No image links"));
    }

    #[test]
    fn image_info_real_fixture() {
        let assets_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("assets/test-image.png");
        if !assets_dir.exists() {
            return;
        }
        let args = serde_json::json!({"path": assets_dir.to_str().unwrap()});
        let result = execute_image_info(&args).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(meta["width"], 16);
        assert_eq!(meta["height"], 16);
        assert!(meta["file_size"].as_u64().unwrap() > 0);
    }
}
