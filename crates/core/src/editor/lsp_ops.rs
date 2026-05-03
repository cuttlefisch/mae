//! Editor-side LSP helpers.
//!
//! These methods translate editor state (current buffer, cursor) into
//! `LspIntent` values and push them onto `pending_lsp_requests`. The
//! binary drains the queue and forwards each intent to `run_lsp_task`.
//!
//! Response handling (hover text, jump-to-definition, references list)
//! is also implemented here so the binary stays thin.

use crate::lsp_intent::{language_id_from_path, path_to_uri, LspIntent};

use super::{CompletionItem, Editor, HoverPopup, LspServerStatus};

/// A span in an LSP response. Mirrors `mae_lsp::protocol::Range` but with
/// the core-friendly type so this module has no dep on the LSP crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspRange {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

/// The kind of a document highlight (read vs write vs text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Text,
    Read,
    Write,
}

/// A highlighted range from `textDocument/documentHighlight`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentHighlightRange {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub kind: HighlightKind,
}

/// A file+range returned by definition/references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
}

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
    /// Compute the LSP context for the focused buffer at the cursor.
    /// Returns (uri, language_id, line, character) — the inputs to every
    /// LSP navigation request. None if the buffer has no file path or
    /// the path has no known language mapping.
    pub fn lsp_context_at_cursor(&self) -> Option<(String, String, u32, u32)> {
        let buf = self.active_buffer();
        let path = buf.file_path()?;
        let language_id = language_id_from_path(path)?;
        let uri = path_to_uri(path);
        let win = self.window_mgr.focused_window();
        Some((
            uri,
            language_id,
            win.cursor_row as u32,
            win.cursor_col as u32,
        ))
    }

    /// Return a status suffix if the LSP server for `language_id` is still
    /// starting. Requests are no longer blocked — they queue in the LSP
    /// command channel and execute once the server finishes initializing.
    fn lsp_starting_suffix(&self, language_id: &str) -> &'static str {
        if let Some(info) = self.lsp_servers.get(language_id) {
            if info.status == LspServerStatus::Starting {
                return " (server starting\u{2026})";
            }
        }
        ""
    }

    /// Queue a `textDocument/definition` request at the cursor.
    /// Sets a status message if no language is detected.
    pub fn lsp_request_definition(&mut self) {
        match self.lsp_context_at_cursor() {
            Some((uri, language_id, line, character)) => {
                let suffix = self.lsp_starting_suffix(&language_id);
                self.pending_lsp_requests.push(LspIntent::GotoDefinition {
                    uri,
                    language_id,
                    line,
                    character,
                });
                self.set_status(format!("[LSP] definition...{}", suffix));
            }
            None => self.set_status("[LSP] no language server for this buffer"),
        }
    }

    /// Queue a `textDocument/references` request at the cursor.
    pub fn lsp_request_references(&mut self) {
        match self.lsp_context_at_cursor() {
            Some((uri, language_id, line, character)) => {
                let suffix = self.lsp_starting_suffix(&language_id);
                self.pending_lsp_requests.push(LspIntent::FindReferences {
                    uri,
                    language_id,
                    line,
                    character,
                    include_declaration: true,
                });
                self.set_status(format!("[LSP] references...{}", suffix));
            }
            None => self.set_status("[LSP] no language server for this buffer"),
        }
    }

    /// Queue a `textDocument/hover` request at the cursor.
    pub fn lsp_request_hover(&mut self) {
        match self.lsp_context_at_cursor() {
            Some((uri, language_id, line, character)) => {
                let suffix = self.lsp_starting_suffix(&language_id);
                self.pending_lsp_requests.push(LspIntent::Hover {
                    uri,
                    language_id,
                    line,
                    character,
                });
                self.set_status(format!("[LSP] hover...{}", suffix));
            }
            None => self.set_status("[LSP] no language server for this buffer"),
        }
    }

    /// Queue a `textDocument/completion` request at the cursor position.
    /// Silently ignored if the buffer has no known language.
    pub fn lsp_request_completion(&mut self) {
        if let Some((uri, language_id, line, character)) = self.lsp_context_at_cursor() {
            self.pending_lsp_requests.push(LspIntent::Completion {
                uri,
                language_id,
                line,
                character,
            });
        }
    }

    /// Store a completion result from the LSP server, making the popup visible.
    pub fn apply_completion_result(&mut self, items: Vec<CompletionItem>) {
        if items.is_empty() {
            self.completion_items.clear();
            self.completion_selected = 0;
            return;
        }
        self.completion_items = items;
        self.completion_selected = 0;
    }

    /// Accept the currently selected completion item — inserts its text at
    /// the cursor, replacing the word prefix that was used to trigger
    /// completion.
    pub fn lsp_accept_completion(&mut self) {
        if self.completion_items.is_empty() {
            return;
        }
        let item = self.completion_items[self.completion_selected].clone();
        // Clear the popup first so downstream state is clean.
        self.completion_items.clear();
        self.completion_selected = 0;

        // Erase the word-prefix already typed, then insert the full item text.
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let row = win.cursor_row;
        let col = win.cursor_col;

        // Find start of the current word (back to non-word char or line start).
        let cursor_offset = self.buffers[idx].char_offset_at(row, col);
        let rope = self.buffers[idx].rope();
        let line_start_offset = rope.line_to_char(row);
        let prefix_start = {
            let mut pos = cursor_offset;
            while pos > line_start_offset {
                let ch = rope.char(pos - 1);
                if ch.is_alphanumeric() || ch == '_' {
                    pos -= 1;
                } else {
                    break;
                }
            }
            pos
        };
        // Replace prefix with insert_text
        if prefix_start < cursor_offset {
            self.buffers[idx].delete_range(prefix_start, cursor_offset);
        }
        let insert = item.insert_text.clone();
        self.buffers[idx].insert_text_at(prefix_start, &insert);
        // Reposition cursor after inserted text
        let inserted_chars = insert.chars().count();
        let new_offset = prefix_start + inserted_chars;
        let rope = self.buffers[idx].rope();
        let new_row = rope.char_to_line(new_offset.min(rope.len_chars().saturating_sub(1)));
        let row_start = rope.line_to_char(new_row);
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = new_row;
        win.cursor_col = new_offset.saturating_sub(row_start);
        win.clamp_cursor(&self.buffers[idx]);
    }

    /// Dismiss the completion popup without inserting anything.
    pub fn lsp_dismiss_completion(&mut self) {
        self.completion_items.clear();
        self.completion_selected = 0;
    }

    /// Select the next completion item.
    pub fn lsp_complete_next(&mut self) {
        if self.completion_items.is_empty() {
            return;
        }
        let len = self.completion_items.len();
        self.completion_selected = (self.completion_selected + 1) % len;
    }

    /// Select the previous completion item.
    pub fn lsp_complete_prev(&mut self) {
        if self.completion_items.is_empty() {
            return;
        }
        let len = self.completion_items.len();
        self.completion_selected = self.completion_selected.checked_sub(1).unwrap_or(len - 1);
    }

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
    fn apply_workspace_edit_json(&mut self, json: &str) {
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

    /// Queue a `textDocument/didOpen` notification for the active buffer
    /// (if it has a file path with a known language).
    pub fn lsp_notify_did_open(&mut self) {
        let buf = self.active_buffer();
        let Some(path) = buf.file_path() else {
            return;
        };
        let Some(language_id) = language_id_from_path(path) else {
            return;
        };
        let uri = path_to_uri(path);
        let text = buf.text();
        self.pending_lsp_requests.push(LspIntent::DidOpen {
            uri,
            language_id,
            text,
        });
    }

    /// Queue a `textDocument/didSave` notification for the active buffer.
    pub fn lsp_notify_did_save(&mut self) {
        let buf = self.active_buffer();
        let Some(path) = buf.file_path() else {
            return;
        };
        let Some(language_id) = language_id_from_path(path) else {
            return;
        };
        let uri = path_to_uri(path);
        let text = Some(buf.text());
        self.pending_lsp_requests.push(LspIntent::DidSave {
            uri,
            language_id,
            text,
        });
    }

    /// Queue a `textDocument/didClose` notification for the buffer at `idx`.
    /// Called before a buffer is removed via `kill-buffer` so the language
    /// server drops its per-file state.
    pub fn lsp_notify_did_close_for_buffer(&mut self, idx: usize) {
        if idx >= self.buffers.len() {
            return;
        }
        let buf = &self.buffers[idx];
        let Some(path) = buf.file_path() else {
            return;
        };
        let Some(language_id) = language_id_from_path(path) else {
            return;
        };
        let uri = path_to_uri(path);
        self.pending_lsp_requests
            .push(LspIntent::DidClose { uri, language_id });
    }

    /// Queue a `textDocument/didChange` notification for the active buffer.
    /// Full-text sync (matches the client's current sync kind handling).
    pub fn lsp_notify_did_change(&mut self) {
        let buf = self.active_buffer();
        let Some(path) = buf.file_path() else {
            return;
        };
        let Some(language_id) = language_id_from_path(path) else {
            return;
        };
        let uri = path_to_uri(path);
        let text = buf.text();
        self.pending_lsp_requests.push(LspIntent::DidChange {
            uri,
            language_id,
            text,
        });
    }

    /// Handle a hover response — show in popup or status bar depending on option.
    pub fn apply_hover_result(&mut self, contents: String) {
        if contents.is_empty() {
            self.set_status("[LSP] no hover info");
            return;
        }
        if self.lsp_hover_popup {
            let win = self.window_mgr.focused_window();
            self.hover_popup = Some(HoverPopup {
                contents,
                anchor_row: win.cursor_row,
                anchor_col: win.cursor_col,
                scroll_offset: 0,
            });
            self.set_status("[LSP] K to scroll, any key to dismiss");
        } else {
            // Collapse newlines and trim for single-line status display.
            let single = contents
                .lines()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            let truncated = if single.chars().count() > 200 {
                let s: String = single.chars().take(197).collect();
                format!("{}...", s)
            } else {
                single
            };
            self.set_status(truncated);
        }
    }

    /// Dismiss the hover popup.
    pub fn dismiss_hover_popup(&mut self) {
        self.hover_popup = None;
    }

    /// Scroll the hover popup down.
    pub fn hover_scroll_down(&mut self) {
        if let Some(ref mut popup) = self.hover_popup {
            popup.scroll_offset += 1;
        }
    }

    /// Scroll the hover popup up.
    pub fn hover_scroll_up(&mut self) {
        if let Some(ref mut popup) = self.hover_popup {
            popup.scroll_offset = popup.scroll_offset.saturating_sub(1);
        }
    }

    /// Handle a definition response.
    /// - Empty list → status "not found".
    /// - Single location, same file → jump cursor to location.
    /// - Single location, other file → note in status (binary does the open).
    /// - Multiple → display count, leave binary to present a picker later.
    ///
    /// Returns the location to open in a new buffer, if any.
    pub fn apply_definition_result(&mut self, locations: Vec<LspLocation>) -> Option<LspLocation> {
        if locations.is_empty() {
            self.set_status("[LSP] definition not found");
            return None;
        }
        if locations.len() > 1 {
            self.set_status(format!("[LSP] {} definition candidates", locations.len()));
            // For now, navigate to the first.
        }
        let loc = locations.into_iter().next().unwrap();
        let current_uri = self.active_buffer().file_path().map(path_to_uri);

        if current_uri.as_deref() == Some(loc.uri.as_str()) {
            // Same file — jump in place.
            let idx = self.active_buffer_idx();
            let line_count = self.buffers[idx].display_line_count();
            let target_row = (loc.range.start_line as usize).min(line_count.saturating_sub(1));
            let target_col = loc.range.start_character as usize;
            let vh = self.viewport_height;
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = target_row;
            win.cursor_col = target_col;
            win.clamp_cursor(&self.buffers[idx]);
            win.scroll_center(vh);
            self.set_status(format!(
                "[LSP] definition at {}:{}",
                target_row + 1,
                target_col + 1
            ));
            None
        } else {
            // Different file — return location so binary can open it.
            self.set_status(format!("[LSP] definition in {}", loc.uri));
            Some(loc)
        }
    }

    /// Handle a references response — display count in status line.
    /// Future: render in a dedicated *LSP References* buffer.
    pub fn apply_references_result(&mut self, locations: Vec<LspLocation>) {
        if locations.is_empty() {
            self.set_status("[LSP] no references found");
        } else {
            self.set_status(format!("[LSP] {} reference(s)", locations.len()));
        }
    }

    /// Queue a `textDocument/documentHighlight` request for the symbol under cursor.
    /// Called on idle timer (~300ms after last cursor move).
    pub fn lsp_request_document_highlight(&mut self) {
        let pos = {
            let win = self.window_mgr.focused_window();
            (win.cursor_row, win.cursor_col)
        };
        // Don't re-request if cursor hasn't moved since last request.
        if self.highlight_last_pos == Some(pos) {
            return;
        }
        self.highlight_last_pos = Some(pos);

        if let Some((uri, language_id, line, character)) = self.lsp_context_at_cursor() {
            // Only request if server is connected.
            if self
                .lsp_servers
                .get(&language_id)
                .map(|s| s.status == LspServerStatus::Connected)
                .unwrap_or(false)
            {
                self.pending_lsp_requests
                    .push(LspIntent::DocumentHighlight {
                        uri,
                        language_id,
                        line,
                        character,
                        generation: self.highlight_generation,
                    });
            }
        }
    }

    /// Store document highlight ranges from the LSP response.
    pub fn apply_document_highlight_result(
        &mut self,
        highlights: Vec<DocumentHighlightRange>,
        generation: u64,
    ) {
        // Only apply if the generation matches (cursor hasn't moved since request).
        if generation == self.highlight_generation {
            self.highlight_ranges = highlights;
        }
    }

    /// Clear highlights and bump generation (called on cursor move).
    pub fn clear_highlights(&mut self) {
        self.highlight_ranges.clear();
        self.highlight_generation = self.highlight_generation.wrapping_add(1);
        self.highlight_last_pos = None;
    }

    /// Show a `*LSP Status*` buffer listing all configured language servers,
    /// their commands, binary discovery status, and connection state.
    pub fn show_lsp_status_buffer(&mut self) {
        let mut body = String::new();
        body.push_str("*LSP Status*\n\n");
        body.push_str(&format!(
            "{:<14} {:<30} {:<12} {}\n",
            "Language", "Command", "Status", "Binary"
        ));
        body.push_str(&format!("{}\n", "─".repeat(72)));

        if self.lsp_servers.is_empty() {
            body.push_str("No LSP servers configured.\n");
        } else {
            let mut langs: Vec<_> = self.lsp_servers.keys().cloned().collect();
            langs.sort();
            for lang in &langs {
                let info = &self.lsp_servers[lang];
                let status_str = match info.status {
                    LspServerStatus::Starting => "Starting",
                    LspServerStatus::Connected => "Connected",
                    LspServerStatus::Failed => "Failed",
                    LspServerStatus::Exited => "Exited",
                };
                let binary_str = if info.binary_found {
                    &info.command
                } else {
                    "not found"
                };
                body.push_str(&format!(
                    "{:<14} {:<30} {:<12} {}\n",
                    lang, info.command, status_str, binary_str
                ));
            }
        }

        // Reuse or create the buffer.
        let existing = self.buffers.iter().position(|b| b.name == "*LSP Status*");
        let idx = if let Some(i) = existing {
            self.buffers[i].replace_contents(&body);
            i
        } else {
            let mut buf = crate::buffer::Buffer::new();
            buf.replace_contents(&body);
            buf.name = "*LSP Status*".into();
            self.buffers.push(buf);
            self.buffers.len() - 1
        };
        self.display_buffer(idx);
        let count = self.lsp_servers.len();
        self.set_status(format!("LSP: {} server(s) configured", count));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::CompletionItem;
    use std::path::PathBuf;

    fn editor_with_file(path: &str, text: &str) -> Editor {
        let mut buf = Buffer::new();
        buf.set_file_path(PathBuf::from(path));
        if !text.is_empty() {
            buf.insert_text_at(0, text);
        }
        Editor::with_buffer(buf)
    }

    #[test]
    fn lsp_context_returns_none_when_no_file_path() {
        let ed = Editor::new();
        assert!(ed.lsp_context_at_cursor().is_none());
    }

    #[test]
    fn lsp_context_rust_file() {
        let ed = editor_with_file("/tmp/test.rs", "fn main() {}\n");
        let ctx = ed.lsp_context_at_cursor();
        assert!(ctx.is_some());
        let (uri, lang, line, ch) = ctx.unwrap();
        assert_eq!(uri, "file:///tmp/test.rs");
        assert_eq!(lang, "rust");
        assert_eq!(line, 0);
        assert_eq!(ch, 0);
    }

    #[test]
    fn lsp_context_unknown_language() {
        let ed = editor_with_file("/tmp/test.xyz", "");
        assert!(ed.lsp_context_at_cursor().is_none());
    }

    #[test]
    fn lsp_request_definition_queues_intent() {
        let mut ed = editor_with_file("/tmp/a.rs", "fn main() {}\n");
        ed.lsp_request_definition();
        assert_eq!(ed.pending_lsp_requests.len(), 1);
        match &ed.pending_lsp_requests[0] {
            LspIntent::GotoDefinition {
                uri, language_id, ..
            } => {
                assert_eq!(uri, "file:///tmp/a.rs");
                assert_eq!(language_id, "rust");
            }
            other => panic!("expected GotoDefinition, got {:?}", other),
        }
    }

    #[test]
    fn lsp_request_references_queues_intent() {
        let mut ed = editor_with_file("/tmp/a.rs", "fn main() {}\n");
        ed.lsp_request_references();
        assert!(matches!(
            ed.pending_lsp_requests[0],
            LspIntent::FindReferences { .. }
        ));
    }

    #[test]
    fn lsp_request_hover_queues_intent() {
        let mut ed = editor_with_file("/tmp/a.rs", "fn main() {}\n");
        ed.lsp_request_hover();
        assert!(matches!(
            ed.pending_lsp_requests[0],
            LspIntent::Hover { .. }
        ));
    }

    #[test]
    fn lsp_request_without_file_sets_status() {
        let mut ed = Editor::new();
        ed.lsp_request_definition();
        assert!(ed.pending_lsp_requests.is_empty());
        assert!(ed.status_msg.contains("no language server"));
    }

    #[test]
    fn lsp_notify_did_open_queues_intent_with_text() {
        let mut ed = editor_with_file("/tmp/a.rs", "hello\nworld\n");
        ed.lsp_notify_did_open();
        assert_eq!(ed.pending_lsp_requests.len(), 1);
        match &ed.pending_lsp_requests[0] {
            LspIntent::DidOpen {
                uri,
                language_id,
                text,
            } => {
                assert_eq!(uri, "file:///tmp/a.rs");
                assert_eq!(language_id, "rust");
                assert!(text.contains("hello"));
                assert!(text.contains("world"));
            }
            other => panic!("expected DidOpen, got {:?}", other),
        }
    }

    #[test]
    fn lsp_notify_did_save_queues_intent() {
        let mut ed = editor_with_file("/tmp/a.rs", "x\n");
        ed.lsp_notify_did_save();
        assert!(matches!(
            ed.pending_lsp_requests[0],
            LspIntent::DidSave { .. }
        ));
    }

    #[test]
    fn lsp_notify_did_change_queues_intent() {
        let mut ed = editor_with_file("/tmp/a.rs", "x\n");
        ed.lsp_notify_did_change();
        assert!(matches!(
            ed.pending_lsp_requests[0],
            LspIntent::DidChange { .. }
        ));
    }

    #[test]
    fn lsp_notify_did_close_queues_intent() {
        let mut ed = editor_with_file("/tmp/a.rs", "x\n");
        ed.lsp_notify_did_close_for_buffer(0);
        assert_eq!(ed.pending_lsp_requests.len(), 1);
        match &ed.pending_lsp_requests[0] {
            LspIntent::DidClose { uri, language_id } => {
                assert_eq!(uri, "file:///tmp/a.rs");
                assert_eq!(language_id, "rust");
            }
            other => panic!("expected DidClose, got {:?}", other),
        }
    }

    #[test]
    fn lsp_notify_did_close_out_of_bounds_is_noop() {
        let mut ed = Editor::new();
        ed.lsp_notify_did_close_for_buffer(42);
        assert!(ed.pending_lsp_requests.is_empty());
    }

    #[test]
    fn lsp_notify_skipped_for_unknown_language() {
        let mut ed = editor_with_file("/tmp/a.xyz", "x\n");
        ed.lsp_notify_did_open();
        assert!(ed.pending_lsp_requests.is_empty());
    }

    #[test]
    fn lsp_notify_skipped_for_unsaved_buffer() {
        let mut ed = Editor::new();
        ed.lsp_notify_did_open();
        assert!(ed.pending_lsp_requests.is_empty());
    }

    #[test]
    fn apply_hover_result_empty_shows_no_info() {
        let mut ed = Editor::new();
        ed.apply_hover_result(String::new());
        assert!(ed.status_msg.contains("no hover"));
    }

    #[test]
    fn apply_hover_result_creates_popup() {
        let mut ed = Editor::new();
        ed.apply_hover_result("fn main()".into());
        assert!(ed.hover_popup.is_some());
        assert_eq!(ed.hover_popup.as_ref().unwrap().contents, "fn main()");
    }

    #[test]
    fn apply_hover_result_collapses_newlines() {
        let mut ed = Editor::new();
        ed.lsp_hover_popup = false; // test status-bar path
        ed.apply_hover_result("fn main()\n  does stuff".into());
        assert_eq!(ed.status_msg, "fn main()   does stuff");
    }

    #[test]
    fn apply_hover_result_truncates_long_text() {
        let mut ed = Editor::new();
        ed.lsp_hover_popup = false; // test status-bar path
        let long: String = "a".repeat(500);
        ed.apply_hover_result(long);
        assert!(ed.status_msg.ends_with("..."));
        assert!(ed.status_msg.chars().count() <= 200);
    }

    #[test]
    fn hover_popup_dismiss() {
        let mut ed = Editor::new();
        ed.apply_hover_result("hello".into());
        assert!(ed.hover_popup.is_some());
        ed.dismiss_hover_popup();
        assert!(ed.hover_popup.is_none());
    }

    #[test]
    fn hover_popup_scroll() {
        let mut ed = Editor::new();
        ed.apply_hover_result("hello\nworld\nfoo\nbar".into());
        assert_eq!(ed.hover_popup.as_ref().unwrap().scroll_offset, 0);
        ed.hover_scroll_down();
        assert_eq!(ed.hover_popup.as_ref().unwrap().scroll_offset, 1);
        ed.hover_scroll_up();
        assert_eq!(ed.hover_popup.as_ref().unwrap().scroll_offset, 0);
        ed.hover_scroll_up(); // doesn't underflow
        assert_eq!(ed.hover_popup.as_ref().unwrap().scroll_offset, 0);
    }

    #[test]
    fn apply_definition_empty_shows_not_found() {
        let mut ed = editor_with_file("/tmp/a.rs", "x\n");
        let result = ed.apply_definition_result(vec![]);
        assert!(result.is_none());
        assert!(ed.status_msg.contains("not found"));
    }

    #[test]
    fn apply_definition_same_file_jumps_cursor() {
        let mut ed = editor_with_file("/tmp/a.rs", "line0\nline1\nline2\n");
        let loc = LspLocation {
            uri: "file:///tmp/a.rs".into(),
            range: LspRange {
                start_line: 2,
                start_character: 1,
                end_line: 2,
                end_character: 4,
            },
        };
        let result = ed.apply_definition_result(vec![loc]);
        assert!(result.is_none());
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 2);
        assert_eq!(ed.window_mgr.focused_window().cursor_col, 1);
    }

    #[test]
    fn apply_definition_other_file_returns_location() {
        let mut ed = editor_with_file("/tmp/a.rs", "x\n");
        let loc = LspLocation {
            uri: "file:///tmp/other.rs".into(),
            range: LspRange {
                start_line: 0,
                start_character: 0,
                end_line: 0,
                end_character: 0,
            },
        };
        let result = ed.apply_definition_result(vec![loc.clone()]);
        assert_eq!(result, Some(loc));
    }

    #[test]
    fn apply_references_empty() {
        let mut ed = Editor::new();
        ed.apply_references_result(vec![]);
        assert!(ed.status_msg.contains("no references"));
    }

    #[test]
    fn apply_references_count() {
        let mut ed = Editor::new();
        let locs = vec![
            LspLocation {
                uri: "file:///a.rs".into(),
                range: LspRange {
                    start_line: 0,
                    start_character: 0,
                    end_line: 0,
                    end_character: 0,
                },
            };
            3
        ];
        ed.apply_references_result(locs);
        assert!(ed.status_msg.contains("3 reference"));
    }

    // --- Completion ---

    fn make_item(label: &str, insert_text: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_string(),
            insert_text: insert_text.to_string(),
            detail: None,
            kind_sigil: 'f',
        }
    }

    #[test]
    fn apply_completion_result_stores_items() {
        let mut ed = Editor::new();
        ed.apply_completion_result(vec![make_item("foo", "foo"), make_item("bar", "bar")]);
        assert_eq!(ed.completion_items.len(), 2);
        assert_eq!(ed.completion_selected, 0);
    }

    #[test]
    fn apply_completion_result_empty_clears_popup() {
        let mut ed = Editor::new();
        ed.apply_completion_result(vec![make_item("foo", "foo")]);
        ed.apply_completion_result(vec![]);
        assert!(ed.completion_items.is_empty());
    }

    #[test]
    fn lsp_dismiss_completion_clears_state() {
        let mut ed = Editor::new();
        ed.apply_completion_result(vec![make_item("foo", "foo")]);
        ed.completion_selected = 0;
        ed.lsp_dismiss_completion();
        assert!(ed.completion_items.is_empty());
        assert_eq!(ed.completion_selected, 0);
    }

    #[test]
    fn lsp_complete_next_wraps() {
        let mut ed = Editor::new();
        ed.apply_completion_result(vec![
            make_item("a", "a"),
            make_item("b", "b"),
            make_item("c", "c"),
        ]);
        ed.lsp_complete_next();
        assert_eq!(ed.completion_selected, 1);
        ed.lsp_complete_next();
        assert_eq!(ed.completion_selected, 2);
        ed.lsp_complete_next(); // wraps to 0
        assert_eq!(ed.completion_selected, 0);
    }

    #[test]
    fn lsp_complete_prev_wraps() {
        let mut ed = Editor::new();
        ed.apply_completion_result(vec![
            make_item("a", "a"),
            make_item("b", "b"),
            make_item("c", "c"),
        ]);
        ed.lsp_complete_prev(); // wraps to 2
        assert_eq!(ed.completion_selected, 2);
    }

    #[test]
    fn lsp_request_completion_queues_intent() {
        let mut ed = editor_with_file("/tmp/a.rs", "fn main() {}\n");
        ed.lsp_request_completion();
        assert_eq!(ed.pending_lsp_requests.len(), 1);
        assert!(matches!(
            ed.pending_lsp_requests[0],
            LspIntent::Completion { .. }
        ));
    }

    #[test]
    fn lsp_request_completion_skipped_for_buffer_without_file() {
        let mut ed = Editor::new();
        ed.lsp_request_completion();
        assert!(ed.pending_lsp_requests.is_empty());
    }

    #[test]
    fn lsp_accept_completion_inserts_text() {
        let mut ed = editor_with_file("/tmp/a.rs", "fn mai\n");
        // Position cursor at end of "mai" (col 6)
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 0;
            win.cursor_col = 6;
        }
        ed.apply_completion_result(vec![make_item("main", "main")]);
        ed.lsp_accept_completion();
        assert_eq!(ed.active_buffer().line_text(0), "fn main\n");
        assert!(ed.completion_items.is_empty());
    }

    #[test]
    fn lsp_accept_completion_noop_when_empty() {
        let mut ed = editor_with_file("/tmp/a.rs", "hello\n");
        ed.lsp_accept_completion(); // must not panic
        assert_eq!(ed.active_buffer().line_text(0), "hello\n");
    }

    #[test]
    fn lsp_status_buffer_empty() {
        let mut ed = Editor::new();
        ed.show_lsp_status_buffer();
        let buf = &ed.buffers[ed.window_mgr.focused_window().buffer_idx];
        assert_eq!(buf.name, "*LSP Status*");
        assert!(buf.text().contains("No LSP servers configured"));
    }

    #[test]
    fn lsp_status_buffer_shows_servers() {
        use crate::editor::{LspServerInfo, LspServerStatus};
        let mut ed = Editor::new();
        ed.lsp_servers.insert(
            "rust".to_string(),
            LspServerInfo {
                status: LspServerStatus::Connected,
                command: "rust-analyzer".into(),
                binary_found: true,
            },
        );
        ed.lsp_servers.insert(
            "python".to_string(),
            LspServerInfo {
                status: LspServerStatus::Failed,
                command: "pylsp".into(),
                binary_found: false,
            },
        );
        ed.show_lsp_status_buffer();
        let buf = &ed.buffers[ed.window_mgr.focused_window().buffer_idx];
        let text = buf.text();
        assert!(text.contains("rust"));
        assert!(text.contains("rust-analyzer"));
        assert!(text.contains("Connected"));
        assert!(text.contains("python"));
        assert!(text.contains("pylsp"));
        assert!(text.contains("Failed"));
        assert!(text.contains("not found"));
    }

    #[test]
    fn lsp_status_buffer_reuses_existing() {
        use crate::editor::{LspServerInfo, LspServerStatus};
        let mut ed = Editor::new();
        ed.show_lsp_status_buffer();
        let initial_count = ed.buffers.len();
        ed.lsp_servers.insert(
            "go".to_string(),
            LspServerInfo {
                status: LspServerStatus::Starting,
                command: "gopls".into(),
                binary_found: true,
            },
        );
        ed.show_lsp_status_buffer();
        assert_eq!(ed.buffers.len(), initial_count); // no new buffer created
        let buf = &ed.buffers[ed.window_mgr.focused_window().buffer_idx];
        assert!(buf.text().contains("gopls"));
    }

    // -----------------------------------------------------------------------
    // Code action menu tests
    // -----------------------------------------------------------------------

    #[test]
    fn code_action_menu_navigation() {
        use crate::editor::CodeActionItem;
        let mut ed = Editor::new();
        ed.apply_code_action_result_items(vec![
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
        assert!(ed.code_action_menu.is_some());
        let menu = ed.code_action_menu.as_ref().unwrap();
        assert_eq!(menu.selected, 0);
        assert_eq!(menu.items.len(), 3);

        ed.code_action_next();
        assert_eq!(ed.code_action_menu.as_ref().unwrap().selected, 1);

        ed.code_action_next();
        assert_eq!(ed.code_action_menu.as_ref().unwrap().selected, 2);

        ed.code_action_next(); // wraps
        assert_eq!(ed.code_action_menu.as_ref().unwrap().selected, 0);

        ed.code_action_prev(); // wraps back
        assert_eq!(ed.code_action_menu.as_ref().unwrap().selected, 2);

        ed.code_action_dismiss();
        assert!(ed.code_action_menu.is_none());
    }

    #[test]
    fn code_action_select_applies_workspace_edit() {
        use crate::editor::CodeActionItem;
        let mut ed = editor_with_file("/tmp/a.rs", "hello world\n");
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
        ed.apply_code_action_result_items(vec![CodeActionItem {
            title: "Replace hello".into(),
            kind: Some("quickfix".into()),
            edit_json: Some(edit_json),
        }]);
        ed.code_action_select();
        let text = ed.active_buffer().text();
        assert!(text.starts_with("goodbye world"));
        assert!(ed.code_action_menu.is_none());
    }

    // -----------------------------------------------------------------------
    // E2E dispatch-level tests
    // -----------------------------------------------------------------------

    #[test]
    fn hover_auto_dismiss_on_motion() {
        let mut ed = editor_with_file("/tmp/a.rs", "fn main() {}\n");
        ed.apply_hover_result("some hover docs".into());
        assert!(ed.hover_popup.is_some());
        // Moving cursor should dismiss via dispatch_builtin auto-dismiss.
        ed.dispatch_builtin("move-down");
        assert!(ed.hover_popup.is_none());
    }

    #[test]
    fn hover_k_again_scrolls_down() {
        let mut ed = editor_with_file("/tmp/a.rs", "fn main() {}\n");
        ed.apply_hover_result("line1\nline2\nline3".into());
        assert!(ed.hover_popup.is_some());
        assert_eq!(ed.hover_popup.as_ref().unwrap().scroll_offset, 0);
        // Pressing K again (lsp-hover) when popup visible scrolls.
        ed.dispatch_builtin("lsp-hover");
        assert!(ed.hover_popup.is_some()); // not dismissed
        assert_eq!(ed.hover_popup.as_ref().unwrap().scroll_offset, 1);
    }

    #[test]
    fn code_action_menu_auto_dismiss_on_motion() {
        use crate::editor::CodeActionItem;
        let mut ed = Editor::new();
        ed.apply_code_action_result_items(vec![CodeActionItem {
            title: "Fix".into(),
            kind: None,
            edit_json: None,
        }]);
        assert!(ed.code_action_menu.is_some());
        ed.dispatch_builtin("move-down");
        assert!(ed.code_action_menu.is_none());
    }

    #[test]
    fn code_action_dispatch_navigation() {
        use crate::editor::CodeActionItem;
        let mut ed = Editor::new();
        ed.apply_code_action_result_items(vec![
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
        ed.dispatch_builtin("lsp-code-action-next");
        assert_eq!(ed.code_action_menu.as_ref().unwrap().selected, 1);
        ed.dispatch_builtin("lsp-code-action-prev");
        assert_eq!(ed.code_action_menu.as_ref().unwrap().selected, 0);
        ed.dispatch_builtin("lsp-code-action-dismiss");
        assert!(ed.code_action_menu.is_none());
    }

    #[test]
    fn toggle_diagnostics_inline_via_dispatch() {
        let mut ed = Editor::new();
        assert!(ed.lsp_diagnostics_inline); // default on
        ed.dispatch_builtin("toggle-lsp-diagnostics-inline");
        assert!(!ed.lsp_diagnostics_inline);
        ed.dispatch_builtin("toggle-lsp-diagnostics-inline");
        assert!(ed.lsp_diagnostics_inline);
    }

    #[test]
    fn lsp_status_via_dispatch() {
        let mut ed = Editor::new();
        let initial = ed.buffers.len();
        ed.dispatch_builtin("lsp-status");
        assert!(ed.buffers.len() > initial);
        let buf = &ed.buffers[ed.window_mgr.focused_window().buffer_idx];
        assert!(buf.name.contains("LSP Status"));
    }

    // -----------------------------------------------------------------------
    // LSP UX: starting-server early return + popup hints
    // -----------------------------------------------------------------------

    #[test]
    fn lsp_request_queued_even_when_server_starting() {
        use crate::editor::{LspServerInfo, LspServerStatus};
        let mut ed = editor_with_file("/tmp/a.rs", "fn main() {}\n");
        ed.lsp_servers.insert(
            "rust".to_string(),
            LspServerInfo {
                status: LspServerStatus::Starting,
                command: "rust-analyzer".into(),
                binary_found: true,
            },
        );
        ed.lsp_request_definition();
        assert_eq!(
            ed.pending_lsp_requests.len(),
            1,
            "should queue even when starting"
        );
        assert!(
            ed.status_msg.contains("server starting"),
            "status should mention server starting"
        );

        ed.lsp_request_hover();
        assert_eq!(ed.pending_lsp_requests.len(), 2);

        ed.lsp_request_references();
        assert_eq!(ed.pending_lsp_requests.len(), 3);
    }

    #[test]
    fn hover_popup_sets_hint_status() {
        let mut ed = Editor::new();
        ed.apply_hover_result("fn main()".into());
        assert!(ed.hover_popup.is_some());
        assert!(ed.status_msg.contains("K to scroll"));
    }

    #[test]
    fn code_action_menu_shows_hint() {
        use crate::editor::CodeActionItem;
        let mut ed = Editor::new();
        ed.apply_code_action_result_items(vec![CodeActionItem {
            title: "Fix".into(),
            kind: None,
            edit_json: None,
        }]);
        assert!(ed.status_msg.contains("j/k navigate"));
        assert!(ed.status_msg.contains("Esc dismiss"));
    }

    // --- Center-on-jump tests ---

    #[test]
    fn apply_definition_same_file_centers_viewport() {
        // Buffer with 100 lines, viewport height 20.
        let text: String = (0..100).map(|i| format!("line{}\n", i)).collect();
        let mut ed = editor_with_file("/tmp/a.rs", &text);
        ed.viewport_height = 20;
        let loc = LspLocation {
            uri: "file:///tmp/a.rs".into(),
            range: LspRange {
                start_line: 50,
                start_character: 0,
                end_line: 50,
                end_character: 4,
            },
        };
        ed.apply_definition_result(vec![loc]);
        // Cursor should be on row 50 and scroll_offset should center it.
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 50);
        assert_eq!(ed.window_mgr.focused_window().scroll_offset, 40);
    }

    // --- Document highlight tests ---

    #[test]
    fn clear_highlights_increments_generation() {
        let mut ed = Editor::new();
        let gen0 = ed.highlight_generation;
        ed.clear_highlights();
        assert_eq!(ed.highlight_generation, gen0 + 1);
    }

    #[test]
    fn clear_highlights_empties_ranges() {
        let mut ed = Editor::new();
        ed.highlight_ranges.push(DocumentHighlightRange {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 5,
            kind: HighlightKind::Read,
        });
        ed.clear_highlights();
        assert!(ed.highlight_ranges.is_empty());
    }

    #[test]
    fn apply_document_highlight_stores_ranges() {
        let mut ed = Editor::new();
        let gen = ed.highlight_generation;
        let highlights = vec![DocumentHighlightRange {
            start_line: 5,
            start_col: 2,
            end_line: 5,
            end_col: 7,
            kind: HighlightKind::Write,
        }];
        ed.apply_document_highlight_result(highlights, gen);
        assert_eq!(ed.highlight_ranges.len(), 1);
        assert_eq!(ed.highlight_ranges[0].kind, HighlightKind::Write);
    }

    #[test]
    fn apply_document_highlight_stale_generation_ignored() {
        let mut ed = Editor::new();
        let highlights = vec![DocumentHighlightRange {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 3,
            kind: HighlightKind::Text,
        }];
        // Apply with a stale generation (gen + 1 != current).
        ed.apply_document_highlight_result(highlights, ed.highlight_generation + 1);
        assert!(ed.highlight_ranges.is_empty());
    }
}
