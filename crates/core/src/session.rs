//! Session persistence — save and restore buffer list + cursor positions.
//!
//! Sessions are stored as JSON at `{project_root}/.mae/session.json`.
//! Non-file buffers (shell, AI conversation, help, etc.) are skipped.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Serialized session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub buffers: Vec<SessionBuffer>,
    pub focused_idx: usize,
}

/// Per-buffer state saved in a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBuffer {
    pub file_path: PathBuf,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
    pub project_root: Option<PathBuf>,
}

impl Session {
    pub const VERSION: u32 = 1;

    /// Build a session from current editor state.
    pub fn from_editor(editor: &super::Editor) -> Self {
        let win = editor.window_mgr.focused_window();
        let focused_idx = win.buffer_idx;

        let buffers: Vec<SessionBuffer> = editor
            .buffers
            .iter()
            .enumerate()
            .filter_map(|(i, buf)| {
                // Only save file-backed text buffers
                if buf.kind != crate::BufferKind::Text {
                    return None;
                }
                let file_path = buf.file_path()?.to_path_buf();
                let (cursor_row, cursor_col, scroll_offset) = if i == focused_idx {
                    (win.cursor_row, win.cursor_col, win.scroll_offset)
                } else {
                    // For non-focused buffers, save defaults (we don't track per-buffer cursors easily)
                    (0, 0, 0)
                };
                Some(SessionBuffer {
                    file_path,
                    cursor_row,
                    cursor_col,
                    scroll_offset,
                    project_root: buf.project_root.clone(),
                })
            })
            .collect();

        Session {
            version: Self::VERSION,
            buffers,
            focused_idx,
        }
    }

    /// Session file path for a project root.
    pub fn session_path(project_root: &Path) -> PathBuf {
        project_root.join(".mae").join("session.json")
    }

    /// Save session to disk.
    pub fn save(&self, project_root: &Path) -> Result<(), String> {
        let path = Self::session_path(project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create .mae dir: {}", e))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize session: {}", e))?;
        std::fs::write(&path, json).map_err(|e| format!("Failed to write session: {}", e))?;
        Ok(())
    }

    /// Load session from disk.
    pub fn load(project_root: &Path) -> Result<Self, String> {
        let path = Self::session_path(project_root);
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("Failed to read session: {}", e))?;
        let session: Session = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse session: {}", e))?;
        if session.version != Self::VERSION {
            return Err(format!(
                "Session version mismatch: expected {}, got {}",
                Self::VERSION,
                session.version
            ));
        }
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session {
            version: Session::VERSION,
            buffers: vec![SessionBuffer {
                file_path: PathBuf::from("/tmp/test.rs"),
                cursor_row: 10,
                cursor_col: 5,
                scroll_offset: 3,
                project_root: Some(PathBuf::from("/tmp")),
            }],
            focused_idx: 0,
        };
        session.save(dir.path()).unwrap();
        let loaded = Session::load(dir.path()).unwrap();
        assert_eq!(loaded.buffers.len(), 1);
        assert_eq!(loaded.buffers[0].cursor_row, 10);
        assert_eq!(loaded.focused_idx, 0);
    }

    #[test]
    fn session_load_missing_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Session::load(dir.path()).is_err());
    }
}
