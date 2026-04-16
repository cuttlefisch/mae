use regex::Regex;

use crate::search::{self, SearchDirection};

use super::Editor;

impl Editor {
    /// Compile the search pattern, cache all matches, and jump to the first result.
    pub fn execute_search(&mut self) {
        let pattern = self.search_input.clone();
        if pattern.is_empty() {
            return;
        }

        match Regex::new(&pattern) {
            Ok(re) => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let matches = search::find_all(buf.rope(), &re);
                let match_count = matches.len();

                self.search_state.pattern = pattern;
                self.search_state.regex = Some(re);
                self.search_state.matches = matches;
                self.search_state.highlight_active = true;

                if match_count > 0 {
                    self.jump_to_next_match(true);
                } else {
                    self.set_status("Pattern not found");
                }
            }
            Err(e) => {
                self.set_status(format!("Invalid regex: {}", e));
            }
        }
    }

    /// Recompute search matches after buffer edit (call when buffer changes).
    pub fn recompute_search_matches(&mut self) {
        if let Some(ref re) = self.search_state.regex {
            let buf = &self.buffers[self.active_buffer_idx()];
            self.search_state.matches = search::find_all(buf.rope(), re);
        }
    }

    /// Navigate to the next/prev match. `same_direction` = true means n, false means N.
    pub(crate) fn jump_to_next_match(&mut self, same_direction: bool) {
        let re = match self.search_state.regex {
            Some(ref re) => re.clone(),
            None => {
                self.set_status("No previous search");
                return;
            }
        };

        let direction = if same_direction {
            self.search_state.direction
        } else {
            match self.search_state.direction {
                SearchDirection::Forward => SearchDirection::Backward,
                SearchDirection::Backward => SearchDirection::Forward,
            }
        };

        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window();
        let char_offset = buf.char_offset_at(win.cursor_row, win.cursor_col);

        if let Some(m) = search::find_next(buf.rope(), &re, char_offset, direction, true) {
            let rope = buf.rope();
            let new_row = rope.char_to_line(m.start);
            let line_start = rope.line_to_char(new_row);
            let new_col = m.start - line_start;

            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = new_col;

            // Show "N of M" status
            let matches = &self.search_state.matches;
            let idx = matches
                .iter()
                .position(|sm| sm.start == m.start)
                .map(|i| i + 1)
                .unwrap_or(0);
            self.set_status(format!("[{}/{}]", idx, matches.len()));
        } else {
            self.set_status("Pattern not found");
        }
    }

    /// Select the match at-or-adjacent to the cursor in the given direction,
    /// entering Visual Char mode with the match highlighted. Implements gn/gN
    /// from Practical Vim tip 86.
    ///
    /// Returns true if a match was selected, false if no regex or no match.
    pub(crate) fn visual_select_match(&mut self, forward: bool) -> bool {
        let re = match self.search_state.regex {
            Some(ref re) => re.clone(),
            None => {
                self.set_status("No previous search");
                return false;
            }
        };

        let direction = if forward {
            SearchDirection::Forward
        } else {
            SearchDirection::Backward
        };

        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window();
        let char_offset = buf.char_offset_at(win.cursor_row, win.cursor_col);

        let m = match search::find_match_at_or_adjacent(buf.rope(), &re, char_offset, direction) {
            Some(m) => m,
            None => {
                self.set_status("Pattern not found");
                return false;
            }
        };

        // Set visual anchor to match start, cursor to match end - 1 (inclusive).
        let rope = buf.rope();
        let anchor_row = rope.char_to_line(m.start);
        let anchor_col = m.start - rope.line_to_char(anchor_row);
        let end_inclusive = m.end.saturating_sub(1).max(m.start);
        let cursor_row = rope.char_to_line(end_inclusive);
        let cursor_col = end_inclusive - rope.line_to_char(cursor_row);

        self.visual_anchor_row = anchor_row;
        self.visual_anchor_col = anchor_col;
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = cursor_row;
        win.cursor_col = cursor_col;
        self.mode = crate::Mode::Visual(crate::VisualType::Char);
        true
    }

    /// Search for word under cursor (* command).
    pub(crate) fn search_word_at_cursor(&mut self) {
        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window();
        let char_offset = buf.char_offset_at(win.cursor_row, win.cursor_col);

        if let Some(pattern) = search::word_at_offset(buf.rope(), char_offset) {
            self.search_input = pattern.clone();
            self.search_state.direction = SearchDirection::Forward;
            match Regex::new(&pattern) {
                Ok(re) => {
                    let matches = search::find_all(buf.rope(), &re);
                    self.search_state.pattern = pattern;
                    self.search_state.regex = Some(re);
                    self.search_state.matches = matches;
                    self.search_state.highlight_active = true;
                    self.jump_to_next_match(true);
                }
                Err(e) => {
                    self.set_status(format!("Invalid regex: {}", e));
                }
            }
        } else {
            self.set_status("No word under cursor");
        }
    }

    /// Search for word under cursor backward (# command).
    pub(crate) fn search_word_at_cursor_backward(&mut self) {
        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window();
        let char_offset = buf.char_offset_at(win.cursor_row, win.cursor_col);

        if let Some(pattern) = search::word_at_offset(buf.rope(), char_offset) {
            self.search_input = pattern.clone();
            self.search_state.direction = SearchDirection::Backward;
            match Regex::new(&pattern) {
                Ok(re) => {
                    let matches = search::find_all(buf.rope(), &re);
                    self.search_state.pattern = pattern;
                    self.search_state.regex = Some(re);
                    self.search_state.matches = matches;
                    self.search_state.highlight_active = true;
                    self.jump_to_next_match(true);
                }
                Err(e) => {
                    self.set_status(format!("Invalid regex: {}", e));
                }
            }
        } else {
            self.set_status("No word under cursor");
        }
    }

    /// Execute a substitute command (`:s/old/new/g` or `:%s/old/new/g`).
    pub(crate) fn execute_substitute_command(&mut self, cmd: &str) {
        let sub = match search::parse_substitute(cmd) {
            Ok(s) => s,
            Err(e) => {
                self.set_status(format!("Substitute error: {}", e));
                return;
            }
        };

        let re = match Regex::new(&sub.pattern) {
            Ok(r) => r,
            Err(e) => {
                self.set_status(format!("Invalid pattern: {}", e));
                return;
            }
        };

        let idx = self.active_buffer_idx();
        let win_row = self.window_mgr.focused_window().cursor_row;

        let (start_line, end_line) = if sub.whole_buffer {
            (0, self.buffers[idx].line_count())
        } else {
            (win_row, win_row + 1)
        };

        let mut total_subs = 0;
        let mut lines_changed = 0;

        // Process lines in reverse so char offsets remain stable
        for line_idx in (start_line..end_line).rev() {
            let line_start = self.buffers[idx].rope().line_to_char(line_idx);
            let line_text = self.buffers[idx].line_text(line_idx);
            let line_content = line_text.trim_end_matches('\n');

            let (new_text, count) =
                search::substitute_line(line_content, &re, &sub.replacement, sub.global);
            if count > 0 {
                total_subs += count;
                lines_changed += 1;
                let end_offset = line_start + line_content.chars().count();
                self.buffers[idx].delete_range(line_start, end_offset);
                self.buffers[idx].insert_text_at(line_start, &new_text);
            }
        }

        if total_subs > 0 {
            self.set_status(format!(
                "{} substitution{} on {} line{}",
                total_subs,
                if total_subs == 1 { "" } else { "s" },
                lines_changed,
                if lines_changed == 1 { "" } else { "s" }
            ));
            self.recompute_search_matches();
        } else {
            self.set_status("Pattern not found");
        }
    }
}
