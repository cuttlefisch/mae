//! LSP code actions, formatting, and rename operations.

use super::Editor;

/// Deserialization helper for workspace edit text edits.
#[derive(serde::Deserialize)]
struct TextEditJson {
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
    new_text: String,
}

impl Editor {
    /// Queue a `textDocument/codeAction` request at the cursor.
    pub fn lsp_request_code_action(&mut self) {
        let (uri, lang_id) = {
            let idx = self.active_buffer_idx();
            let buf = &self.buffers[idx];
            let Some(path) = buf.file_path() else {
                self.set_status("LSP code-action: buffer has no file path");
                return;
            };
            let Some(lang_id) = crate::lsp_intent::language_id_from_path(path) else {
                self.set_status("LSP code-action: unsupported language");
                return;
            };
            (crate::lsp_intent::path_to_uri(path), lang_id)
        };
        let suffix = self.lsp_starting_suffix(&lang_id);
        let win = self.window_mgr.focused_window();
        self.pending_lsp_requests
            .push(crate::LspIntent::CodeAction {
                uri,
                language_id: lang_id,
                line: win.cursor_row as u32,
                character: win.cursor_col as u32,
            });
        self.set_status(format!(
            "LSP code-action: awaiting server response{}",
            suffix
        ));
    }

    /// Apply code action results from LSP — populate the code action menu.
    pub fn apply_code_action_result_items(&mut self, items: Vec<super::CodeActionItem>) {
        if items.is_empty() {
            self.set_status("[LSP] no code actions available");
            return;
        }
        let count = items.len();
        self.code_action_menu = Some(super::CodeActionMenu { items, selected: 0 });
        self.set_status(format!(
            "[LSP] {} code action(s) — j/k navigate, Enter apply, Esc dismiss",
            count
        ));
    }

    /// Navigate code action menu down.
    pub fn code_action_next(&mut self) {
        if let Some(ref mut menu) = self.code_action_menu {
            let len = menu.items.len();
            menu.selected = (menu.selected + 1) % len;
        }
    }

    /// Navigate code action menu up.
    pub fn code_action_prev(&mut self) {
        if let Some(ref mut menu) = self.code_action_menu {
            let len = menu.items.len();
            menu.selected = menu.selected.checked_sub(1).unwrap_or(len - 1);
        }
    }

    /// Dismiss the code action menu without applying.
    pub fn code_action_dismiss(&mut self) {
        self.code_action_menu = None;
    }

    /// Apply the selected code action's workspace edit.
    pub fn code_action_select(&mut self) {
        let menu = match self.code_action_menu.take() {
            Some(m) => m,
            None => return,
        };
        let item = &menu.items[menu.selected];
        if let Some(ref edit_json) = item.edit_json {
            self.apply_workspace_edit_json(edit_json);
        }
        self.set_status(format!("[LSP] applied: {}", item.title));
    }

    /// Apply a workspace edit from JSON (TextEdits per URI).
    pub(super) fn apply_workspace_edit_json(&mut self, json: &str) {
        let Ok(edits) = serde_json::from_str::<Vec<(String, Vec<TextEditJson>)>>(json) else {
            self.set_status("[LSP] failed to parse workspace edit");
            return;
        };
        for (uri, text_edits) in edits {
            let path = uri.strip_prefix("file://").unwrap_or(&uri);
            let buf_idx = self
                .buffers
                .iter()
                .position(|b| b.file_path().map(|p| p.to_string_lossy()) == Some(path.into()));
            let Some(idx) = buf_idx else {
                continue; // buffer not open — skip
            };
            // Apply edits in reverse order to preserve offsets.
            let mut sorted_edits = text_edits;
            sorted_edits.sort_by(|a, b| {
                b.start_line
                    .cmp(&a.start_line)
                    .then(b.start_character.cmp(&a.start_character))
            });
            for edit in sorted_edits {
                let start = self.buffers[idx]
                    .char_offset_at(edit.start_line as usize, edit.start_character as usize);
                let end = self.buffers[idx]
                    .char_offset_at(edit.end_line as usize, edit.end_character as usize);
                if start < end {
                    self.buffers[idx].delete_range(start, end);
                }
                if !edit.new_text.is_empty() {
                    self.buffers[idx].insert_text_at(start, &edit.new_text);
                }
            }
        }
    }

