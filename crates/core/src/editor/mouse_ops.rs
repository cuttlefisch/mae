use crate::window::WindowId;

impl super::Editor {
    pub fn handle_mouse_click(
        &mut self,
        row: usize,
        col: usize,
        button: crate::input::MouseButton,
    ) {
        self.handle_mouse_click_inner(row, col, button, false);
    }

    /// Handle a mouse click with optional shift modifier for selection extension.
    pub fn handle_mouse_click_shift(
        &mut self,
        row: usize,
        col: usize,
        button: crate::input::MouseButton,
        shift_held: bool,
    ) {
        self.handle_mouse_click_inner(row, col, button, shift_held);
    }

    fn handle_mouse_click_inner(
        &mut self,
        row: usize,
        col: usize,
        button: crate::input::MouseButton,
        shift_held: bool,
    ) {
        use crate::input::MouseButton;

        // Dismiss stale popups on any mouse click.
        self.lsp.hover_popup = None;
        self.lsp.code_action_menu = None;

        // Shell buffers: route to pending_shell_click for the binary to drain.
        // Subtract window border offset (1 row top, 1 col left).
        let active = self.active_buffer_idx();
        if self.buffers[active].kind == crate::BufferKind::Shell {
            let shell_row = row.saturating_sub(1);
            let shell_col = col.saturating_sub(1);
            self.shell.click = Some((shell_row, shell_col, button));
            return;
        }

        match button {
            MouseButton::Left => {
                // Place cursor at clicked position, adjusting for gutter and scroll.
                let win = self.window_mgr.focused_window();
                let buf = &self.buffers[win.buffer_idx];
                let line_count = buf.rope().len_lines();
                let digits = if line_count == 0 {
                    1
                } else {
                    (line_count as f64).log10().floor() as usize + 1
                };
                let gutter_width = if self.show_line_numbers {
                    digits.max(2) + 1
                } else {
                    0
                };
                if col < gutter_width {
                    return; // Clicked in gutter, ignore
                }
                let text_col = col.saturating_sub(gutter_width);
                // row 0 is the window border in GUI mode; buffer content starts at row 1.
                let buf_row = win.scroll_offset + row.saturating_sub(1);
                let max_row = line_count.saturating_sub(1);
                let target_row = buf_row.min(max_row);

                // Click counting: same position within 400ms increments count
                let click_count =
                    if let Some((prev_time, prev_row, prev_col, prev_count)) = self.last_click {
                        if prev_row == target_row
                            && prev_col == text_col
                            && prev_time.elapsed() < std::time::Duration::from_millis(400)
                        {
                            if prev_count >= 3 {
                                1
                            } else {
                                prev_count + 1
                            }
                        } else {
                            1
                        }
                    } else {
                        1
                    };
                self.last_click =
                    Some((std::time::Instant::now(), target_row, text_col, click_count));

                // --- Shift-click: extend or start selection ---
                if shift_held {
                    let buf = &self.buffers[self.window_mgr.focused_window().buffer_idx];
                    let line_len = buf.line_len(target_row);
                    let target_col = text_col.min(if line_len > 0 { line_len - 1 } else { 0 });

                    if !matches!(self.mode, crate::Mode::Visual(_)) {
                        // Start new visual selection from current cursor to click pos
                        let cur_row = self.window_mgr.focused_window().cursor_row;
                        let cur_col = self.window_mgr.focused_window().cursor_col;
                        self.vi.visual_anchor_row = cur_row;
                        self.vi.visual_anchor_col = cur_col;
                        self.set_mode(crate::Mode::Visual(crate::VisualType::Char));
                    }
                    // Move cursor to click position (anchor stays)
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = target_row;
                    win.cursor_col = target_col;
                    return;
                }

                // --- Triple-click: select line ---
                if click_count == 3 {
                    self.vi.visual_anchor_row = target_row;
                    self.vi.visual_anchor_col = 0;
                    self.set_mode(crate::Mode::Visual(crate::VisualType::Line));
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = target_row;
                    win.cursor_col = 0;
                    return;
                }

                // --- Double-click: try link following, then word select ---
                if click_count == 2 {
                    // Clamp to the clicked line's content. The raw text_col can
                    // exceed the line length — a click past end-of-line, or in the
                    // right pane of a split window where the screen column far
                    // overruns a short line — which would push char_offset_at past
                    // the rope and panic in word_start_backward.
                    let click_col = {
                        let buf = &self.buffers[self.window_mgr.focused_window().buffer_idx];
                        let line_len = buf.line_len(target_row);
                        text_col.min(if line_len > 0 { line_len - 1 } else { 0 })
                    };

                    // Try link following first (existing behavior)
                    if self.try_link_follow_at(target_row, click_col) {
                        return;
                    }

                    // No link — select word at cursor
                    let buf = &self.buffers[self.window_mgr.focused_window().buffer_idx];
                    let offset = buf.char_offset_at(target_row, click_col);
                    let word_start = crate::word::word_start_backward(buf.rope(), offset);
                    let word_end = crate::word::word_end_forward(buf.rope(), offset);
                    // word_end is inclusive (on last char of word)
                    if word_start <= word_end {
                        let (start_row, start_col) = buf.row_col_from_offset(word_start);
                        let (end_row, end_col) = buf.row_col_from_offset(word_end);
                        self.vi.visual_anchor_row = start_row;
                        self.vi.visual_anchor_col = start_col;
                        self.set_mode(crate::Mode::Visual(crate::VisualType::Char));
                        let win = self.window_mgr.focused_window_mut();
                        win.cursor_row = end_row;
                        win.cursor_col = end_col;
                    }
                    return;
                }

                // Single-click: just position cursor
                let buf = &self.buffers[self.window_mgr.focused_window().buffer_idx];
                let line_len = buf.line_len(target_row);
                let target_col = text_col.min(if line_len > 0 { line_len - 1 } else { 0 });
                // Exit visual mode on single click
                if matches!(self.mode, crate::Mode::Visual(_)) {
                    self.set_mode(crate::Mode::Normal);
                }
                let win = self.window_mgr.focused_window_mut();
                win.cursor_row = target_row;
                win.cursor_col = target_col;
            }
            MouseButton::Right => {
                // Right-click could open context menu in the future.
            }
            MouseButton::Middle => {
                // Middle-click paste from default register.
                self.dispatch_builtin("paste-after");
            }
        }
    }

