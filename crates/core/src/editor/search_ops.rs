use regex::Regex;

use crate::search::{self, SearchDirection};

use super::Editor;

impl Editor {
    /// Apply ignorecase/smartcase logic to a search pattern.
    fn apply_case_sensitivity(&self, pattern: &str) -> String {
        if self.ignorecase {
            if self.smartcase && pattern.chars().any(|c| c.is_uppercase()) {
                pattern.to_string()
            } else {
                format!("(?i){}", pattern)
            }
        } else {
            pattern.to_string()
        }
    }

    /// Compile the search pattern, cache all matches, and jump to the first result.
    pub fn execute_search(&mut self) {
        let pattern = self.search_input.clone();
        if pattern.is_empty() {
            return;
        }

        let effective = self.apply_case_sensitivity(&pattern);
        match Regex::new(&effective) {
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
                    self.ring_bell();
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
            self.ring_bell();
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
                self.ring_bell();
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
        self.set_mode(crate::Mode::Visual(crate::VisualType::Char));
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
            let effective = self.apply_case_sensitivity(&pattern);
            match Regex::new(&effective) {
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
            let effective = self.apply_case_sensitivity(&pattern);
            match Regex::new(&effective) {
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

    /// Execute a global command (`:g/pattern/cmd` or `:v/pattern/cmd`).
    ///
    /// `:g/pattern/cmd` runs `cmd` on every line matching `pattern`.
    /// `:v/pattern/cmd` runs `cmd` on every line NOT matching `pattern`.
    ///
    /// Supported subcommands: `d` (delete), `s/…/…/` (substitute),
    /// `normal …` (execute normal-mode keys).
    pub(crate) fn execute_global_command(&mut self, cmd: &str) {
        let invert = cmd.starts_with("v/");
        let rest = &cmd[2..];

        // Parse: pattern/subcmd
        let (pattern, subcmd) = match rest.find('/') {
            Some(pos) => (&rest[..pos], rest[pos + 1..].trim()),
            None => {
                self.set_status("Usage: :g/pattern/cmd or :v/pattern/cmd");
                return;
            }
        };

        if pattern.is_empty() {
            self.set_status("Empty pattern");
            return;
        }

        let effective = self.apply_case_sensitivity(pattern);
        let re = match Regex::new(&effective) {
            Ok(r) => r,
            Err(e) => {
                self.set_status(format!("Invalid pattern: {}", e));
                return;
            }
        };

        let idx = self.active_buffer_idx();
        let line_count = self.buffers[idx].line_count();

        // Collect matching line indices.
        let mut matching_lines: Vec<usize> = Vec::new();
        for line_idx in 0..line_count {
            let line_text = self.buffers[idx].line_text(line_idx);
            let matches = re.is_match(line_text.trim_end_matches('\n'));
            if matches != invert {
                matching_lines.push(line_idx);
            }
        }

        if matching_lines.is_empty() {
            self.set_status("Pattern not found");
            self.ring_bell();
            return;
        }

        let match_count = matching_lines.len();

        if subcmd == "d" {
            // Delete matching lines (iterate in reverse for stable indices).
            self.buffers[idx].begin_undo_group();
            for &line_idx in matching_lines.iter().rev() {
                let line_start = self.buffers[idx].rope().line_to_char(line_idx);
                let line_end = if line_idx + 1 < self.buffers[idx].line_count() {
                    self.buffers[idx].rope().line_to_char(line_idx + 1)
                } else {
                    self.buffers[idx].rope().len_chars()
                };
                if line_start < line_end {
                    self.buffers[idx].delete_range(line_start, line_end);
                }
            }
            self.buffers[idx].end_undo_group();
            self.set_status(format!("{} lines deleted", match_count));
            let win = self.window_mgr.focused_window_mut();
            win.clamp_cursor(&self.buffers[idx]);
        } else if subcmd.starts_with("s/") {
            // Per-line substitute on matching lines.
            let sub = match search::parse_substitute(subcmd) {
                Ok(s) => s,
                Err(e) => {
                    self.set_status(format!("Substitute error: {}", e));
                    return;
                }
            };
            let sub_re = match Regex::new(&sub.pattern) {
                Ok(r) => r,
                Err(e) => {
                    self.set_status(format!("Invalid substitute pattern: {}", e));
                    return;
                }
            };
            let mut total_subs = 0;
            let mut lines_changed = 0;
            // Process in reverse for stable offsets.
            for &line_idx in matching_lines.iter().rev() {
                let line_start = self.buffers[idx].rope().line_to_char(line_idx);
                let line_text = self.buffers[idx].line_text(line_idx);
                let line_content = line_text.trim_end_matches('\n');
                let (new_text, count) =
                    search::substitute_line(line_content, &sub_re, &sub.replacement, sub.global);
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
            } else {
                self.set_status("No substitutions made");
            }
        } else if let Some(normal_keys) = subcmd.strip_prefix("normal ") {
            // Execute normal-mode key sequence on each matching line.
            // We position the cursor on each line, then dispatch each key as a command.
            // For simplicity, we process in reverse to keep line indices stable.
            for &line_idx in matching_lines.iter().rev() {
                let win = self.window_mgr.focused_window_mut();
                win.cursor_row =
                    line_idx.min(self.buffers[idx].display_line_count().saturating_sub(1));
                win.cursor_col = 0;
                // Dispatch each char as a normal-mode command name.
                for ch in normal_keys.chars() {
                    let cmd_name = match ch {
                        'd' => "delete-line",
                        'x' => "delete-char",
                        'A' => "append-end-of-line",
                        'I' => "insert-beginning-of-line",
                        '0' => "move-line-start",
                        '$' => "move-line-end",
                        'J' => "join-lines",
                        _ => continue,
                    };
                    self.dispatch_builtin(cmd_name);
                }
            }
            self.set_status(format!("{} lines processed", match_count));
        } else {
            self.set_status(format!("Unsupported :g subcommand: {}", subcmd));
        }
    }

    /// Execute a substitute command (`:s/old/new/g` or `:%s/old/new/g`).
    /// Parse a vim-style line address relative to the current cursor row.
    /// Supports: `.` (current), `$` (last), digits, `+N`, `-N`, `.+N`, `.-N`.
    fn parse_line_address(&self, addr: &str) -> Option<usize> {
        let addr = addr.trim();
        let idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let last_line = self.buffers[idx].line_count().saturating_sub(1);

        if addr == "." {
            return Some(cursor_row);
        }
        if addr == "$" {
            return Some(last_line);
        }
        // Relative: +N, -N, .+N, .-N (check before pure number since +2 parses as usize)
        let rel = addr.strip_prefix('.').unwrap_or(addr);
        if let Some(offset) = rel.strip_prefix('+') {
            let n: usize = offset.parse().ok()?;
            return Some((cursor_row + n).min(last_line));
        }
        if let Some(offset) = rel.strip_prefix('-') {
            let n: usize = offset.parse().ok()?;
            return Some(cursor_row.saturating_sub(n));
        }
        // Pure number (1-indexed absolute line number)
        if let Ok(n) = addr.parse::<usize>() {
            return Some(n.saturating_sub(1).min(last_line));
        }
        None
    }

    /// Parse a range prefix like `1,5`, `.,+5`, `%` from the beginning of an
    /// ex command. Returns `(start_line, end_line_exclusive, rest_of_cmd)`.
    /// If no range is found, returns None.
    pub(crate) fn parse_ex_range<'a>(&self, cmd: &'a str) -> Option<(usize, usize, &'a str)> {
        // % prefix (whole buffer)
        if let Some(rest) = cmd.strip_prefix('%') {
            let idx = self.active_buffer_idx();
            return Some((0, self.buffers[idx].line_count(), rest));
        }

        // Find the 's/' that starts the substitute command to know where range ends.
        // Range is everything before 's/'.
        let s_pos = cmd.find("s/")?;
        if s_pos == 0 {
            return None; // No range prefix
        }
        let range_str = &cmd[..s_pos];
        let rest = &cmd[s_pos..];

        // Split on comma
        let parts: Vec<&str> = range_str.splitn(2, ',').collect();
        match parts.len() {
            1 => {
                let line = self.parse_line_address(parts[0])?;
                Some((line, line + 1, rest))
            }
            2 => {
                let start = self.parse_line_address(parts[0])?;
                let end = self.parse_line_address(parts[1])?;
                Some((start, end + 1, rest))
            }
            _ => None,
        }
    }

    pub(crate) fn execute_substitute_command(&mut self, cmd: &str) {
        self.execute_substitute_with_range(cmd, None);
    }

    pub(crate) fn execute_substitute_with_range(
        &mut self,
        cmd: &str,
        range: Option<(usize, usize)>,
    ) {
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

        let (start_line, end_line) = if let Some((s, e)) = range {
            (s, e)
        } else if sub.whole_buffer {
            (0, self.buffers[idx].line_count())
        } else {
            (win_row, win_row + 1)
        };

        let mut total_subs = 0;
        let mut lines_changed = 0;

        // Group all substitute edits into a single undo entry.
        let multi_line = end_line - start_line > 1;
        if multi_line {
            self.buffers[idx].begin_undo_group();
        }

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

        if multi_line {
            self.buffers[idx].end_undo_group();
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
            self.ring_bell();
        }
    }
}