    /// Queue a `textDocument/formatting` request for the active buffer.
    pub fn lsp_request_format(&mut self) {
        let idx = self.active_buffer_idx();
        let buf = &self.buffers[idx];
        let Some(path) = buf.file_path() else {
            self.set_status("LSP format: buffer has no file path");
            return;
        };
        let Some(lang_id) = crate::lsp_intent::language_id_from_path(path) else {
            self.set_status("LSP format: unsupported language");
            return;
        };
        let uri = crate::lsp_intent::path_to_uri(path);
        self.pending_lsp_requests.push(crate::LspIntent::Format {
            uri,
            language_id: lang_id,
        });
        self.set_status("LSP format: awaiting server response");
    }

    /// Queue a `textDocument/rangeFormatting` request for the visual selection
    /// or fall back to full-file formatting if not in visual mode.
    pub fn lsp_request_range_format(&mut self) {
        let idx = self.active_buffer_idx();
        let buf = &self.buffers[idx];
        let Some(path) = buf.file_path() else {
            self.set_status("LSP format: buffer has no file path");
            return;
        };
        let Some(lang_id) = crate::lsp_intent::language_id_from_path(path) else {
            self.set_status("LSP format: unsupported language");
            return;
        };
        let uri = crate::lsp_intent::path_to_uri(path);

        if let crate::Mode::Visual(_) = self.mode {
            let win = self.window_mgr.focused_window();
            let start_row = self.vi.visual_anchor_row.min(win.cursor_row);
            let end_row = self.vi.visual_anchor_row.max(win.cursor_row);
            let start_col = if start_row == self.vi.visual_anchor_row {
                self.vi.visual_anchor_col
            } else {
                win.cursor_col
            };
            let end_col = if end_row == win.cursor_row {
                win.cursor_col
            } else {
                self.vi.visual_anchor_col
            };
            self.pending_lsp_requests
                .push(crate::LspIntent::RangeFormat {
                    uri,
                    language_id: lang_id,
                    start_line: start_row as u32,
                    start_char: start_col as u32,
                    end_line: end_row as u32,
                    end_char: end_col as u32,
                });
            self.set_status("LSP range format: awaiting server response");
        } else {
            // Fall back to full-file format
            self.pending_lsp_requests.push(crate::LspIntent::Format {
                uri,
                language_id: lang_id,
            });
            self.set_status("LSP format: awaiting server response");
        }
    }

    /// Apply format edits from the LSP server. The `edits_json` is a
    /// JSON-serialized `Vec<(uri, Vec<TextEditJson>)>` — same format as
    /// `apply_workspace_edit_json` uses internally.
    pub fn apply_format_edits_json(&mut self, edits_json: &str, count: usize) {
        self.apply_workspace_edit_json(edits_json);
        self.set_status(format!("[LSP] formatted: {} edit(s) applied", count));
    }