    /// Attempt link following at (row, col). Returns true if a link was found and followed.
    fn try_link_follow_at(&mut self, target_row: usize, text_col: usize) -> bool {
        // Check display regions (concealed links in markup buffers).
        let buf = &self.buffers[self.window_mgr.focused_window().buffer_idx];
        if !buf.display_regions.is_empty() {
            let line_byte_start = buf.rope().char_to_byte(buf.rope().line_to_char(target_row));
            let cursor_byte = line_byte_start + {
                let line_str: String = buf
                    .rope()
                    .line(target_row)
                    .chars()
                    .filter(|c| *c != '\n' && *c != '\r')
                    .collect();
                line_str
                    .char_indices()
                    .nth(text_col)
                    .map(|(b, _)| b)
                    .unwrap_or(line_str.len())
            };
            if let Some(region) = buf
                .display_regions
                .iter()
                .find(|r| cursor_byte >= r.byte_start && cursor_byte < r.byte_end)
            {
                if let Some(ref target) = region.link_target {
                    let target = target.clone();
                    self.handle_link_click(&target);
                    return true;
                }
            }
        }

        // Check pre-populated link_spans
        let buf = &self.buffers[self.window_mgr.focused_window().buffer_idx];
        if !buf.link_spans.is_empty() {
            let line_start_byte = buf.rope().char_to_byte(buf.rope().line_to_char(target_row));
            let click_byte = line_start_byte + text_col;
            if let Some(link) = buf
                .link_spans
                .iter()
                .find(|s| click_byte >= s.byte_start && click_byte < s.byte_end)
            {
                let target = link.target.clone();
                self.handle_link_click(&target);
                return true;
            }
        }

        // On-the-fly link detection
        let buf = &self.buffers[self.window_mgr.focused_window().buffer_idx];
        if target_row < buf.rope().len_lines() {
            let line_text: String = buf.rope().line(target_row).chars().collect();
            let links = crate::link_detect::detect_links(&line_text);
            for link in &links {
                let link_char_start = line_text[..link.byte_start].chars().count();
                let link_char_end = line_text[..link.byte_end].chars().count();
                if text_col >= link_char_start && text_col < link_char_end {
                    let target = link.target.clone();
                    self.handle_link_click(&target);
                    return true;
                }
            }
        }

        false
    }

