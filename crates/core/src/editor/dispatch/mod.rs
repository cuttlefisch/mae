mod collab;
mod config;
mod dap;
mod edit;
mod file;
mod file_tree;
mod fold_org;
mod git;
mod help;
mod kb;
mod kb_sharing;
mod lsp;
mod nav;
mod notify;
mod project;
mod terminal;
mod ui;
mod visual;
mod window;

use crate::buffer::Buffer;
use crate::window::Direction;

use super::Editor;

impl Editor {
    /// Dispatch a built-in command by name. Returns true if recognized.
    ///
    /// This is the shared dispatch point for human keybindings and the AI agent.
    /// Scheme-defined commands are handled by the binary (which has the SchemeRuntime).
    pub fn dispatch_builtin(&mut self, name: &str) -> bool {
        let _cmd_start = std::time::Instant::now();
        let _cmd_name = name;

        // Mark a CRDT undo boundary before each dispatch so consecutive
        // normal-mode operations become separate undo items.  Inside an
        // explicit undo group (insert mode, compound ops) we skip this so
        // all edits merge into one item.
        {
            let idx = self.active_buffer_idx();
            let buf = &mut self.buffers[idx];
            if !buf.in_undo_group() {
                buf.sync_undo_boundary();
            }
        }

        let pre_buf_idx = self.active_buffer_idx();
        // Part C (native KB graph view): snapshot focus before dispatch so
        // a keyboard/Scheme-triggered focus change (e.g. `focus-left`,
        // `focus-next-window`) can update the graph view's captured
        // `companion_window` afterward. `dispatch_builtin` is the single
        // point EVERY command — human keybinding or AI/MCP — passes
        // through, making it the right central hook (see
        // `Editor::capture_graph_companion_focus`'s doc comment for why
        // `focus_window_at`, the mouse-click path, ALSO calls the same
        // helper directly rather than relying on this alone).
        let pre_focused = self.window_mgr.focused_id();
        self.fire_hook("command-pre");
        let result = self.dispatch_builtin_inner(name);
        self.fire_hook("command-post");
        if self.active_buffer_idx() != pre_buf_idx {
            self.fire_hook("buffer-switch");
        }
        let post_focused = self.window_mgr.focused_id();
        if post_focused != pre_focused {
            self.capture_graph_companion_focus(post_focused);
        }

        let elapsed_us = _cmd_start.elapsed().as_micros() as u64;
        self.perf_stats.record_command(_cmd_name, elapsed_us);
        result
    }

