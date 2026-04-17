use ropey::Rope;
use std::fs;
use std::path::{Path, PathBuf};

use crate::conversation::Conversation;
use crate::help_view::HelpView;
use crate::window::Window;

/// What kind of content this buffer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    /// Normal text editing buffer (backed by rope).
    Text,
    /// AI conversation buffer (backed by structured entries, not rope).
    Conversation,
    /// Rendered preview of org/markdown (read-only).
    Preview,
    /// In-editor log viewer (*Messages* buffer). Read-only, live view.
    Messages,
    /// Knowledge-base viewer (`*Help*`). Body rendered live from the KB.
    Help,
}

/// A single edit operation, stored for undo/redo.
///
/// Emacs lesson: undo.c uses an unbounded cons-list truncated at GC time.
/// We use explicit action records with bounded stacks and standard undo/redo
/// semantics (redo stack cleared on new edit).
#[derive(Debug, Clone)]
pub enum EditAction {
    InsertChar { pos: usize, ch: char },
    DeleteChar { pos: usize, ch: char },
    InsertRange { pos: usize, text: String },
    DeleteRange { pos: usize, text: String },
}

/// Rope-backed text buffer with undo history.
///
/// Emacs lesson: point (cursor) is per-window, not per-buffer. Two windows can
/// view the same buffer at different positions. Cursor state lives on `Window`.
///
/// Design: lean struct, pure state mutation, no I/O dependencies beyond std::fs.
/// All operations are designed to be called programmatically by an AI agent.
pub struct Buffer {
    rope: Rope,
    file_path: Option<PathBuf>,
    pub modified: bool,
    pub name: String,
    pub kind: BufferKind,
    /// Read-only buffers reject all edit operations. Set for Help, Messages.
    pub read_only: bool,
    pub conversation: Option<Conversation>,
    /// Help-buffer navigation state. Present iff `kind == BufferKind::Help`.
    pub help_view: Option<HelpView>,
    undo_stack: Vec<EditAction>,
    redo_stack: Vec<EditAction>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    pub fn new() -> Self {
        Buffer {
            rope: Rope::new(),
            file_path: None,
            modified: false,
            name: String::from("[scratch]"),
            kind: BufferKind::Text,
            read_only: false,
            conversation: None,
            help_view: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Create a conversation buffer (AI interaction pane).
    pub fn new_conversation(name: impl Into<String>) -> Self {
        Buffer {
            rope: Rope::new(),
            file_path: None,
            modified: false,
            name: name.into(),
            kind: BufferKind::Conversation,
            read_only: false,
            conversation: Some(Conversation::new()),
            help_view: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Create a messages buffer (live view of the in-editor log).
    pub fn new_messages() -> Self {
        Buffer {
            rope: Rope::new(),
            file_path: None,
            modified: false,
            name: String::from("*Messages*"),
            kind: BufferKind::Messages,
            read_only: true,
            conversation: None,
            help_view: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Create a help buffer viewing a KB node.
    pub fn new_help(start_node_id: impl Into<String>) -> Self {
        let start = start_node_id.into();
        Buffer {
            rope: Rope::new(),
            file_path: None,
            modified: false,
            name: String::from("*Help*"),
            kind: BufferKind::Help,
            read_only: true,
            conversation: None,
            help_view: Some(HelpView::new(start)),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let content = fs::read_to_string(path)?;
        let rope = Rope::from_str(&content);
        Ok(Buffer {
            rope,
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            file_path: Some(path.to_path_buf()),
            modified: false,
            kind: BufferKind::Text,
            read_only: false,
            conversation: None,
            help_view: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        })
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        if let Some(ref path) = self.file_path {
            fs::write(path, self.rope.to_string())?;
            self.modified = false;
            Ok(())
        } else {
            Err(std::io::Error::other("No file path set"))
        }
    }

    pub fn set_file_path(&mut self, path: PathBuf) {
        self.name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.file_path = Some(path);
    }

    pub fn file_path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Replace the entire buffer contents. Used for read-only/generated buffers
    /// like *Messages*. Clears undo history.
    pub fn replace_contents(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    // --- Text extraction ---

    /// Get the full text of a line, including trailing newline if present.
    pub fn line_text(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        self.rope.line(line).to_string()
    }

    /// Get text in a character range [char_start, char_end).
    pub fn text_range(&self, char_start: usize, char_end: usize) -> String {
        let start = char_start.min(self.rope.len_chars());
        let end = char_end.min(self.rope.len_chars());
        if start >= end {
            return String::new();
        }
        self.rope.slice(start..end).to_string()
    }

    // --- Metrics ---

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line count excluding the phantom empty line that ropey adds after
    /// a trailing newline. Use for display (line numbers, gutter width).
    pub fn display_line_count(&self) -> usize {
        let n = self.rope.len_lines();
        if n > 1 && self.rope.len_chars() > 0 && self.rope.char(self.rope.len_chars() - 1) == '\n' {
            n - 1
        } else {
            n
        }
    }

    /// Length of a line in characters, excluding the trailing newline.
    pub fn line_len(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let line_slice = self.rope.line(line);
        let len = line_slice.len_chars();
        if len > 0 && line_slice.char(len - 1) == '\n' {
            len - 1
        } else {
            len
        }
    }

    /// Char offset in the rope for a given (row, col) position.
    pub fn char_offset_at(&self, row: usize, col: usize) -> usize {
        if self.rope.len_chars() == 0 {
            return 0;
        }
        let row = row.min(self.line_count().saturating_sub(1));
        let line_start = self.rope.line_to_char(row);
        line_start + col
    }

    /// Maximum number of undo entries to retain.
    const MAX_UNDO_ENTRIES: usize = 1000;

    /// Push an edit action onto the undo stack, trimming if it exceeds the bound.
    fn push_undo(&mut self, action: EditAction) {
        self.undo_stack.push(action);
        if self.undo_stack.len() > Self::MAX_UNDO_ENTRIES {
            let excess = self.undo_stack.len() - Self::MAX_UNDO_ENTRIES;
            self.undo_stack.drain(..excess);
        }
    }

    // --- Editing operations ---
    // Each records an EditAction for undo and clears the redo stack.
    // Cursor state is on Window, passed as parameter.

    pub fn insert_char(&mut self, win: &mut Window, ch: char) {
        if self.read_only {
            return;
        }
        let pos = self.char_offset_at(win.cursor_row, win.cursor_col);
        self.rope.insert_char(pos, ch);
        self.push_undo(EditAction::InsertChar { pos, ch });
        self.redo_stack.clear();
        if ch == '\n' {
            win.cursor_row += 1;
            win.cursor_col = 0;
        } else {
            win.cursor_col += 1;
        }
        self.modified = true;
    }

    pub fn delete_char_backward(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        if win.cursor_col == 0 && win.cursor_row == 0 {
            return;
        }
        let pos = self.char_offset_at(win.cursor_row, win.cursor_col);
        if pos == 0 {
            return;
        }
        let ch = self.rope.char(pos - 1);
        let prev_line_len = if ch == '\n' {
            self.line_len(win.cursor_row - 1)
        } else {
            0
        };
        self.rope.remove(pos - 1..pos);
        self.undo_stack
            .push(EditAction::DeleteChar { pos: pos - 1, ch });
        self.redo_stack.clear();
        if ch == '\n' {
            win.cursor_row -= 1;
            win.cursor_col = prev_line_len;
        } else {
            win.cursor_col -= 1;
        }
        self.modified = true;
    }

    pub fn delete_char_forward(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let pos = self.char_offset_at(win.cursor_row, win.cursor_col);
        if pos >= self.rope.len_chars() {
            return;
        }
        let ch = self.rope.char(pos);
        self.rope.remove(pos..pos + 1);
        self.push_undo(EditAction::DeleteChar { pos, ch });
        self.redo_stack.clear();
        self.modified = true;
        win.clamp_cursor(self);
    }

    /// Delete the current line. Returns the deleted text (for yank register).
    pub fn delete_line(&mut self, win: &mut Window) -> String {
        if self.read_only {
            return String::new();
        }
        let line_count = self.line_count();
        if line_count == 0 || self.rope.len_chars() == 0 {
            return String::new();
        }
        let line_start = self.rope.line_to_char(win.cursor_row);
        let line = self.rope.line(win.cursor_row);
        let line_chars = line.len_chars();
        if line_chars == 0 {
            return String::new();
        }
        let text: String = self.rope.slice(line_start..line_start + line_chars).into();
        self.rope.remove(line_start..line_start + line_chars);
        self.push_undo(EditAction::DeleteRange {
            pos: line_start,
            text: text.clone(),
        });
        self.redo_stack.clear();
        self.modified = true;
        win.clamp_cursor(self);
        text
    }

    /// Delete backward to the start of the previous whitespace-delimited token
    /// (readline/bash C-w behaviour). Does NOT cross line boundaries.
    pub fn delete_word_backward(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let cursor = self.char_offset_at(win.cursor_row, win.cursor_col);
        let line_start = self.rope.line_to_char(win.cursor_row);
        if cursor <= line_start {
            return;
        }
        // Walk back over trailing whitespace, then over the word.
        let mut pos = cursor;
        while pos > line_start && self.rope.char(pos - 1).is_whitespace() {
            pos -= 1;
        }
        while pos > line_start && !self.rope.char(pos - 1).is_whitespace() {
            pos -= 1;
        }
        if pos == cursor {
            return;
        }
        let deleted: String = self.rope.slice(pos..cursor).into();
        self.rope.remove(pos..cursor);
        self.push_undo(EditAction::DeleteRange { pos, text: deleted });
        self.redo_stack.clear();
        self.modified = true;
        win.cursor_col = pos - line_start;
    }

    /// Delete from the cursor to the beginning of the current line (C-u).
    pub fn delete_to_line_start(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let cursor = self.char_offset_at(win.cursor_row, win.cursor_col);
        let line_start = self.rope.line_to_char(win.cursor_row);
        if cursor <= line_start {
            return;
        }
        let deleted: String = self.rope.slice(line_start..cursor).into();
        self.rope.remove(line_start..cursor);
        self.push_undo(EditAction::DeleteRange {
            pos: line_start,
            text: deleted,
        });
        self.redo_stack.clear();
        self.modified = true;
        win.cursor_col = 0;
    }

    /// Delete from the cursor to the end of the current line (C-k / kill-line).
    /// Deletes the newline itself only if the line is otherwise empty.
    pub fn delete_to_line_end(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let cursor = self.char_offset_at(win.cursor_row, win.cursor_col);
        let rope = &self.rope;
        let line_end = {
            let line_start = rope.line_to_char(win.cursor_row);
            let line = rope.line(win.cursor_row);
            let raw_end = line_start + line.len_chars();
            // If the line ends with '\n', stop before it (don't kill the newline
            // unless the cursor is already AT the newline).
            if raw_end > line_start && raw_end <= rope.len_chars() && rope.char(raw_end - 1) == '\n'
            {
                if cursor == raw_end - 1 {
                    // Cursor on the newline itself — kill it.
                    raw_end
                } else {
                    raw_end - 1
                }
            } else {
                raw_end
            }
        };
        if cursor >= line_end {
            return;
        }
        let deleted: String = self.rope.slice(cursor..line_end).into();
        self.rope.remove(cursor..line_end);
        self.push_undo(EditAction::DeleteRange {
            pos: cursor,
            text: deleted,
        });
        self.redo_stack.clear();
        self.modified = true;
        win.clamp_cursor(self);
    }

    /// Insert text at an arbitrary character offset. Used by the AI agent.
    pub fn insert_text_at(&mut self, char_offset: usize, text: &str) {
        if self.read_only {
            return;
        }
        let offset = char_offset.min(self.rope.len_chars());
        self.rope.insert(offset, text);
        self.push_undo(EditAction::InsertRange {
            pos: offset,
            text: text.to_string(),
        });
        self.redo_stack.clear();
        self.modified = true;
    }

    /// Delete a character range [start, end). Used by the AI agent.
    pub fn delete_range(&mut self, start: usize, end: usize) {
        if self.read_only {
            return;
        }
        let start = start.min(self.rope.len_chars());
        let end = end.min(self.rope.len_chars());
        if start >= end {
            return;
        }
        let text: String = self.rope.slice(start..end).into();
        self.rope.remove(start..end);
        self.undo_stack
            .push(EditAction::DeleteRange { pos: start, text });
        self.redo_stack.clear();
        self.modified = true;
    }

    pub fn open_line_below(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let line_start = self.rope.line_to_char(win.cursor_row);
        let line = self.rope.line(win.cursor_row);
        let line_chars = line.len_chars();

        let insert_pos = line_start + line_chars;
        self.rope.insert_char(insert_pos, '\n');
        self.push_undo(EditAction::InsertChar {
            pos: insert_pos,
            ch: '\n',
        });
        self.redo_stack.clear();
        win.cursor_row += 1;
        win.cursor_col = 0;
        self.modified = true;
    }

    pub fn open_line_above(&mut self, win: &mut Window) {
        if self.read_only {
            return;
        }
        let line_start = self.rope.line_to_char(win.cursor_row);
        self.rope.insert_char(line_start, '\n');
        self.push_undo(EditAction::InsertChar {
            pos: line_start,
            ch: '\n',
        });
        self.redo_stack.clear();
        win.cursor_col = 0;
        self.modified = true;
    }

    // --- Undo / Redo ---

    pub fn undo(&mut self, win: &mut Window) {
        let action = match self.undo_stack.pop() {
            Some(a) => a,
            None => return,
        };
        match &action {
            EditAction::InsertChar { pos, .. } => {
                self.rope.remove(*pos..*pos + 1);
                Self::set_cursor_from_char_pos(&self.rope, win, *pos);
            }
            EditAction::DeleteChar { pos, ch } => {
                self.rope.insert_char(*pos, *ch);
                Self::set_cursor_from_char_pos(&self.rope, win, *pos + 1);
            }
            EditAction::InsertRange { pos, text } => {
                self.rope.remove(*pos..*pos + text.chars().count());
                Self::set_cursor_from_char_pos(&self.rope, win, *pos);
            }
            EditAction::DeleteRange { pos, text } => {
                self.rope.insert(*pos, text);
                Self::set_cursor_from_char_pos(&self.rope, win, *pos);
            }
        }
        self.redo_stack.push(action);
        self.modified = true;
        win.clamp_cursor(self);
    }

    pub fn redo(&mut self, win: &mut Window) {
        let action = match self.redo_stack.pop() {
            Some(a) => a,
            None => return,
        };
        match &action {
            EditAction::InsertChar { pos, ch } => {
                self.rope.insert_char(*pos, *ch);
                Self::set_cursor_from_char_pos(&self.rope, win, *pos + 1);
            }
            EditAction::DeleteChar { pos, .. } => {
                self.rope.remove(*pos..*pos + 1);
                Self::set_cursor_from_char_pos(&self.rope, win, *pos);
            }
            EditAction::InsertRange { pos, text } => {
                self.rope.insert(*pos, text);
                Self::set_cursor_from_char_pos(&self.rope, win, *pos + text.chars().count());
            }
            EditAction::DeleteRange { pos, text } => {
                let end = *pos + text.chars().count();
                self.rope.remove(*pos..end);
                Self::set_cursor_from_char_pos(&self.rope, win, *pos);
            }
        }
        self.push_undo(action);
        self.modified = true;
        win.clamp_cursor(self);
    }

    /// Set cursor row/col from a char offset in the rope.
    fn set_cursor_from_char_pos(rope: &Rope, win: &mut Window, pos: usize) {
        let pos = pos.min(rope.len_chars());
        win.cursor_row = rope.char_to_line(pos);
        let line_start = rope.line_to_char(win.cursor_row);
        win.cursor_col = pos - line_start;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a buffer + window pair for testing.
    fn new_buf_win() -> (Buffer, Window) {
        (Buffer::new(), Window::new(0, 0))
    }

    /// Helper: insert a string into buffer char by char.
    fn insert_str(buf: &mut Buffer, win: &mut Window, s: &str) {
        for ch in s.chars() {
            buf.insert_char(win, ch);
        }
    }

    // --- Construction ---

    #[test]
    fn new_buffer_is_empty() {
        let (buf, _win) = new_buf_win();
        assert_eq!(buf.text(), "");
        assert!(!buf.modified);
        assert_eq!(buf.name, "[scratch]");
    }

    #[test]
    fn from_file_and_save_round_trip() {
        let dir = std::env::temp_dir().join("mae_test_round_trip");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        fs::write(&path, "hello\nworld\n").unwrap();

        let mut buf = Buffer::from_file(&path).unwrap();
        let mut win = Window::new(0, 0);
        assert_eq!(buf.text(), "hello\nworld\n");
        assert_eq!(buf.name, "test.txt");
        assert!(!buf.modified);

        buf.insert_char(&mut win, '!');
        assert!(buf.modified);
        buf.save().unwrap();
        assert!(!buf.modified);

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "!hello\nworld\n");

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Insert ---

    #[test]
    fn insert_char_at_start() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        assert_eq!(buf.text(), "a");
        assert_eq!(win.cursor_col, 1);
        assert!(buf.modified);
    }

    #[test]
    fn insert_multiple_chars() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'h');
        buf.insert_char(&mut win, 'i');
        assert_eq!(buf.text(), "hi");
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn insert_newline_splits_line() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.insert_char(&mut win, '\n');
        buf.insert_char(&mut win, 'b');
        assert_eq!(buf.text(), "a\nb");
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_col, 1);
    }

    // --- Delete backward ---

    #[test]
    fn delete_backward_at_start_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.delete_char_backward(&mut win);
        assert_eq!(buf.text(), "");
        assert!(!buf.modified);
    }

    #[test]
    fn delete_backward_mid_line() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.insert_char(&mut win, 'b');
        buf.delete_char_backward(&mut win);
        assert_eq!(buf.text(), "a");
        assert_eq!(win.cursor_col, 1);
    }

    #[test]
    fn delete_backward_at_line_start_joins_lines() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "a\nb");
        // move to start of line 1
        win.cursor_col = 0;
        buf.delete_char_backward(&mut win);
        assert_eq!(buf.text(), "ab");
        assert_eq!(win.cursor_row, 0);
        assert_eq!(win.cursor_col, 1);
    }

    // --- Delete forward ---

    #[test]
    fn delete_forward_at_end_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.delete_char_forward(&mut win);
        assert_eq!(buf.text(), "a");
    }