    /// Handle a link click: open file paths in the editor, URLs externally.
    pub(crate) fn handle_link_click(&mut self, target: &str) {
        if target.starts_with("http://") || target.starts_with("https://") {
            // Open URL externally
            let _ = std::process::Command::new("xdg-open").arg(target).spawn();
            self.set_status(format!("Opening {}", target));
        } else {
            // Parse :line:col suffix from file paths
            let (path, line, col) = super::file_ops::parse_file_link(target);
            self.open_file(path);
            // Navigate to line:col if specified
            if let Some(ln) = line {
                let buf = &self.buffers[self.active_buffer_idx()];
                let target_row = ln
                    .saturating_sub(1)
                    .min(buf.display_line_count().saturating_sub(1));
                let target_col = col.unwrap_or(1).saturating_sub(1);
                let win = self.window_mgr.focused_window_mut();
                win.cursor_row = target_row;
                win.cursor_col = target_col;
            }
        }
    }

    /// Handle mouse drag — update cursor position and enter/update Visual mode.
    ///
    /// On first drag event, the click position becomes the visual anchor.
    /// Subsequent drag events update the cursor, extending the selection.
    pub fn handle_mouse_drag(&mut self, row: usize, col: usize) {
        // Shell buffers: route to pending_shell_drag for selection update.
        let active = self.active_buffer_idx();
        if self.buffers[active].kind == crate::BufferKind::Shell {
            let shell_row = row.saturating_sub(1);
            let shell_col = col.saturating_sub(1);
            self.shell.drag = Some((shell_row, shell_col));
            return;
        }

        let win = self.window_mgr.focused_window();
        let buf = &self.buffers[win.buffer_idx];
        let line_count = buf.rope().len_lines();
        let digits = if line_count == 0 {
            1
        } else {
            (line_count as f64).log10().floor() as usize + 1
        };
        let gutter_width = if self.show_line_numbers {
            digits.max(2) + 1
        } else {
            0
        };
        let text_col = col.saturating_sub(gutter_width);
        let buf_row = win.scroll_offset + row.saturating_sub(1);
        let max_row = buf.display_line_count().saturating_sub(1);
        let target_row = buf_row.min(max_row);
        let line_len = buf.line_len(target_row);
        let target_col = text_col.min(if line_len > 0 { line_len - 1 } else { 0 });

        // Enter Visual mode on first drag if not already in it.
        if !matches!(self.mode, crate::Mode::Visual(_)) {
            // Anchor at current cursor position (the click position).
            let win = self.window_mgr.focused_window();
            self.vi.visual_anchor_row = win.cursor_row;
            self.vi.visual_anchor_col = win.cursor_col;
            self.set_mode(crate::Mode::Visual(crate::VisualType::Char));
        }

        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = target_row;
        win.cursor_col = target_col;
    }

    /// Handle mouse button release at the given cell coordinates.
    ///
    /// For shell buffers, finalizes text selection and copies to registers.
    /// For text buffers, this is a no-op (Visual mode persists until Esc).
    pub fn handle_mouse_release(&mut self, row: usize, col: usize) {
        let active = self.active_buffer_idx();
        if self.buffers[active].kind == crate::BufferKind::Shell {
            let shell_row = row.saturating_sub(1);
            let shell_col = col.saturating_sub(1);
            self.shell.release = Some((shell_row, shell_col));
        }
    }