    /// Open a *Rename Preview* buffer showing the unified diff of all
    /// affected locations. Called by the binary when a rename workspace edit
    /// arrives and preview mode is active.
    pub fn show_rename_preview(&mut self, edits_json: &str, new_name: &str) {
        let Ok(edits) = serde_json::from_str::<Vec<(String, Vec<TextEditJson>)>>(edits_json) else {
            self.set_status("[LSP] failed to parse rename preview");
            return;
        };

        let mut diff_text = format!("Rename Preview → '{}'\n\n", new_name);
        let mut total_changes = 0usize;

        for (uri, text_edits) in &edits {
            let path = uri.strip_prefix("file://").unwrap_or(uri);
            diff_text.push_str(&format!("--- {}\n+++ {}\n", path, path));

            // Read buffer content for context
            let buf_idx = self
                .buffers
                .iter()
                .position(|b| b.file_path().map(|p| p.to_string_lossy()) == Some(path.into()));

            if let Some(idx) = buf_idx {
                let buf = &self.buffers[idx];
                for edit in text_edits {
                    let line = edit.start_line as usize;
                    if line < buf.rope().len_lines() {
                        let line_text: String = buf.rope().line(line).chars().collect();
                        let trimmed = line_text.trim_end_matches('\n');
                        diff_text.push_str(&format!(
                            "@@ -{},{} +{},{} @@\n",
                            line + 1,
                            1,
                            line + 1,
                            1
                        ));
                        diff_text.push_str(&format!("-{}\n", trimmed));
                        // Show the line with the edit applied
                        let start_c = edit.start_character as usize;
                        let end_c = edit.end_character as usize;
                        let mut new_line = String::new();
                        for (i, ch) in trimmed.char_indices() {
                            let char_idx = trimmed[..i].chars().count();
                            if char_idx == start_c {
                                new_line.push_str(&edit.new_text);
                            }
                            if char_idx < start_c || char_idx >= end_c {
                                new_line.push(ch);
                            }
                        }
                        // Handle case where edit is at end of line
                        if start_c >= trimmed.chars().count() {
                            new_line.push_str(&edit.new_text);
                        }
                        diff_text.push_str(&format!("+{}\n", new_line));
                        total_changes += 1;
                    }
                }
            } else {
                diff_text.push_str(&format!("  (file not open: {})\n", path));
                total_changes += text_edits.len();
            }
        }

        diff_text.push_str(&format!(
            "\n{} change(s) across {} file(s)",
            total_changes,
            edits.len()
        ));
        diff_text.push_str("\nPress Enter to apply, Esc to abort");

        // Create or reuse the rename preview buffer
        let preview_name = "*Rename Preview*";
        let preview_idx = self.buffers.iter().position(|b| b.name == preview_name);
        let idx = if let Some(idx) = preview_idx {
            self.buffers[idx].replace_contents(&diff_text);
            idx
        } else {
            let mut buf = crate::buffer::Buffer::new();
            buf.name = preview_name.to_string();
            buf.replace_contents(&diff_text);
            buf.modified = false;
            self.buffers.push(buf);
            self.buffers.len() - 1
        };

        // Store the edits JSON for later application
        self.pending_rename_edit = Some(edits_json.to_string());

        // Display the preview buffer
        self.display_buffer_and_focus(idx);
        self.window_mgr.focused_window_mut().scroll_offset = 0;

        self.set_status(format!(
            "[LSP] Rename preview: {} change(s) — Enter to apply, Esc to abort",
            total_changes
        ));
    }