    // @ai-caution: [dispatch] Sequential category dispatch — order matters for
    // performance (nav/edit first = hot path). Adding new categories: append at
    // the end, don't insert before nav/edit. Each handler explicitly calls
    // mark_full_redraw() — nav does NOT (cursor-only). Changing this regresses
    // render perf 10x on large files.
    fn dispatch_builtin_inner(&mut self, name: &str) -> bool {
        // Auto-dismiss hover popup on any command that isn't hover-related.
        if self.lsp.hover_popup.is_some()
            && !matches!(name, "lsp-hover" | "hover-scroll-down" | "hover-scroll-up")
        {
            self.lsp.hover_popup = None;
        }
        // Auto-dismiss signature help on non-related commands.
        if self.lsp.signature_help.is_some() && !matches!(name, "lsp-signature-help") {
            self.lsp.signature_help = None;
        }
        // Auto-dismiss peek definition on non-peek commands.
        if self.lsp.peek_state.is_some() && !matches!(name, "lsp-peek-definition") {
            self.lsp.peek_state = None;
        }
        // Auto-dismiss code action menu on non-code-action commands.
        if self.lsp.code_action_menu.is_some()
            && !matches!(
                name,
                "lsp-code-action"
                    | "lsp-code-action-next"
                    | "lsp-code-action-prev"
                    | "lsp-code-action-select"
                    | "lsp-code-action-dismiss"
            )
        {
            self.lsp.code_action_menu = None;
        }

        // Consume the count prefix at the top of every dispatch.
        // `count` is Some(n) if user typed a digit prefix, None if not.
        // `n` is the effective repeat count (default 1).
        let count = self.vi.count_prefix.take();
        let n = count.unwrap_or(1);

        // Track linewise vs characterwise for operator-pending mode
        self.vi.last_motion_linewise = Self::is_linewise_motion(name);

        // Try each category in turn. Order doesn't matter for correctness
        // (arm names are unique across categories), but we put high-frequency
        // categories first for marginal efficiency.
        if let Some(v) = self.dispatch_nav(name, count, n) {
            return v;
        }
        if let Some(v) = self.dispatch_edit(name, count, n) {
            return v;
        }
        if let Some(v) = self.dispatch_visual(name) {
            return v;
        }
        if let Some(v) = self.dispatch_file(name) {
            return v;
        }
        if let Some(v) = self.dispatch_window(name) {
            return v;
        }
        if let Some(v) = self.dispatch_help(name) {
            return v;
        }
        if let Some(v) = self.dispatch_terminal(name) {
            return v;
        }
        if let Some(v) = self.dispatch_project(name) {
            return v;
        }
        if let Some(v) = self.dispatch_kb(name) {
            return v;
        }
        if let Some(v) = self.dispatch_config(name) {
            return v;
        }
        if let Some(v) = self.dispatch_ui(name) {
            return v;
        }
        if let Some(v) = self.dispatch_fold_org(name) {
            return v;
        }
        // Multi-cursor commands
        match name {
            "mc-add-cursor-below" => {
                super::multicursor::mc_add_cursor_below(self);
                self.mark_full_redraw();
                return true;
            }
            "mc-add-cursor-above" => {
                super::multicursor::mc_add_cursor_above(self);
                self.mark_full_redraw();
                return true;
            }
            "mc-add-at-next-word" => {
                super::multicursor::mc_add_at_next_word(self);
                self.mark_full_redraw();
                return true;
            }
            "mc-add-all-word" => {
                super::multicursor::mc_add_all_word(self);
                self.mark_full_redraw();
                return true;
            }
            "mc-skip-next" => {
                super::multicursor::mc_skip_next(self);
                self.mark_full_redraw();
                return true;
            }
            "mc-clear" => {
                super::multicursor::mc_clear(self);
                self.mark_full_redraw();
                return true;
            }
            "mc-align" => {
                super::multicursor::mc_align(self);
                self.mark_full_redraw();
                return true;
            }
            _ => {}
        }
        if let Some(v) = self.dispatch_git(name) {
            return v;
        }
        if let Some(v) = self.dispatch_lsp(name) {
            return v;
        }
        if let Some(v) = self.dispatch_dap(name) {
            return v;
        }
        if let Some(v) = self.dispatch_file_tree(name) {
            self.mark_full_redraw();
            return v;
        }
        if let Some(v) = self.dispatch_collab(name) {
            return v;
        }
        if let Some(v) = self.dispatch_notifications(name) {
            return v;
        }
        // `:kb-sharing` and the documented `:kb-sharing-status` both open the
        // *KB Sharing* buffer — the human status surface (the same snapshot the
        // `(kb-sharing-status)` Scheme prim and `kb_sharing_status` MCP tool read).
        if name == "kb-sharing" || name == "kb-sharing-status" {
            self.open_kb_sharing();
            return true;
        }
        if let Some(v) = self.dispatch_kb_sharing(name) {
            return v;
        }

        // Snippet commands
        match name {
            "snippet-expand-or-next" => {
                if let Some(ref mut session) = self.snippet_session {
                    if let Some((_offset, _len)) = session.next_field() {
                        // Field navigation — cursor positioning handled by caller
                        self.mark_full_redraw();
                    } else {
                        // Session complete
                        self.snippet_session = None;
                    }
                }
                // If no active session, fall through (let Tab do its normal thing)
                return self.snippet_session.is_some();
            }
            "snippet-prev-field" => {
                if let Some(ref mut session) = self.snippet_session {
                    session.prev_field();
                    self.mark_full_redraw();
                    return true;
                }
                return false;
            }
            "snippet-commit" => {
                self.snippet_session = None;
                return true;
            }
            _ => {}
        }

        // Format commands
        match name {
            "format-buffer" => {
                let idx = self.window_mgr.focused_window().buffer_idx;
                let lang = self.buffers[idx]
                    .file_path()
                    .and_then(crate::lsp_intent::language_id_from_path)
                    .unwrap_or_default()
                    .to_string();
                if let Some(fmt) = self.format_config.get(&lang).cloned() {
                    let content = self.buffers[idx].rope().to_string();
                    let path = self.buffers[idx]
                        .file_path()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_default();
                    match mae_format::format_with_external(&fmt, &content, &path) {
                        Ok(result) if result.changed => {
                            self.buffers[idx]
                                .replace_rope(ropey::Rope::from_str(&result.formatted));
                            self.set_status("Formatted with external formatter");
                        }
                        Ok(_) => self.set_status("Already formatted"),
                        Err(e) => self.set_status(format!("Format error: {}", e)),
                    }
                    self.mark_full_redraw();
                    return true;
                }
                return false; // Fall through to LSP format
            }
            "format-before-save" => {
                let idx = self.window_mgr.focused_window().buffer_idx;
                let lang = self.buffers[idx]
                    .file_path()
                    .and_then(crate::lsp_intent::language_id_from_path)
                    .unwrap_or_default()
                    .to_string();
                if let Some(fmt) = self.format_config.get(&lang).cloned() {
                    let content = self.buffers[idx].rope().to_string();
                    let path = self.buffers[idx]
                        .file_path()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_default();
                    if let Ok(result) = mae_format::format_with_external(&fmt, &content, &path) {
                        if result.changed {
                            self.buffers[idx]
                                .replace_rope(ropey::Rope::from_str(&result.formatted));
                        }
                    }
                }
                return true;
            }
            _ => {}
        }

        // Build commands
        match name {
            "run-build" | "run-test" => {
                let idx = self.window_mgr.focused_window().buffer_idx;
                let path = self.buffers[idx].file_path().map(|p| p.to_path_buf());
                let start = path.as_deref().unwrap_or(std::path::Path::new("."));
                if let Some(bs) = mae_make::detect_build_system(start) {
                    let cmd = if name == "run-test" {
                        bs.test_cmd.unwrap_or(bs.build_cmd)
                    } else {
                        bs.build_cmd
                    };
                    self.set_status(format!("[{}] Run: {}", bs.kind, cmd));
                    // Shell command execution is handled by the binary event loop
                    // via pending_scheme_eval or direct shell spawn
                    self.pending_scheme_eval
                        .push(format!("(shell-command \"{}\")", cmd.replace('\"', "\\\"")));
                } else {
                    self.set_status("No build system detected");
                }
                return true;
            }
            "next-error" => {
                if !self.build_errors.is_empty() {
                    if self.build_error_idx < self.build_errors.len().saturating_sub(1) {
                        self.build_error_idx += 1;
                    }
                    let e = &self.build_errors[self.build_error_idx];
                    self.set_status(format!("{}:{}: {}", e.file, e.line, e.message));
                    self.mark_full_redraw();
                } else {
                    self.set_status("No build errors");
                }
                return true;
            }
            "prev-error" => {
                if !self.build_errors.is_empty() {
                    self.build_error_idx = self.build_error_idx.saturating_sub(1);
                    let e = &self.build_errors[self.build_error_idx];
                    self.set_status(format!("{}:{}: {}", e.file, e.line, e.message));
                    self.mark_full_redraw();
                } else {
                    self.set_status("No build errors");
                }
                return true;
            }
            _ => {}
        }

        // Lookup commands
        if name == "lookup-online" {
            let idx = self.window_mgr.focused_window().buffer_idx;
            let win = self.window_mgr.focused_window();
            let char_off = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
            let word = crate::search::word_at_offset(self.buffers[idx].rope(), char_off)
                .unwrap_or_default();
            let lang = self.buffers[idx]
                .file_path()
                .and_then(crate::lsp_intent::language_id_from_path)
                .unwrap_or_default()
                .to_string();
            if let Some(url) = mae_lookup::docs_url(&word, &lang) {
                self.set_status(format!("Docs: {}", url));
            } else {
                self.set_status("No docs URL for this language");
            }
            return true;
        }

        // Spell commands
        match name {
            "spell-check-buffer" | "spell-toggle" => {
                let idx = self.window_mgr.focused_window().buffer_idx;
                let text = self.buffers[idx].rope().to_string();
                if let Some(backend) = mae_spell::check_available() {
                    match mae_spell::check_text(&text, &backend) {
                        Ok(results) => {
                            let count = results.len();
                            self.spell_results.insert(idx, results);
                            self.set_status(format!("{} misspelling(s) found", count));
                        }
                        Err(e) => self.set_status(format!("Spell check error: {}", e)),
                    }
                } else {
                    self.set_status("No spell checker found (install aspell or hunspell)");
                }
                self.mark_full_redraw();
                return true;
            }
            "spell-next" => {
                let idx = self.window_mgr.focused_window().buffer_idx;
                let win = self.window_mgr.focused_window();
                let cursor_byte = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                if let Some(results) = self.spell_results.get(&idx) {
                    if let Some(m) = results.iter().find(|m| m.offset > cursor_byte) {
                        let hint = m
                            .suggestions
                            .first()
                            .map(|s| s.as_str())
                            .unwrap_or("no suggestions");
                        self.set_status(format!("Misspelled: {} ({})", m.word, hint));
                    }
                }
                return true;
            }
            "spell-prev" => {
                return true; // placeholder
            }
            "spell-suggest" => {
                let idx = self.window_mgr.focused_window().buffer_idx;
                let win = self.window_mgr.focused_window();
                let cursor_byte = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                if let Some(results) = self.spell_results.get(&idx) {
                    if let Some(m) = results
                        .iter()
                        .find(|m| cursor_byte >= m.offset && cursor_byte < m.offset + m.length)
                    {
                        if m.suggestions.is_empty() {
                            self.set_status(format!("No suggestions for '{}'", m.word));
                        } else {
                            let top: Vec<&str> =
                                m.suggestions.iter().take(5).map(|s| s.as_str()).collect();
                            self.set_status(format!(
                                "Suggestions for '{}': {}",
                                m.word,
                                top.join(", ")
                            ));
                        }
                    }
                }
                return true;
            }
            _ => {}
        }

        false
    }