    /// Handle horizontal mouse scroll (positive = right, negative = left).
    ///
    /// Adjusts col_offset directly. Only applies to normal file buffers.
    /// Clamped so the rightmost character is still visible.
    pub fn handle_mouse_scroll_horizontal(&mut self, delta: i16) {
        let cols = delta.unsigned_abs() as usize;
        if cols == 0 {
            return;
        }
        let scroll_speed = self.scroll_speed;
        let buf_idx = self.active_buffer_idx();
        let kind = self.buffers[buf_idx].kind;
        if kind != crate::BufferKind::Text {
            return;
        }

        // Find the longest visible line to clamp horizontal scroll.
        // Only scan the viewport region to avoid O(n) on large files.
        let buf = &self.buffers[buf_idx];
        let max_line_width = {
            let rope = buf.rope();
            let total = rope.len_lines();
            let win = self.window_mgr.focused_window();
            let start = win.scroll_offset;
            let end = (start + self.focused_viewport_height() + 1).min(total);
            let mut max_w = 0usize;
            for i in start..end {
                let line = rope.line(i);
                let w = line.chars().filter(|c| *c != '\n' && *c != '\r').count();
                if w > max_w {
                    max_w = w;
                }
            }
            max_w
        };

        let win = self.window_mgr.focused_window_mut();
        if delta > 0 {
            win.col_offset = win.col_offset.saturating_add(cols * scroll_speed);
        } else {
            win.col_offset = win.col_offset.saturating_sub(cols * scroll_speed);
        }
        // Clamp: don't scroll past the rightmost character.
        let max_offset = max_line_width.saturating_sub(1);
        if win.col_offset > max_offset {
            win.col_offset = max_offset;
        }
    }

    /// Handle mouse scroll (positive = up, negative = down).
    ///
    /// Vim-style: scroll moves the viewport and clamps the cursor into the
    /// visible area, so `ensure_scroll` on the next frame is a no-op.
    /// Perform background housekeeping when the editor is idle (~100ms no input).
    /// Called from the event loop's IdleTick handler.
    pub fn idle_work(&mut self) {
        // 1. Reparse dirty non-visible buffers (tree-sitter incremental).
        //    Visible buffers are reparsed during render; this catches background ones.
        let pending: Vec<usize> = self.syntax_reparse_pending.drain().collect();
        for buf_idx in pending {
            if buf_idx < self.buffers.len() {
                let gen = self.buffers[buf_idx].generation;
                let source: String = self.buffers[buf_idx].rope().chars().collect();
                let _ = self.syntax.spans_for(buf_idx, &source, gen);
            }
        }

        // 2. Write swap files for dirty buffers.
        if self.swap_file {
            let custom_dir = if self.swap_directory.is_empty() {
                None
            } else {
                Some(std::path::Path::new(&self.swap_directory))
            };
            for buf in &self.buffers {
                if buf.modified {
                    if let Some(path) = buf.file_path() {
                        let _ = crate::swap::write_swap(path, buf.rope(), custom_dir);
                    }
                }
            }
        }

        // 3. Poll pending async git diff results.
        self.poll_pending_git_diff();

        // 4. Drain KB file watchers for federated instances.
        self.drain_kb_watchers();
    }

    /// Switch focus to whichever window contains the given cell coordinates.
    /// Returns `true` if focus actually changed.
    pub fn focus_window_at(&mut self, col: u16, row: u16) -> bool {
        let area = self.last_layout_area;
        if let Some(win_id) = self.window_mgr.window_at_cell(col, row, area) {
            if win_id != self.window_mgr.focused_id() {
                self.window_mgr.set_focused(win_id);
                return true;
            }
        }
        false
    }

    /// Scroll a specific window without permanently changing focus.
    pub fn handle_mouse_scroll_in_window(&mut self, target_id: WindowId, delta: i16) {
        let original = self.window_mgr.focused_id();
        self.window_mgr.set_focused(target_id);
        self.handle_mouse_scroll(delta);
        self.window_mgr.set_focused(original);
    }

    /// Horizontal-scroll a specific window without permanently changing focus.
    pub fn handle_mouse_scroll_horizontal_in_window(&mut self, target_id: WindowId, delta: i16) {
        let original = self.window_mgr.focused_id();
        self.window_mgr.set_focused(target_id);
        self.handle_mouse_scroll_horizontal(delta);
        self.window_mgr.set_focused(original);
    }