    /// Apply the pending rename edit and close the preview buffer.
    pub fn apply_pending_rename(&mut self) {
        let edits_json = match self.pending_rename_edit.take() {
            Some(j) => j,
            None => return,
        };
        self.apply_workspace_edit_json(&edits_json);
        // Remove the preview buffer
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.name == "*Rename Preview*")
        {
            self.kill_buffer_at(idx);
        }
        self.set_status("[LSP] Rename applied");
    }

    /// Abort a pending rename and close the preview buffer.
    pub fn abort_pending_rename(&mut self) {
        self.pending_rename_edit = None;
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.name == "*Rename Preview*")
        {
            self.kill_buffer_at(idx);
        }
        self.set_status("[LSP] Rename aborted");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use std::path::PathBuf;

    fn editor_with_file(path: &str, text: &str) -> Editor {
        let mut buf = Buffer::new();
        buf.set_file_path(PathBuf::from(path));
        if !text.is_empty() {
            buf.insert_text_at(0, text);
        }
        Editor::with_buffer(buf)
    }

    // -----------------------------------------------------------------------
    // Code action menu tests
    // -----------------------------------------------------------------------

    #[test]
    fn code_action_menu_navigation() {
        use crate::editor::CodeActionItem;
        let mut editor = Editor::new();
        editor.apply_code_action_result_items(vec![
            CodeActionItem {
                title: "Import foo".into(),
                kind: Some("quickfix".into()),
                edit_json: None,
            },
            CodeActionItem {
                title: "Extract function".into(),
                kind: Some("refactor".into()),
                edit_json: None,
            },
            CodeActionItem {
                title: "Remove unused import".into(),
                kind: Some("source".into()),
                edit_json: None,
            },
        ]);
        assert!(editor.code_action_menu.is_some());
        let menu = editor.code_action_menu.as_ref().unwrap();
        assert_eq!(menu.selected, 0);
        assert_eq!(menu.items.len(), 3);

        editor.code_action_next();
        assert_eq!(editor.code_action_menu.as_ref().unwrap().selected, 1);

        editor.code_action_next();
        assert_eq!(editor.code_action_menu.as_ref().unwrap().selected, 2);

        editor.code_action_next(); // wraps
        assert_eq!(editor.code_action_menu.as_ref().unwrap().selected, 0);

        editor.code_action_prev(); // wraps back
        assert_eq!(editor.code_action_menu.as_ref().unwrap().selected, 2);

        editor.code_action_dismiss();
        assert!(editor.code_action_menu.is_none());
    }

    #[test]
    fn code_action_select_applies_workspace_edit() {
        use crate::editor::CodeActionItem;
        let mut editor = editor_with_file("/tmp/a.rs", "hello world\n");
        // Format: Vec<(uri, Vec<TextEdit>)>
        let edit_json = serde_json::json!([
            ["file:///tmp/a.rs", [{
                "start_line": 0,
                "start_character": 0,
                "end_line": 0,
                "end_character": 5,
                "new_text": "goodbye"
            }]]
        ])
        .to_string();
        editor.apply_code_action_result_items(vec![CodeActionItem {
            title: "Replace hello".into(),
            kind: Some("quickfix".into()),
            edit_json: Some(edit_json),
        }]);
        editor.code_action_select();
        let text = editor.active_buffer().text();
        assert!(text.starts_with("goodbye world"));
        assert!(editor.code_action_menu.is_none());
    }

    #[test]
    fn code_action_menu_auto_dismiss_on_motion() {
        use crate::editor::CodeActionItem;
        let mut editor = Editor::new();
        editor.apply_code_action_result_items(vec![CodeActionItem {
            title: "Fix".into(),
            kind: None,
            edit_json: None,
        }]);
        assert!(editor.code_action_menu.is_some());
        editor.dispatch_builtin("move-down");
        assert!(editor.code_action_menu.is_none());
    }

    #[test]
    fn code_action_dispatch_navigation() {
        use crate::editor::CodeActionItem;
        let mut editor = Editor::new();
        editor.apply_code_action_result_items(vec![
            CodeActionItem {
                title: "A".into(),
                kind: None,
                edit_json: None,
            },
            CodeActionItem {
                title: "B".into(),
                kind: None,
                edit_json: None,
            },
        ]);
        editor.dispatch_builtin("lsp-code-action-next");
        assert_eq!(editor.code_action_menu.as_ref().unwrap().selected, 1);
        editor.dispatch_builtin("lsp-code-action-prev");
        assert_eq!(editor.code_action_menu.as_ref().unwrap().selected, 0);
        editor.dispatch_builtin("lsp-code-action-dismiss");
        assert!(editor.code_action_menu.is_none());
    }

    #[test]
    fn code_action_menu_shows_hint() {
        use crate::editor::CodeActionItem;
        let mut editor = Editor::new();
        editor.apply_code_action_result_items(vec![CodeActionItem {
            title: "Fix".into(),
            kind: None,
            edit_json: None,
        }]);
        assert!(editor.status_msg.contains("j/k navigate"));
        assert!(editor.status_msg.contains("Esc dismiss"));
    }
}
