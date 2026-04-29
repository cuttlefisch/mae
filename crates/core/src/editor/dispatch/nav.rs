use super::super::Editor;

impl Editor {
    /// Dispatch navigation, movement, scroll, search, jump, and mark commands.
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_nav(
        &mut self,
        name: &str,
        count: Option<usize>,
        n: usize,
    ) -> Option<bool> {
        match name {
            "move-up" => {
                let idx = self.active_buffer_idx();
                if self.buffers[idx].kind == crate::BufferKind::Messages {
                    let win = self.window_mgr.focused_window_mut();
                    for _ in 0..n {
                        win.scroll_offset = win.scroll_offset.saturating_sub(1);
                        win.cursor_row = win.cursor_row.saturating_sub(1);
                    }
                } else {
                    let buf = &self.buffers[idx];
                    for _ in 0..n {
                        self.window_mgr.focused_window_mut().move_up(buf);
                    }
                }
            }
            "move-down" => {
                let idx = self.active_buffer_idx();
                if self.buffers[idx].kind == crate::BufferKind::Messages {
                    let total = self.message_log.len();
                    let vh = self.viewport_height;
                    let max = total.saturating_sub(vh);
                    let line_count = self.buffers[idx].line_count().saturating_sub(1);
                    let win = self.window_mgr.focused_window_mut();
                    for _ in 0..n {
                        win.scroll_offset = (win.scroll_offset + 1).min(max);
                        win.cursor_row = (win.cursor_row + 1).min(line_count);
                    }
                } else {
                    let buf = &self.buffers[idx];
                    for _ in 0..n {
                        self.window_mgr.focused_window_mut().move_down(buf);
                    }
                }
            }
            "move-left" => {
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_left();
                }
            }
            "move-right" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_right(buf);
                }
            }
            "move-to-line-start" => {
                self.window_mgr.focused_window_mut().move_to_line_start();
            }
            "move-to-line-end" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr.focused_window_mut().move_to_line_end(buf);
            }
            "move-display-down" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let tw = self.text_area_width;
                if !self.word_wrap || tw == 0 {
                    for _ in 0..n {
                        self.window_mgr.focused_window_mut().move_down(buf);
                    }
                } else {
                    let sb_w = self.show_break.chars().count();
                    let bi = self.break_indent;
                    let win = self.window_mgr.focused_window_mut();
                    for _ in 0..n {
                        let line = buf.rope().line(win.cursor_row);
                        let lt: String = line.chars().collect();
                        let lt = lt.trim_end_matches('\n');
                        let (wrap_row, _) =
                            crate::wrap::wrap_cursor_position(lt, win.cursor_col, tw, bi, sb_w);
                        let total_rows = crate::wrap::wrap_line_display_rows(lt, tw, bi, sb_w);
                        if wrap_row + 1 < total_rows {
                            let next_start =
                                crate::wrap::wrap_row_start_col(lt, wrap_row + 1, tw, bi, sb_w);
                            win.cursor_col = next_start;
                        } else {
                            win.move_down(buf);
                            win.cursor_col = 0;
                        }
                    }
                }
            }
            "move-display-up" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let tw = self.text_area_width;
                if !self.word_wrap || tw == 0 {
                    for _ in 0..n {
                        self.window_mgr.focused_window_mut().move_up(buf);
                    }
                } else {
                    let sb_w = self.show_break.chars().count();
                    let bi = self.break_indent;
                    let win = self.window_mgr.focused_window_mut();
                    for _ in 0..n {
                        let line = buf.rope().line(win.cursor_row);
                        let lt: String = line.chars().collect();
                        let lt = lt.trim_end_matches('\n');
                        let (wrap_row, _) =
                            crate::wrap::wrap_cursor_position(lt, win.cursor_col, tw, bi, sb_w);
                        if wrap_row > 0 {
                            let prev_start =
                                crate::wrap::wrap_row_start_col(lt, wrap_row - 1, tw, bi, sb_w);
                            win.cursor_col = prev_start;
                        } else if win.cursor_row > 0 {
                            win.move_up(buf);
                            let prev_line = buf.rope().line(win.cursor_row);
                            let plt: String = prev_line.chars().collect();
                            let plt = plt.trim_end_matches('\n');
                            let prev_rows = crate::wrap::wrap_line_display_rows(plt, tw, bi, sb_w);
                            if prev_rows > 1 {
                                let last_start = crate::wrap::wrap_row_start_col(
                                    plt,
                                    prev_rows - 1,
                                    tw,
                                    bi,
                                    sb_w,
                                );
                                win.cursor_col = last_start;
                            }
                        }
                    }
                }
            }
            "move-display-line-start" => {
                let tw = self.text_area_width;
                if !self.word_wrap || tw == 0 {
                    self.window_mgr.focused_window_mut().move_to_line_start();
                } else {
                    let buf = &self.buffers[self.active_buffer_idx()];
                    let sb_w = self.show_break.chars().count();
                    let bi = self.break_indent;
                    let win = self.window_mgr.focused_window_mut();
                    let line = buf.rope().line(win.cursor_row);
                    let lt: String = line.chars().collect();
                    let lt = lt.trim_end_matches('\n');
                    let (wrap_row, _) =
                        crate::wrap::wrap_cursor_position(lt, win.cursor_col, tw, bi, sb_w);
                    win.cursor_col = crate::wrap::wrap_row_start_col(lt, wrap_row, tw, bi, sb_w);
                }
            }
            "move-display-line-end" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let tw = self.text_area_width;
                if !self.word_wrap || tw == 0 {
                    self.window_mgr.focused_window_mut().move_to_line_end(buf);
                } else {
                    let sb_w = self.show_break.chars().count();
                    let bi = self.break_indent;
                    let win = self.window_mgr.focused_window_mut();
                    let line = buf.rope().line(win.cursor_row);
                    let lt: String = line.chars().collect();
                    let lt = lt.trim_end_matches('\n');
                    let line_len = lt.chars().count();
                    let (wrap_row, _) =
                        crate::wrap::wrap_cursor_position(lt, win.cursor_col, tw, bi, sb_w);
                    let total_rows = crate::wrap::wrap_line_display_rows(lt, tw, bi, sb_w);
                    let end = if wrap_row + 1 < total_rows {
                        crate::wrap::wrap_row_start_col(lt, wrap_row + 1, tw, bi, sb_w)
                            .saturating_sub(1)
                    } else {
                        line_len.saturating_sub(1)
                    };
                    win.cursor_col = end;
                }
            }
            "move-to-first-line" => {
                self.record_jump();
                let idx = self.active_buffer_idx();
                let kind = self.buffers[idx].kind;
                if kind == crate::BufferKind::Messages {
                    let win = self.window_mgr.focused_window_mut();
                    win.scroll_offset = 0;
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                } else {
                    let buf = &self.buffers[idx];
                    if let Some(target) = count {
                        let row = (target.saturating_sub(1))
                            .min(buf.display_line_count().saturating_sub(1));
                        self.window_mgr.focused_window_mut().cursor_row = row;
                        self.window_mgr.focused_window_mut().clamp_cursor(buf);
                    } else {
                        self.window_mgr.focused_window_mut().move_to_first_line(buf);
                    }
                }
            }
            "move-to-last-line" => {
                self.record_jump();
                let idx = self.active_buffer_idx();
                let kind = self.buffers[idx].kind;
                if kind == crate::BufferKind::Messages {
                    let total = self.message_log.len();
                    let vh = self.viewport_height;
                    let last_line = self.buffers[idx].line_count().saturating_sub(1);
                    let win = self.window_mgr.focused_window_mut();
                    win.scroll_offset = total.saturating_sub(vh);
                    win.cursor_row = last_line;
                    win.cursor_col = 0;
                } else {
                    let buf = &self.buffers[idx];
                    if let Some(target) = count {
                        let row = (target.saturating_sub(1))
                            .min(buf.display_line_count().saturating_sub(1));
                        self.window_mgr.focused_window_mut().cursor_row = row;
                        self.window_mgr.focused_window_mut().clamp_cursor(buf);
                    } else {
                        self.window_mgr.focused_window_mut().move_to_last_line(buf);
                    }
                }
            }
            "move-word-forward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_word_forward(buf);
                }
            }
            "move-word-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_word_backward(buf);
                }
            }
            "move-word-end" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_word_end(buf);
                }
            }
            "move-big-word-forward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_big_word_forward(buf);
                }
            }
            "move-big-word-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_big_word_backward(buf);
                }
            }
            "move-big-word-end" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_big_word_end(buf);
                }
            }
            "move-word-end-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_word_end_backward(buf);
                }
            }
            "move-big-word-end-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_big_word_end_backward(buf);
                }
            }
            "move-to-first-non-blank" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr
                    .focused_window_mut()
                    .move_to_first_non_blank(buf);
            }
            "move-line-next-non-blank" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let win = self.window_mgr.focused_window_mut();
                for _ in 0..n {
                    win.move_down(buf);
                }
                win.move_to_first_non_blank(buf);
            }
            "move-line-prev-non-blank" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let win = self.window_mgr.focused_window_mut();
                for _ in 0..n {
                    win.move_up(buf);
                }
                win.move_to_first_non_blank(buf);
            }
            "move-matching-bracket" => {
                self.record_jump();
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr
                    .focused_window_mut()
                    .move_matching_bracket(buf);
            }
            "move-paragraph-forward" => {
                self.record_jump();
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_paragraph_forward(buf);
                }
            }
            "move-paragraph-backward" => {
                self.record_jump();
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_paragraph_backward(buf);
                }
            }
            "find-char-forward-await" => {
                self.pending_char_command = Some("find-char-forward".to_string());
                self.pending_char_count = n;
            }
            "find-char-backward-await" => {
                self.pending_char_command = Some("find-char-backward".to_string());
                self.pending_char_count = n;
            }
            "till-char-forward-await" => {
                self.pending_char_command = Some("till-char-forward".to_string());
                self.pending_char_count = n;
            }
            "till-char-backward-await" => {
                self.pending_char_command = Some("till-char-backward".to_string());
                self.pending_char_count = n;
            }

            // Scroll commands
            "scroll-half-up" | "scroll-page-up" => {
                let idx = self.active_buffer_idx();
                let kind = self.buffers[idx].kind;
                let vh = self.viewport_height;
                let is_half = name == "scroll-half-up";
                let amount = if is_half { vh / 2 } else { vh };
                match kind {
                    crate::BufferKind::Messages => {
                        let win = self.window_mgr.focused_window_mut();
                        for _ in 0..n {
                            win.scroll_offset = win.scroll_offset.saturating_sub(amount);
                            win.cursor_row = win.cursor_row.saturating_sub(amount);
                        }
                    }
                    _ => {
                        if is_half {
                            for _ in 0..n {
                                self.window_mgr.focused_window_mut().scroll_half_up(vh);
                            }
                        } else {
                            for _ in 0..n {
                                self.window_mgr.focused_window_mut().scroll_page_up(vh);
                            }
                        }
                    }
                }
            }
            "scroll-half-down" | "scroll-page-down" => {
                let idx = self.active_buffer_idx();
                let kind = self.buffers[idx].kind;
                let vh = self.viewport_height;
                let is_half = name == "scroll-half-down";
                let amount = if is_half { vh / 2 } else { vh };
                match kind {
                    crate::BufferKind::Messages => {
                        let total = self.message_log.len();
                        let line_count = self.buffers[idx].line_count().saturating_sub(1);
                        let win = self.window_mgr.focused_window_mut();
                        let max = total.saturating_sub(vh);
                        for _ in 0..n {
                            win.scroll_offset = (win.scroll_offset + amount).min(max);
                            win.cursor_row = (win.cursor_row + amount).min(line_count);
                        }
                    }
                    _ => {
                        let buf = &self.buffers[idx];
                        if is_half {
                            for _ in 0..n {
                                self.window_mgr
                                    .focused_window_mut()
                                    .scroll_half_down(buf, vh);
                            }
                        } else {
                            for _ in 0..n {
                                self.window_mgr
                                    .focused_window_mut()
                                    .scroll_page_down(buf, vh);
                            }
                        }
                    }
                }
            }
            "scroll-down-line" => {
                let idx = self.active_buffer_idx();
                let kind = self.buffers[idx].kind;
                match kind {
                    crate::BufferKind::Messages => {
                        let total = self.message_log.len();
                        let vh = self.viewport_height;
                        let win = self.window_mgr.focused_window_mut();
                        let max = total.saturating_sub(vh);
                        for _ in 0..n {
                            win.scroll_offset = (win.scroll_offset + 1).min(max);
                        }
                    }
                    _ => {
                        let buf = &self.buffers[idx];
                        let vh = self.viewport_height;
                        for _ in 0..n {
                            self.window_mgr
                                .focused_window_mut()
                                .scroll_down_line(buf, vh);
                        }
                    }
                }
            }
            "scroll-up-line" => {
                let idx = self.active_buffer_idx();
                let kind = self.buffers[idx].kind;
                match kind {
                    crate::BufferKind::Messages => {
                        let win = self.window_mgr.focused_window_mut();
                        for _ in 0..n {
                            win.scroll_offset = win.scroll_offset.saturating_sub(1);
                        }
                    }
                    _ => {
                        let vh = self.viewport_height;
                        let has_folds = !self.buffers[idx].folded_ranges.is_empty();
                        let needs_wrapped = (self.word_wrap && self.text_area_width > 0)
                            || has_folds
                            || self.heading_scale;
                        for _ in 0..n {
                            if needs_wrapped {
                                // Pre-compute visual rows for the viewport range so
                                // the closure doesn't need &self (borrow conflict).
                                let buf = &self.buffers[idx];
                                let max_line = buf.display_line_count();
                                let scroll = self.window_mgr.focused_window().scroll_offset;
                                // Pre-compute for lines around scroll_offset ± viewport.
                                let range_start = scroll.saturating_sub(1);
                                let range_end = (scroll + vh + 2).min(max_line);
                                let mut row_cache: Vec<(usize, usize)> =
                                    Vec::with_capacity(range_end - range_start + 1);
                                for l in range_start..range_end {
                                    row_cache.push((l, self.line_visual_rows(idx, l)));
                                }
                                let buf = &self.buffers[idx];
                                self.window_mgr.focused_window_mut().scroll_up_line_wrapped(
                                    buf,
                                    vh,
                                    |line| {
                                        // Look up from pre-computed cache; fallback to 1.
                                        row_cache
                                            .iter()
                                            .find(|(l, _)| *l == line)
                                            .map(|(_, r)| *r)
                                            .unwrap_or(1)
                                    },
                                );
                            } else {
                                let buf = &self.buffers[idx];
                                self.window_mgr.focused_window_mut().scroll_up_line(buf, vh);
                            }
                        }
                    }
                }
            }
            "scroll-center" => {
                let vh = self.viewport_height;
                self.window_mgr.focused_window_mut().scroll_center(vh);
            }
            "scroll-top" => {
                self.window_mgr.focused_window_mut().scroll_cursor_top();
            }
            "scroll-bottom" => {
                let vh = self.viewport_height;
                self.window_mgr
                    .focused_window_mut()
                    .scroll_cursor_bottom(vh);
            }
            // Screen-relative cursor
            "move-screen-top" => {
                self.window_mgr.focused_window_mut().move_to_screen_top();
            }
            "move-screen-middle" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let vh = self.viewport_height;
                self.window_mgr
                    .focused_window_mut()
                    .move_to_screen_middle(buf, vh);
            }
            "move-screen-bottom" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let vh = self.viewport_height;
                self.window_mgr
                    .focused_window_mut()
                    .move_to_screen_bottom(buf, vh);
            }

            // Repeat f/F/t/T
            "repeat-find" => {
                if let Some((ch, ref cmd)) = self.last_find_char.clone() {
                    self.pending_char_count = n;
                    self.dispatch_char_motion(cmd, ch);
                }
            }
            "repeat-find-reverse" => {
                if let Some((ch, ref cmd)) = self.last_find_char.clone() {
                    let reversed = match cmd.as_str() {
                        "find-char-forward" => "find-char-backward",
                        "find-char-backward" => "find-char-forward",
                        "till-char-forward" => "till-char-backward",
                        "till-char-backward" => "till-char-forward",
                        _ => return Some(true),
                    };
                    self.pending_char_count = n;
                    self.dispatch_char_motion(reversed, ch);
                }
            }

            // Reselect last visual selection (gv)
            "reselect-visual" => {
                if let Some((ar, ac, cr, cc, vtype)) = self.last_visual {
                    self.visual_anchor_row = ar;
                    self.visual_anchor_col = ac;
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = cr;
                    win.cursor_col = cc;
                    self.set_mode(crate::Mode::Visual(vtype));
                }
            }

            // Search
            "search-forward-start" => {
                self.search_state.direction = crate::SearchDirection::Forward;
                self.search_input.clear();
                self.set_mode(crate::Mode::Search);
            }
            "search-backward-start" => {
                self.search_state.direction = crate::SearchDirection::Backward;
                self.search_input.clear();
                self.set_mode(crate::Mode::Search);
            }
            "search-next" => {
                self.record_jump();
                for _ in 0..n {
                    self.jump_to_next_match(true);
                }
            }
            "search-prev" => {
                self.record_jump();
                for _ in 0..n {
                    self.jump_to_next_match(false);
                }
            }
            "search-word-under-cursor" => {
                self.record_jump();
                self.search_word_at_cursor();
            }
            "search-word-under-cursor-backward" => {
                self.record_jump();
                self.search_word_at_cursor_backward();
            }
            "clear-search-highlight" | "nohlsearch" => {
                self.search_state.highlight_active = false;
            }
            "visual-select-next-match" => {
                self.record_jump();
                self.visual_select_match(true);
            }
            "visual-select-prev-match" => {
                self.record_jump();
                self.visual_select_match(false);
            }
            "search-buffer" => {
                self.dispatch_builtin("search-forward-start");
                return Some(true);
            }

            // Marks
            "set-mark-await" => {
                self.pending_char_command = Some("set-mark".to_string());
            }
            "jump-mark-await" => {
                self.pending_char_command = Some("jump-mark".to_string());
            }

            // Jump list
            "jump-backward" => self.jump_backward(n),
            "jump-forward" => self.jump_forward(n),

            // Change list navigation
            "change-backward" => self.change_backward(n),
            "change-forward" => self.change_forward(n),

            _ => return None,
        }
        Some(true)
    }
}