    pub fn handle_mouse_scroll(&mut self, delta: i16) {
        let lines = delta.unsigned_abs() as usize;
        if lines == 0 {
            return;
        }
        let scroll_speed = self.scroll_speed;
        let buf_idx = self.active_buffer_idx();
        let kind = self.buffers[buf_idx].kind;

        match kind {
            crate::BufferKind::Shell => {
                let amount = if delta > 0 {
                    lines as i32 * scroll_speed as i32
                } else {
                    -(lines as i32 * scroll_speed as i32)
                };
                let prev = self.shell.scroll.unwrap_or(0);
                self.shell.scroll = Some(prev + amount);
            }
            crate::BufferKind::Messages => {
                let total = self.message_log.len();
                let vh = self.focused_viewport_height();
                let win = self.window_mgr.focused_window_mut();
                if delta > 0 {
                    win.scroll_offset = win.scroll_offset.saturating_sub(lines * scroll_speed);
                } else {
                    let max = total.saturating_sub(vh);
                    win.scroll_offset = (win.scroll_offset + lines * scroll_speed).min(max);
                }
            }
            _ => {
                let buf_line_count = self.buffers[buf_idx].display_line_count();
                let viewport_height = self.focused_viewport_height();
                let steps = lines * scroll_speed;

                // Phase 1: Sub-line-aware scroll stepping.
                // Pre-compute visual rows for the range around scroll_offset.
                {
                    let scroll = self.window_mgr.focused_window().scroll_offset;
                    let range_start = scroll.saturating_sub(2);
                    let range_end = (scroll + viewport_height + 2).min(buf_line_count);
                    self.populate_visual_rows_cache(buf_idx, range_start, range_end);
                    let (cache_rows, cache_line_start) =
                        match &self.buffers[buf_idx].visual_rows_cache {
                            Some(c) => (c.rows.clone(), c.line_start),
                            None => (Vec::new(), 0),
                        };
                    let lvr = |line: usize| -> usize {
                        if line >= cache_line_start && line < cache_line_start + cache_rows.len() {
                            let v = cache_rows[line - cache_line_start] as usize;
                            if v > 0 {
                                v
                            } else {
                                1
                            }
                        } else {
                            1
                        }
                    };

                    let cell_h = self.gui_cell_height;
                    let buf = &self.buffers[buf_idx];
                    let win = self.window_mgr.focused_window_mut();
                    if delta > 0 {
                        // Scroll up (reveal lines above).
                        for _ in 0..steps {
                            if win.scroll_pixel_offset >= cell_h {
                                win.scroll_pixel_offset -= cell_h;
                            } else {
                                let prev = buf.prev_visible_line(win.scroll_offset);
                                if prev >= win.scroll_offset {
                                    break;
                                }
                                let prev_rows = lvr(prev);
                                win.scroll_offset = prev;
                                win.scroll_pixel_offset = if prev_rows > 1 {
                                    (prev_rows as f32 - 2.0) * cell_h
                                } else {
                                    0.0
                                };
                                if win.scroll_pixel_offset < 0.0 {
                                    win.scroll_pixel_offset = 0.0;
                                }
                            }
                        }
                    } else {
                        // Scroll down (hide lines above).
                        let max_scroll = buf_line_count.saturating_sub(viewport_height);
                        for _ in 0..steps {
                            if win.scroll_offset >= max_scroll {
                                break;
                            }
                            let top_rows = lvr(win.scroll_offset);
                            let line_height = top_rows as f32 * cell_h;
                            if win.scroll_pixel_offset + cell_h < line_height {
                                win.scroll_pixel_offset += cell_h;
                            } else {
                                let next = buf.next_visible_line(win.scroll_offset);
                                if next <= win.scroll_offset {
                                    break;
                                }
                                win.scroll_offset = next.min(max_scroll);
                                win.scroll_pixel_offset = 0.0;
                            }
                        }
                    }
                }

                // Phase 2: Compute bottom visible row using canonical line_visual_rows.
                let scroll_off = self.window_mgr.focused_window().scroll_offset;
                let skip = self
                    .window_mgr
                    .focused_window()
                    .scroll_skip_rows(self.gui_cell_height);
                let bottom = {
                    let buf = &self.buffers[buf_idx];
                    let max_row = buf_line_count.saturating_sub(1);
                    let mut visual = 0;
                    let mut last_fit = scroll_off;
                    let mut line = scroll_off;
                    let mut first = true;
                    while line <= max_row {
                        let rows = self.line_visual_rows(buf_idx, line);
                        if rows > 0 {
                            let effective = if first {
                                rows.saturating_sub(skip)
                            } else {
                                rows
                            };
                            first = false;
                            if visual + effective > viewport_height {
                                break;
                            }
                            visual += effective;
                            last_fit = line;
                        }
                        line = buf.next_visible_line(line);
                        if line <= last_fit {
                            break;
                        }
                    }
                    last_fit
                };

                // Phase 3: Clamp cursor.
                let buf = &self.buffers[buf_idx];
                let win = self.window_mgr.focused_window_mut();
                if win.cursor_row < scroll_off {
                    win.cursor_row = scroll_off;
                }
                if win.cursor_row > bottom {
                    win.cursor_row = bottom;
                }
                win.clamp_cursor(buf);
                win.scroll_locked = true;
                win.scroll_locked_cursor = win.cursor_row;
            }
        }
    }

