//! Editor-side LSP helpers.
//!
//! These methods translate editor state (current buffer, cursor) into
//! `LspIntent` values and push them onto `pending_lsp_requests`. The
//! binary drains the queue and forwards each intent to `run_lsp_task`.
//!
//! Response handling (hover text, jump-to-definition, references list)
//! is also implemented here so the binary stays thin.

use crate::lsp_intent::{language_id_from_path, path_to_uri, LspIntent};

use super::Editor;

/// A span in an LSP response. Mirrors `mae_lsp::protocol::Range` but with
/// the core-friendly type so this module has no dep on the LSP crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspRange {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

/// A file+range returned by definition/references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
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

    /// Queue a `textDocument/definition` request at the cursor.
    /// Sets a status message if no language is detected.
    pub fn lsp_request_definition(&mut self) {
        match self.lsp_context_at_cursor() {
            Some((uri, language_id, line, character)) => {
                self.pending_lsp_requests.push(LspIntent::GotoDefinition {
                    uri,
                    language_id,
                    line,
                    character,
                });
                self.set_status("[LSP] definition...");
            }
            None => self.set_status("[LSP] no language server for this buffer"),
        }
    }

    /// Queue a `textDocument/references` request at the cursor.
    pub fn lsp_request_references(&mut self) {
        match self.lsp_context_at_cursor() {
            Some((uri, language_id, line, character)) => {
                self.pending_lsp_requests.push(LspIntent::FindReferences {
                    uri,
                    language_id,
                    line,
                    character,
                    include_declaration: true,
                });
                self.set_status("[LSP] references...");
            }
            None => self.set_status("[LSP] no language server for this buffer"),
        }
    }

    /// Queue a `textDocument/hover` request at the cursor.
    pub fn lsp_request_hover(&mut self) {
        match self.lsp_context_at_cursor() {
            Some((uri, language_id, line, character)) => {
                self.pending_lsp_requests.push(LspIntent::Hover {
                    uri,
                    language_id,
                    line,
                    character,
                });
                self.set_status("[LSP] hover...");
            }
            None => self.set_status("[LSP] no language server for this buffer"),
        }
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

    /// Handle a hover response — display the contents in the status line
    /// (truncated to fit on one line). Multi-line contents are collapsed.
    pub fn apply_hover_result(&mut self, contents: String) {
        if contents.is_empty() {
            self.set_status("[LSP] no hover info");
            return;
        }
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
            let line_count = self.buffers[idx].line_count();
            let target_row = (loc.range.start_line as usize).min(line_count.saturating_sub(1));
            let target_col = loc.range.start_character as usize;
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = target_row;
            win.cursor_col = target_col;
            win.clamp_cursor(&self.buffers[idx]);
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
    fn apply_hover_result_collapses_newlines() {
        let mut ed = Editor::new();
        ed.apply_hover_result("fn main()\n  does stuff".into());
        assert_eq!(ed.status_msg, "fn main()   does stuff");
    }

    #[test]
    fn apply_hover_result_truncates_long_text() {
        let mut ed = Editor::new();
        let long: String = "a".repeat(500);
        ed.apply_hover_result(long);
        assert!(ed.status_msg.ends_with("..."));
        assert!(ed.status_msg.chars().count() <= 200);
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
}