    /// Dispatch a built-in command in the AI target window context.
    ///
    /// Temporarily switches focus to the AI target window (if set), dispatches
    /// the command, then restores focus. This is the Emacs `save-excursion` /
    /// `with-current-buffer` pattern. Synchronous — no render between
    /// save/restore, so no visual flicker.
    ///
    /// Returns true if the command was recognized.
    pub fn dispatch_builtin_in_target(&mut self, name: &str) -> bool {
        let target_win = self.ai.target_window_id;
        let saved_focus = self.window_mgr.focused_id();

        // Switch focus to the AI target window if set
        if let Some(win_id) = target_win {
            self.window_mgr.set_focused(win_id);
        }

        let result = self.dispatch_builtin(name);

        // Restore original focus
        if target_win.is_some() {
            self.window_mgr.set_focused(saved_focus);
        }

        result
    }

    /// Dispatch a command and replay at secondary cursors if applicable.
    pub fn dispatch_with_multicursor(&mut self, name: &str) -> bool {
        let result = self.dispatch_builtin(name);
        if result {
            super::multicursor::replay_command_at_secondaries(self, name);
        }
        result
    }

    /// Kill buffer at `idx`, handling LSP notification, window fixup, and fallback.
    pub fn kill_buffer_at(&mut self, idx: usize) {
        // If this buffer is part of a conversation pair, close both halves.
        if let Some(ref pair) = self.ai.conversation_pair {
            let sibling_idx = if idx == pair.output_buffer_idx {
                Some(pair.input_buffer_idx)
            } else if idx == pair.input_buffer_idx {
                Some(pair.output_buffer_idx)
            } else {
                None
            };
            if let Some(sib) = sibling_idx {
                let pair = self.ai.conversation_pair.take().unwrap();
                // Close the sibling's window.
                let sib_win = if sib == pair.input_buffer_idx {
                    pair.input_window_id
                } else {
                    pair.output_window_id
                };
                self.window_mgr.close(sib_win);
                // Remove both buffers (higher index first to avoid shifting).
                let (first, second) = if idx > sib { (idx, sib) } else { (sib, idx) };
                self.remove_buffer_raw(first);
                self.remove_buffer_raw(second);
                self.set_mode(crate::Mode::Normal);
                let new_idx = self.active_buffer_idx();
                let name = self.buffers[new_idx].name.clone();
                self.set_status(format!("Conversation closed — now: {}", name));
                return;
            }
        }

        self.fire_hook("buffer-close");
        // Dismiss hover popup if it belongs to the buffer being killed.
        if self
            .lsp
            .hover_popup
            .as_ref()
            .is_some_and(|p| p.buffer_idx == idx)
        {
            self.lsp.hover_popup = None;
        }
        if self.buffers.len() <= 1 {
            self.lsp_notify_did_close_for_buffer(0);
            self.buffers[0] = Buffer::new();
            self.syntax.remove(0);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = 0;
            win.cursor_col = 0;
            self.set_status("Buffer killed — [scratch]");
        } else {
            // If killing a FileTree buffer, close its dedicated window.
            if self.buffers[idx].kind == crate::buffer::BufferKind::FileTree {
                if let Some(win_id) = self.file_tree_window_id.take() {
                    self.window_mgr.close(win_id);
                }
            }
            self.lsp_notify_did_close_for_buffer(idx);
            self.buffers.remove(idx);
            self.notify_buffer_removed(idx);
            // Collect dedicated window IDs before mutable iteration.
            let dedicated: Vec<crate::window::WindowId> = self
                .window_mgr
                .iter_windows()
                .filter(|w| w.buffer_idx == idx)
                .map(|w| w.id)
                .collect();
            for win in self.window_mgr.iter_windows_mut() {
                if win.buffer_idx == idx {
                    win.buffer_idx = find_replacement_buffer(&self.buffers, idx);
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                } else if win.buffer_idx > idx {
                    win.buffer_idx -= 1;
                }
            }
            // Close dedicated windows whose buffer was killed rather than
            // reassigning them to an unrelated buffer.
            for wid in dedicated {
                if self.is_dedicated_window(wid) {
                    self.window_mgr.close(wid);
                }
            }
            let new_idx = self.active_buffer_idx();
            let name = self.buffers[new_idx].name.clone();
            self.set_status(format!("Buffer killed — now: {}", name));
        }
    }

