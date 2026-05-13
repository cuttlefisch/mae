//! LSP completion popup: request, accept, dismiss, navigate.

use crate::lsp_intent::LspIntent;

use super::{CompletionItem, Editor};

impl Editor {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::CompletionItem;
    use crate::lsp_intent::LspIntent;
    use std::path::PathBuf;

    fn editor_with_file(path: &str, text: &str) -> Editor {
        let mut buf = Buffer::new();
        buf.set_file_path(PathBuf::from(path));
        if !text.is_empty() {
            buf.insert_text_at(0, text);
        }
        Editor::with_buffer(buf)
    }

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
}
