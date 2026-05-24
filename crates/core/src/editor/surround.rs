//! vim-surround ports: `ds<char>`, `cs<from><to>`, `yss<char>`, visual `S<char>`.
//!
//! Tim Pope's vim-surround plugin is the de-facto standard for editing
//! paired delimiters. We implement its four core operations directly —
//! no plugin layer, no late-binding. The text-object range finder from
//! `crate::word::text_object_range` is the same engine used by `ci(` /
//! `di"`, so the range semantics match what the user already expects
//! from text objects.
//!
//! *Practical Vim* tip 57 recommends vim-surround as essential workflow;
//! replicating it in-core keeps parity with a vim user's muscle memory.
//!
//! State machine: each command is a char-await (via
//! [`Editor::pending_char_command`]). For the two-char `cs<from><to>`
//! sequence, the first captured char is stashed in
//! [`Editor::pending_surround_from`] and a second await is armed.

use crate::Mode;

use super::Editor;

impl Editor {
    /// Map a surround character to the `(open, close)` delimiter pair
    /// to *insert*. Paired chars normalize to canonical open/close;
    /// quotes are symmetric; unknown chars wrap with themselves.
    fn surround_pair(ch: char) -> (char, char) {
        match ch {
            '(' | ')' | 'b' => ('(', ')'),
            '[' | ']' => ('[', ']'),
            '{' | '}' | 'B' => ('{', '}'),
            '<' | '>' => ('<', '>'),
            '"' | '\'' | '`' => (ch, ch),
            other => (other, other),
        }
    }

    /// `ds<char>` — delete the surrounding delimiter pair around the
    /// cursor (the delims themselves, not the content between).
    pub fn delete_surround(&mut self, ch: char) {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let pos = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
        let Some((start, end)) =
            crate::word::text_object_range(self.buffers[idx].rope(), pos, ch, false)
        else {
            self.set_status(format!("No surrounding {}", ch));
            return;
        };
        if end <= start + 1 {
            return;
        }
        // Delete close first (higher offset), then open — order matters
        // because deletions shift everything after the cut.
        self.buffers[idx].delete_range(end - 1, end);
        self.buffers[idx].delete_range(start, start + 1);
        // Cursor sticks at the old open-delim position (now points at
        // the first inner char).
        let rope = self.buffers[idx].rope();
        let new_row = rope.char_to_line(start.min(rope.len_chars().saturating_sub(1)));
        let line_start = rope.line_to_char(new_row);
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = new_row;
        win.cursor_col = start.saturating_sub(line_start);
        win.clamp_cursor(&self.buffers[idx]);
        self.record_edit("delete-surround");
    }

    /// `cs<from><to>` — replace the surrounding delimiter pair.
    pub fn change_surround(&mut self, from: char, to: char) {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let pos = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
        let Some((start, end)) =
            crate::word::text_object_range(self.buffers[idx].rope(), pos, from, false)
        else {
            self.set_status(format!("No surrounding {}", from));
            return;
        };
        if end <= start + 1 {
            return;
        }
        let (open, close) = Self::surround_pair(to);
        // Replace close first, then open — same reason as delete_surround.
        self.buffers[idx].delete_range(end - 1, end);
        self.buffers[idx].insert_text_at(end - 1, &close.to_string());
        self.buffers[idx].delete_range(start, start + 1);
        self.buffers[idx].insert_text_at(start, &open.to_string());
        self.record_edit("change-surround");
    }

    /// `yss<char>` — wrap the current line's content (excluding its
    /// trailing newline) with the pair for `ch`.
    pub fn surround_line(&mut self, ch: char) {
        let (open, close) = Self::surround_pair(ch);
        let idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let rope = self.buffers[idx].rope();
        let line_start = rope.line_to_char(row);
        let line = rope.line(row);
        let line_len = line.len_chars();
        let end = if line_len > 0 && line.char(line_len - 1) == '\n' {
            line_start + line_len - 1
        } else {
            line_start + line_len
        };
        // Insert close first (avoids shifting the `start` offset).
        self.buffers[idx].insert_text_at(end, &close.to_string());
        self.buffers[idx].insert_text_at(line_start, &open.to_string());
        self.record_edit("surround-line");
    }

    /// Visual mode `S<char>` — wrap the current selection with the
    /// pair for `ch` and return to Normal mode.
    pub fn surround_visual(&mut self, ch: char) {
        let (open, close) = Self::surround_pair(ch);
        let (start, end) = self.visual_selection_range();
        if start >= end {
            self.set_mode(Mode::Normal);
            return;
        }
        let idx = self.active_buffer_idx();
        self.buffers[idx].insert_text_at(end, &close.to_string());
        self.buffers[idx].insert_text_at(start, &open.to_string());
        self.set_mode(Mode::Normal);
        self.record_edit("surround-visual");
    }

    /// `ys{motion}<char>` — wrap the range from the preceding motion
    /// with the delimiter pair for `ch`. Called after
    /// `apply_pending_operator_for_motion` stashes the range in
    /// `pending_surround_range`.
    pub fn surround_motion(&mut self, ch: char) {
        let Some((from, to)) = self.vi.pending_surround_range.take() else {
            return;
        };
        let (open, close) = Self::surround_pair(ch);
        let idx = self.active_buffer_idx();
        // Insert close first (avoids shifting `from`).
        self.buffers[idx].insert_text_at(to, &close.to_string());
        self.buffers[idx].insert_text_at(from, &open.to_string());
        self.record_edit("surround-motion");
    }