    /// Remove a buffer at index and adjust all window references. Shared by
    /// conversation pair teardown so we don't duplicate the index-shifting logic.
    fn remove_buffer_raw(&mut self, idx: usize) {
        if idx >= self.buffers.len() {
            return;
        }
        // Warn if sync updates are being dropped — the broadcaster drains
        // every ~16-100ms, so this should be rare in practice.
        if !self.buffers[idx].pending_sync_updates.is_empty() {
            tracing::warn!(
                buffer = %self.buffers[idx].name,
                pending = self.buffers[idx].pending_sync_updates.len(),
                "dropping pending sync updates on buffer close"
            );
        }
        self.lsp_notify_did_close_for_buffer(idx);
        self.buffers.remove(idx);
        self.notify_buffer_removed(idx);
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx == idx {
                win.buffer_idx = find_replacement_buffer(&self.buffers, idx);
                win.cursor_row = 0;
                win.cursor_col = 0;
            } else if win.buffer_idx > idx {
                win.buffer_idx -= 1;
            }
        }
    }

    /// Ensure at least one non-sidebar text buffer exists. If none remains after
    /// a bulk kill, create a fresh `[scratch]` buffer.
    pub fn ensure_scratch_exists(&mut self) {
        let has_user_buf = self
            .buffers
            .iter()
            .any(|b| !b.kind.is_sidebar() && b.kind != crate::BufferKind::Dashboard);
        if !has_user_buf {
            self.buffers.push(Buffer::new());
        }
    }

    /// Focus a window in the given direction with proper hook firing and mode sync.
    fn focus_direction(&mut self, dir: Direction) {
        self.fire_hook("focus-out");
        self.save_mode_to_buffer();
        let area = self.default_area();
        self.window_mgr.focus_direction(dir, area);
        self.sync_mode_to_buffer();
        // Refresh KB buffer on focus (picks up node edits from other windows).
        let idx = self.active_buffer_idx();
        if self.buffers[idx].kind == crate::buffer::BufferKind::Kb {
            self.kb_populate_buffer(idx);
        }
        // When focusing a conversation output buffer, jump cursor to the last line
        // so the user sees the most recent content (not stranded at row 0).
        if self.buffers[idx].kind == crate::buffer::BufferKind::Conversation {
            let last_line = self.buffers[idx].display_line_count().saturating_sub(1);
            let vh = self.focused_viewport_height();
            let win = self.window_mgr.focused_window_mut();
            if win.cursor_row == 0 && last_line > 0 {
                win.cursor_row = last_line;
                win.cursor_col = 0;
                // scroll_offset is now a rope line index (same as all other buffers).
                // Set it high; the renderer clamps to total-viewport_height.
                win.scroll_offset = last_line.saturating_sub(vh);
            }
        }
        self.fire_hook("focus-in");
    }

    /// Transform the current line's text using a closure (e.g. uppercase, lowercase).
    fn transform_current_line(&mut self, f: impl FnOnce(&str) -> String) {
        let idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let line_start = self.buffers[idx].rope().line_to_char(row);
        let line_len = self.buffers[idx].line_len(row);
        if line_len > 0 {
            let text = self.buffers[idx].text_range(line_start, line_start + line_len);
            let transformed = f(&text);
            self.buffers[idx].delete_range(line_start, line_start + line_len);
            self.buffers[idx].insert_text_at(line_start, &transformed);
        }
    }
}

/// Find a non-sidebar buffer near `near` to use as a replacement when
/// a buffer is killed. Prefers buffers at lower indices first, then higher.
fn find_replacement_buffer(buffers: &[Buffer], near: usize) -> usize {
    let start = near.min(buffers.len().saturating_sub(1));
    for offset in 0..buffers.len() {
        if start >= offset {
            let j = start - offset;
            if !buffers[j].kind.is_sidebar() {
                return j;
            }
        }
        let j = start + offset + 1;
        if j < buffers.len() && !buffers[j].kind.is_sidebar() {
            return j;
        }
    }
    // Fallback: everything is a sidebar, just pick the nearest.
    start
}