    /// Pixel-precise scroll: directly adjusts `scroll_pixel_offset` by a
    /// floating-point pixel amount, crossing line boundaries as needed.
    /// Returns `true` if the scroll position actually changed (for inertia cancellation at bounds).
    pub fn handle_mouse_scroll_pixels(&mut self, delta_px: f32) -> bool {
        if delta_px.abs() < 0.01 {
            return false;
        }
        let buf_idx = self.active_buffer_idx();
        let kind = self.buffers[buf_idx].kind;

        // Non-text buffers: accumulate fractional lines, emit when threshold crossed.
        match kind {
            crate::BufferKind::Shell | crate::BufferKind::Messages => {
                let cell_h = self.gui_cell_height;
                if cell_h <= 0.0 {
                    return false;
                }
                let win = self.window_mgr.focused_window_mut();
                win.shell_scroll_accumulator += delta_px;
                let lines = (win.shell_scroll_accumulator / cell_h).trunc() as i16;
                win.shell_scroll_accumulator -= lines as f32 * cell_h;
                if lines != 0 {
                    self.handle_mouse_scroll(lines);
                }
                // Keep inertia alive while accumulator has residual fractional pixels.
                let acc = self.window_mgr.focused_window().shell_scroll_accumulator;
                return acc.abs() > 0.01 || lines != 0;
            }
            _ => {}
        }

        let buf_line_count = self.buffers[buf_idx].display_line_count();
        let viewport_height = self.focused_viewport_height();
        let cell_h = self.gui_cell_height;
        if cell_h <= 0.0 {
            return false;
        }

        // Pre-compute visual rows cache around scroll region.
        {
            let scroll = self.window_mgr.focused_window().scroll_offset;
            let range_start = scroll.saturating_sub(viewport_height);
            let range_end = (scroll + viewport_height * 2).min(buf_line_count);
            self.populate_visual_rows_cache(buf_idx, range_start, range_end);
        }

        let (cache_rows, cache_line_start) = match &self.buffers[buf_idx].visual_rows_cache {
            Some(c) => (c.rows.clone(), c.line_start),
            None => (Vec::new(), 0),
        };
        let lvr = |line: usize| -> usize {
            if line >= cache_line_start && line < cache_line_start + cache_rows.len() {
                let v = cache_rows[line - cache_line_start] as usize;
                if v > 0 {
                    v
                } else {
                    1
                }
            } else {
                1
            }
        };

        let old_scroll = self.window_mgr.focused_window().scroll_offset;
        let old_pixel = self.window_mgr.focused_window().scroll_pixel_offset;

        if delta_px > 0.0 {
            // Scroll up (reveal content above).
            let mut remaining = delta_px;
            let win = self.window_mgr.focused_window_mut();
            while remaining > 0.0 {
                if win.scroll_pixel_offset >= remaining {
                    win.scroll_pixel_offset -= remaining;
                    remaining = 0.0;
                } else {
                    remaining -= win.scroll_pixel_offset;
                    win.scroll_pixel_offset = 0.0;
                    // Move to previous line.
                    let prev = self.buffers[buf_idx].prev_visible_line(win.scroll_offset);
                    if prev >= win.scroll_offset {
                        // At top.
                        win.scroll_pixel_offset = 0.0;
                        break;
                    }
                    win.scroll_offset = prev;
                    let prev_rows = lvr(prev);
                    let line_height = prev_rows as f32 * cell_h;
                    win.scroll_pixel_offset = line_height - 0.01; // position at bottom of prev line
                }
            }
            // Ensure pixel offset is non-negative.
            let win = self.window_mgr.focused_window_mut();
            if win.scroll_pixel_offset < 0.0 {
                win.scroll_pixel_offset = 0.0;
            }
        } else {
            // Scroll down (reveal content below).
            let max_scroll = buf_line_count.saturating_sub(viewport_height);
            let mut remaining = -delta_px; // positive magnitude
            let win = self.window_mgr.focused_window_mut();
            while remaining > 0.0 && win.scroll_offset < max_scroll {
                let top_rows = lvr(win.scroll_offset);
                let line_height = top_rows as f32 * cell_h;
                let space_in_line = line_height - win.scroll_pixel_offset;
                if remaining < space_in_line {
                    win.scroll_pixel_offset += remaining;
                    remaining = 0.0;
                } else {
                    remaining -= space_in_line;
                    let next = self.buffers[buf_idx].next_visible_line(win.scroll_offset);
                    if next <= win.scroll_offset {
                        break;
                    }
                    win.scroll_offset = next.min(max_scroll);
                    win.scroll_pixel_offset = 0.0;
                }
            }
            // Clamp at max.
            let win = self.window_mgr.focused_window_mut();
            if win.scroll_offset >= max_scroll {
                win.scroll_offset = max_scroll;
                // Allow sub-line offset within last visible range but don't exceed.
            }
        }

        // Phase 2: Compute bottom visible line for cursor clamping.
        let scroll_off = self.window_mgr.focused_window().scroll_offset;
        let skip = self.window_mgr.focused_window().scroll_skip_rows(cell_h);
        let bottom = {
            let buf = &self.buffers[buf_idx];
            let max_row = buf_line_count.saturating_sub(1);
            let mut visual = 0;
            let mut last_fit = scroll_off;
            let mut line = scroll_off;
            let mut first = true;
            while line <= max_row {
                let rows = self.line_visual_rows(buf_idx, line);
                if rows > 0 {
                    let effective = if first {
                        rows.saturating_sub(skip)
                    } else {
                        rows
                    };
                    first = false;
                    if visual + effective > viewport_height {
                        break;
                    }
                    visual += effective;
                    last_fit = line;
                }
                line = buf.next_visible_line(line);
                if line <= last_fit {
                    break;
                }
            }
            last_fit
        };

        // Phase 3: Clamp cursor.
        let buf = &self.buffers[buf_idx];
        let win = self.window_mgr.focused_window_mut();
        if win.cursor_row < scroll_off {
            win.cursor_row = scroll_off;
        }
        if win.cursor_row > bottom {
            win.cursor_row = bottom;
        }
        win.clamp_cursor(buf);
        win.scroll_locked = true;
        win.scroll_locked_cursor = win.cursor_row;

        // Return whether scroll actually moved.
        let new_scroll = self.window_mgr.focused_window().scroll_offset;
        let new_pixel = self.window_mgr.focused_window().scroll_pixel_offset;
        new_scroll != old_scroll || (new_pixel - old_pixel).abs() > 0.01
    }

    /// Pixel-scroll a specific window without permanently changing focus.
    pub fn handle_mouse_scroll_pixels_in_window(
        &mut self,
        target_id: WindowId,
        delta_px: f32,
    ) -> bool {
        let original = self.window_mgr.focused_id();
        self.window_mgr.set_focused(target_id);
        let moved = self.handle_mouse_scroll_pixels(delta_px);
        self.window_mgr.set_focused(original);
        moved
    }
}