    /// Char-await dispatcher for surround commands. Mirrors
    /// [`Editor::dispatch_text_object`] and is called from the key
    /// handler's `pending_char_command` resolution site. Returns true
    /// if the command name was a surround op.
    pub fn dispatch_surround(&mut self, command: &str, ch: char) -> bool {
        match command {
            "delete-surround" => self.delete_surround(ch),
            "change-surround-1" => {
                // First char captured; stash and re-arm for the second.
                self.vi.pending_surround_from = Some(ch);
                self.vi.pending_char_command = Some("change-surround-2".to_string());
            }
            "change-surround-2" => {
                if let Some(from) = self.vi.pending_surround_from.take() {
                    self.change_surround(from, ch);
                }
            }
            "surround-line" => self.surround_line(ch),
            "surround-visual" => self.surround_visual(ch),
            "surround-motion" => self.surround_motion(ch),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn ed_with(text: &str) -> Editor {
        let mut buf = Buffer::new();
        buf.replace_contents(text);
        Editor::with_buffer(buf)
    }

    fn set_cursor(editor: &mut Editor, row: usize, col: usize) {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = row;
        win.cursor_col = col;
    }

    #[test]
    fn delete_surround_parens() {
        let mut editor = ed_with("hello (world)");
        set_cursor(&mut editor, 0, 8); // inside the parens
        editor.delete_surround('(');
        assert_eq!(editor.buffers[0].text(), "hello world");
    }

    #[test]
    fn delete_surround_quotes() {
        let mut editor = ed_with("a \"quoted\" b");
        set_cursor(&mut editor, 0, 5);
        editor.delete_surround('"');
        assert_eq!(editor.buffers[0].text(), "a quoted b");
    }

    #[test]
    fn delete_surround_missing_sets_status() {
        let mut editor = ed_with("plain text");
        set_cursor(&mut editor, 0, 3);
        editor.delete_surround('(');
        assert!(editor.status_msg.contains("No surrounding"));
        assert_eq!(editor.buffers[0].text(), "plain text");
    }

    #[test]
    fn change_surround_parens_to_brackets() {
        let mut editor = ed_with("hello (world)");
        set_cursor(&mut editor, 0, 8);
        editor.change_surround('(', '[');
        assert_eq!(editor.buffers[0].text(), "hello [world]");
    }

    #[test]
    fn change_surround_quotes_to_parens() {
        let mut editor = ed_with("say \"hi\" now");
        set_cursor(&mut editor, 0, 5);
        editor.change_surround('"', '(');
        assert_eq!(editor.buffers[0].text(), "say (hi) now");
    }

    #[test]
    fn surround_line_parens() {
        let mut editor = ed_with("hello");
        set_cursor(&mut editor, 0, 2);
        editor.surround_line('(');
        assert_eq!(editor.buffers[0].text(), "(hello)");
    }

    #[test]
    fn surround_line_preserves_trailing_newline() {
        let mut editor = ed_with("hello\nworld\n");
        set_cursor(&mut editor, 0, 0);
        editor.surround_line('"');
        assert_eq!(editor.buffers[0].text(), "\"hello\"\nworld\n");
    }

    #[test]
    fn change_surround_state_machine() {
        let mut editor = ed_with("x (y) z");
        set_cursor(&mut editor, 0, 3);
        // First char: arms state for second char.
        assert!(editor.dispatch_surround("change-surround-1", '('));
        assert_eq!(editor.vi.pending_surround_from, Some('('));
        assert_eq!(
            editor.vi.pending_char_command.as_deref(),
            Some("change-surround-2")
        );
        // Second char: performs the swap.
        assert!(editor.dispatch_surround("change-surround-2", '['));
        assert_eq!(editor.buffers[0].text(), "x [y] z");
        assert_eq!(editor.vi.pending_surround_from, None);
    }

    #[test]
    fn surround_visual_wraps_selection() {
        let mut editor = ed_with("abcdef");
        // Visual-char: anchor at col 1, cursor at col 3 (selecting "bcd").
        editor.mode = Mode::Visual(crate::VisualType::Char);
        editor.vi.visual_anchor_row = 0;
        editor.vi.visual_anchor_col = 1;
        set_cursor(&mut editor, 0, 3);
        editor.surround_visual('(');
        assert_eq!(editor.buffers[0].text(), "a(bcd)ef");
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn surround_motion_wraps_range() {
        let mut editor = ed_with("hello world");
        // Simulate ys{motion}( wrapping chars 0..5 ("hello") with parens
        editor.vi.pending_surround_range = Some((0, 5));
        editor.surround_motion('(');
        assert_eq!(editor.buffers[0].text(), "(hello) world");
    }

    #[test]
    fn surround_motion_brackets() {
        let mut editor = ed_with("foo bar baz");
        editor.vi.pending_surround_range = Some((4, 7));
        editor.surround_motion('[');
        assert_eq!(editor.buffers[0].text(), "foo [bar] baz");
    }

    #[test]
    fn dispatch_surround_motion() {
        let mut editor = ed_with("test");
        editor.vi.pending_surround_range = Some((0, 4));
        assert!(editor.dispatch_surround("surround-motion", '"'));
        assert_eq!(editor.buffers[0].text(), "\"test\"");
    }

    #[test]
    fn dispatch_surround_unknown_returns_false() {
        let mut editor = Editor::new();
        assert!(!editor.dispatch_surround("not-a-surround", 'x'));
    }
}