    #[test]
    fn delete_forward_mid_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "ab");
        win.cursor_col = 0;
        buf.delete_char_forward(&mut win);
        assert_eq!(buf.text(), "b");
        assert_eq!(win.cursor_col, 0);
    }

    // --- Delete line ---

    #[test]
    fn delete_line_single_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "ab");
        win.cursor_col = 0;
        buf.delete_line(&mut win);
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn delete_line_middle() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb\nccc\n");
        win.cursor_row = 1;
        win.cursor_col = 0;
        buf.delete_line(&mut win);
        assert_eq!(buf.text(), "aaa\nccc\n");
    }

    #[test]
    fn delete_line_last_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb");
        win.cursor_row = 1;
        win.cursor_col = 0;
        buf.delete_line(&mut win);
        assert_eq!(buf.text(), "aaa\n");
    }

    // --- Movement (now on Window) ---

    #[test]
    fn move_up_at_top_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        win.move_up(&buf);
        assert_eq!(win.cursor_row, 0);
    }

    #[test]
    fn move_down_at_bottom_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        win.cursor_col = 0;
        win.move_down(&buf);
        assert_eq!(win.cursor_row, 0);
    }

    #[test]
    fn move_up_and_down() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb\nccc");
        win.cursor_row = 2;
        win.cursor_col = 0;
        win.move_up(&buf);
        assert_eq!(win.cursor_row, 1);
        win.move_down(&buf);
        assert_eq!(win.cursor_row, 2);
    }

    #[test]
    fn move_down_clamps_col_to_shorter_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "long line\nhi");
        win.cursor_row = 0;
        win.cursor_col = 8;
        win.move_down(&buf);
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn move_left_at_start_is_noop() {
        let (_buf, mut win) = new_buf_win();
        win.move_left();
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn move_right_at_end_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        win.move_right(&buf);
        assert_eq!(win.cursor_col, 1);
    }

    #[test]
    fn move_left_and_right() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "ab");
        win.move_left();
        assert_eq!(win.cursor_col, 1);
        win.move_right(&buf);
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn move_to_line_start_and_end() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello");
        win.move_to_line_start();
        assert_eq!(win.cursor_col, 0);
        win.move_to_line_end(&buf);
        assert_eq!(win.cursor_col, 5);
    }

    #[test]
    fn move_to_first_and_last_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb\nccc");
        win.move_to_first_line(&buf);
        assert_eq!(win.cursor_row, 0);
        win.move_to_last_line(&buf);
        assert_eq!(win.cursor_row, 2);
    }

    // --- Clamp cursor ---

    #[test]
    fn clamp_cursor_after_line_shortening() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello\nhi");
        win.cursor_row = 1;
        win.cursor_col = 10;
        win.clamp_cursor(&buf);
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn clamp_cursor_empty_buffer() {
        let (buf, mut win) = new_buf_win();
        win.cursor_row = 5;
        win.cursor_col = 10;
        win.clamp_cursor(&buf);
        assert_eq!(win.cursor_row, 0);
        assert_eq!(win.cursor_col, 0);
    }

    // --- Scrolling ---

    #[test]
    fn ensure_scroll_cursor_above_viewport() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
        win.scroll_offset = 5;
        win.cursor_row = 2;
        win.ensure_scroll(5);
        assert_eq!(win.scroll_offset, 2);
    }

    #[test]
    fn ensure_scroll_cursor_below_viewport() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
        win.scroll_offset = 0;
        win.cursor_row = 7;
        win.ensure_scroll(5);
        assert_eq!(win.scroll_offset, 3);
    }

    #[test]
    fn ensure_scroll_cursor_within_viewport() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "a\nb\nc\nd\ne");
        win.scroll_offset = 0;
        win.cursor_row = 2;
        win.ensure_scroll(5);
        assert_eq!(win.scroll_offset, 0);
    }

    // --- Open line ---

    #[test]
    fn open_line_below() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb");
        win.cursor_row = 0;
        win.cursor_col = 0;
        buf.open_line_below(&mut win);
        assert_eq!(buf.text(), "aaa\n\nbbb");
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn open_line_above() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb");
        win.cursor_row = 1;
        win.cursor_col = 0;
        buf.open_line_above(&mut win);
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_col, 0);
        assert!(buf.text().contains("aaa\n\nbbb"));
    }

    // --- Undo / Redo ---

    #[test]
    fn undo_insert_char() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.insert_char(&mut win, 'b');
        assert_eq!(buf.text(), "ab");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "a");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn undo_delete_char() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "ab");
        buf.delete_char_backward(&mut win);
        assert_eq!(buf.text(), "a");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "ab");
    }

    #[test]
    fn undo_delete_line() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "aaa\nbbb\n");
        win.cursor_row = 1;
        win.cursor_col = 0;
        buf.delete_line(&mut win);
        assert_eq!(buf.text(), "aaa\n");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "aaa\nbbb\n");
    }

    #[test]
    fn redo_after_undo() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.undo(&mut win);
        assert_eq!(buf.text(), "");
        buf.redo(&mut win);
        assert_eq!(buf.text(), "a");
    }

    #[test]
    fn redo_cleared_on_new_edit() {
        let (mut buf, mut win) = new_buf_win();
        buf.insert_char(&mut win, 'a');
        buf.undo(&mut win);
        buf.insert_char(&mut win, 'b');
        buf.redo(&mut win);
        assert_eq!(buf.text(), "b");
    }

    #[test]
    fn undo_empty_stack_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.undo(&mut win);
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn redo_empty_stack_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        buf.redo(&mut win);
        assert_eq!(buf.text(), "");
    }

    // --- Range operations (AI agent) ---

    #[test]
    fn insert_text_at_beginning() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "world");
        buf.insert_text_at(0, "hello ");
        assert_eq!(buf.text(), "hello world");
    }

    #[test]
    fn insert_text_at_end() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello");
        buf.insert_text_at(5, " world");
        assert_eq!(buf.text(), "hello world");
    }

    #[test]
    fn insert_text_at_undo() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "ab");
        buf.insert_text_at(1, "XY");
        assert_eq!(buf.text(), "aXYb");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "ab");
        buf.redo(&mut win);
        assert_eq!(buf.text(), "aXYb");
    }

    #[test]
    fn delete_range_middle() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello world");
        buf.delete_range(5, 11);
        assert_eq!(buf.text(), "hello");
    }

    #[test]
    fn delete_range_undo() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "abcdef");
        buf.delete_range(2, 4);
        assert_eq!(buf.text(), "abef");
        buf.undo(&mut win);
        assert_eq!(buf.text(), "abcdef");
        buf.redo(&mut win);
        assert_eq!(buf.text(), "abef");
    }

    #[test]
    fn delete_range_empty_is_noop() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "abc");
        buf.delete_range(2, 2);
        assert_eq!(buf.text(), "abc");
    }

    // --- Line metrics ---

    #[test]
    fn line_len_excludes_newline() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "hello\nworld");
        assert_eq!(buf.line_len(0), 5);
        assert_eq!(buf.line_len(1), 5);
    }

    #[test]
    fn line_count_with_trailing_newline() {
        let (mut buf, mut win) = new_buf_win();
        insert_str(&mut buf, &mut win, "a\nb\n");
        assert_eq!(buf.line_count(), 3);
    }

    // --- BufferKind ---

    #[test]
    fn default_kind_is_text() {
        let buf = Buffer::new();
        assert_eq!(buf.kind, BufferKind::Text);
        assert!(buf.conversation.is_none());
    }

    #[test]
    fn conversation_buffer_creation() {
        let buf = Buffer::new_conversation("[conversation]");
        assert_eq!(buf.kind, BufferKind::Conversation);
        assert!(buf.conversation.is_some());
        assert_eq!(buf.name, "[conversation]");
    }

    // --- delete_word_backward (C-w) ---

    #[test]
    fn delete_word_backward_basic() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello world");
        let mut win = Window::new(0, 0);
        win.cursor_col = 11; // end of "world"
        buf.delete_word_backward(&mut win);
        assert_eq!(buf.text(), "hello ");
        assert_eq!(win.cursor_col, 6);
    }

    #[test]
    fn delete_word_backward_strips_trailing_whitespace_first() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "foo   ");
        let mut win = Window::new(0, 0);
        win.cursor_col = 6;
        buf.delete_word_backward(&mut win); // removes "foo   "
        assert_eq!(buf.text(), "");
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn delete_word_backward_at_line_start_is_noop() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello\n");
        let mut win = Window::new(0, 0);
        win.cursor_row = 1;
        win.cursor_col = 0;
        buf.delete_word_backward(&mut win);
        assert_eq!(buf.text(), "hello\n"); // newline not crossed
    }

    // --- delete_to_line_start (C-u) ---

    #[test]
    fn delete_to_line_start_basic() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello world");
        let mut win = Window::new(0, 0);
        win.cursor_col = 5;
        buf.delete_to_line_start(&mut win);
        assert_eq!(buf.text(), " world");
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn delete_to_line_start_at_col0_is_noop() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello");
        let mut win = Window::new(0, 0);
        win.cursor_col = 0;
        buf.delete_to_line_start(&mut win);
        assert_eq!(buf.text(), "hello");
    }

    // --- delete_to_line_end (C-k) ---

    #[test]
    fn delete_to_line_end_basic() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello world\n");
        let mut win = Window::new(0, 0);
        win.cursor_col = 5;
        buf.delete_to_line_end(&mut win);
        assert_eq!(buf.text(), "hello\n");
    }

    #[test]
    fn delete_to_line_end_on_newline_kills_it() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello\nworld\n");
        let mut win = Window::new(0, 0);
        win.cursor_col = 5; // cursor on the '\n' of "hello\n"
        buf.delete_to_line_end(&mut win);
        // kills the newline, joining with next line
        assert_eq!(buf.text(), "helloworld\n");
    }

    #[test]
    fn delete_to_line_end_at_end_is_noop() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "hello\n");
        let mut win = Window::new(0, 0);
        win.cursor_col = 5; // already at '\n'
        buf.delete_to_line_end(&mut win);
        // '\n' is killed when cursor is on it
        assert_eq!(buf.text(), "hello");
    }

    #[test]
    fn file_buffer_is_text_kind() {
        let dir = std::env::temp_dir().join("mae_test_kind");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        fs::write(&path, "hello").unwrap();

        let buf = Buffer::from_file(&path).unwrap();
        assert_eq!(buf.kind, BufferKind::Text);
        assert!(buf.conversation.is_none());

        let _ = fs::remove_dir_all(&dir);
    }
}
