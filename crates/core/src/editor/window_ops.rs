//! Buffer/window management: focus routing, buffer display policy, and
//! window lifecycle bookkeeping extracted from `Editor`'s main impl block.
//! Split out of `mod.rs` (ADR none needed) — pure code motion, same pattern
//! as `kb_ops.rs`.

use crate::buffer::Buffer;
use crate::command_palette::CommandPalette;
use crate::window::Rect;
use crate::Mode;

use super::{rekey_after_remove, Editor};

impl Editor {
    /// Insert a dashboard buffer at position 0 and focus it.
    /// Call this at application startup (before opening files) to get a
    /// Doom-style splash screen. The existing scratch buffer shifts to index 1.
    pub fn install_dashboard(&mut self) {
        self.buffers.insert(0, Buffer::new_dashboard());
        // Fix up window buffer indices — they all shift right by 1.
        for win in self.window_mgr.iter_windows_mut() {
            win.buffer_idx += 1;
        }
        if let Some(alt) = self.vi.alternate_buffer_idx.as_mut() {
            *alt += 1;
        }
        // Focus the dashboard.
        self.window_mgr.focused_window_mut().buffer_idx = 0;
    }

    /// Convenience: index of the active (focused window's) buffer.
    pub fn active_buffer_idx(&self) -> usize {
        self.window_mgr.focused_window().buffer_idx
    }

    /// AI-aware buffer index: returns `ai_target_buffer_idx` if set,
    /// otherwise falls back to `active_buffer_idx()`.
    pub fn ai_active_buffer_idx(&self) -> usize {
        self.ai
            .target_buffer_idx
            .unwrap_or_else(|| self.active_buffer_idx())
    }

    /// AI-aware cursor row: reads cursor from the AI target window if set,
    /// otherwise from the focused window.
    pub fn ai_cursor_row(&self) -> usize {
        if let Some(win_id) = self.ai.target_window_id {
            if let Some(win) = self.window_mgr.iter_windows().find(|w| w.id == win_id) {
                return win.cursor_row;
            }
        }
        self.window_mgr.focused_window().cursor_row
    }

    pub fn active_buffer(&self) -> &Buffer {
        let idx = self.active_buffer_idx();
        assert!(
            idx < self.buffers.len(),
            "buffer_idx {} out of range ({})",
            idx,
            self.buffers.len()
        );
        &self.buffers[idx]
    }

    pub fn active_buffer_mut(&mut self) -> &mut Buffer {
        let idx = self.active_buffer_idx();
        assert!(
            idx < self.buffers.len(),
            "buffer_idx {} out of range ({})",
            idx,
            self.buffers.len()
        );
        &mut self.buffers[idx]
    }

    /// Find a buffer index by name. Returns None if not found.
    pub fn find_buffer_by_name(&self, name: &str) -> Option<usize> {
        self.buffers.iter().position(|b| b.name == name)
    }

    /// Find a buffer by its collaborative document ID.
    /// Falls back to `find_buffer_by_name` if no buffer has a matching `collab_doc_id`.
    pub fn find_buffer_by_collab_doc_id(&self, doc_id: &str) -> Option<usize> {
        self.buffers
            .iter()
            .position(|b| b.collab_doc_id.as_deref() == Some(doc_id))
            .or_else(|| self.find_buffer_by_name(doc_id))
    }

    /// Find a buffer by name, or create it with the provided closure.
    /// Returns the buffer index.
    pub fn find_or_create_buffer(&mut self, name: &str, create: impl FnOnce() -> Buffer) -> usize {
        if let Some(idx) = self.find_buffer_by_name(name) {
            idx
        } else {
            self.buffers.push(create());
            self.buffers.len() - 1
        }
    }

    /// Open a command palette popup and switch to CommandPalette mode.
    pub fn open_palette(&mut self, palette: CommandPalette) {
        self.command_palette = Some(palette);
        self.set_mode(Mode::CommandPalette);
    }

    /// Set the editor mode and fire the `mode-change` hook.
    /// Returns `true` if the mode was changed, `false` if blocked or already in that mode.
    pub fn set_mode(&mut self, mode: Mode) -> bool {
        // Block non-Normal modes for buffers that only allow Normal mode
        // (e.g. Dashboard, Modules).
        if mode != Mode::Normal
            && mode != Mode::Command
            && mode != Mode::Search
            && mode != Mode::CommandPalette
            && mode != Mode::FilePicker
            && mode != Mode::FileBrowser
        {
            use crate::BufferMode;
            if self.active_buffer().kind.normal_mode_only() {
                tracing::debug!(
                    requested = ?mode,
                    buffer = %self.active_buffer().name,
                    kind = ?self.active_buffer().kind,
                    "set_mode blocked: buffer is normal_mode_only"
                );
                return false;
            }
        }
        if self.mode != mode {
            self.mode = mode;
            self.fire_hook("mode-change");
            true
        } else {
            false
        }
    }

    /// Switch the focused window to the buffer at the given index.
    /// Returns false if index is out of bounds.
    pub fn switch_to_buffer(&mut self, idx: usize) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }
        let prev_idx = self.active_buffer_idx();
        if prev_idx != idx {
            self.vi.alternate_buffer_idx = Some(prev_idx);
        }
        self.save_mode_to_buffer();
        // Check for external file changes before showing the buffer.
        self.check_and_reload_buffer(idx);
        let win = self.window_mgr.focused_window_mut();
        win.save_view_state();
        win.restore_view_state(idx);
        // Clamp cursor to buffer bounds (file may have changed on disk).
        let line_count = self.buffers[idx].line_count();
        let win = self.window_mgr.focused_window_mut();
        if win.cursor_row >= line_count {
            win.cursor_row = line_count.saturating_sub(1);
        }
        let line_len = self.buffers[idx].line_len(win.cursor_row);
        if win.cursor_col > line_len {
            win.cursor_col = line_len;
        }
        // Recompute search matches for the new buffer so highlights and
        // `n`/`N` navigation are correct.
        self.recompute_search_matches();
        self.sync_mode_to_buffer();
        true
    }

    /// Returns true if the buffer at `idx` is a Conversation buffer, or an
    /// agent-shell buffer (e.g. an external MCP agent's `*AI:claude*` PTY) —
    /// both are "protected" surfaces an AI/MCP dispatch must never silently
    /// steal (see `ensure_ai_dispatch_target`/`with_ai_dispatch_scope`).
    pub fn is_conversation_buffer(&self, idx: usize) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }
        if self.buffers[idx].kind == crate::BufferKind::Conversation {
            return true;
        }
        if self.buffers[idx].agent_shell {
            return true;
        }
        // The *ai-input* buffer is also part of the conversation pair.
        if let Some(ref pair) = self.ai.conversation_pair {
            if idx == pair.input_buffer_idx {
                return true;
            }
        }
        false
    }

    /// Returns true if windows showing this buffer kind can be replaced by new content.
    pub fn is_kind_replaceable(&self, kind: crate::BufferKind) -> bool {
        self.replaceable_kinds.contains(&kind)
    }

    /// Find a window showing a replaceable buffer kind. Returns the window ID.
    /// Prefers the focused window if it's replaceable. Excludes conversation pair windows.
    fn find_replaceable_window(&self) -> Option<crate::window::WindowId> {
        let conv_ids = self
            .ai
            .conversation_pair
            .as_ref()
            .map(|p| [p.output_window_id, p.input_window_id]);
        let focused_id = self.window_mgr.focused_id();
        // Prefer the focused window (natural UX: what you see gets replaced).
        if let Some(fw) = self.window_mgr.window(focused_id) {
            if fw.buffer_idx < self.buffers.len()
                && self.is_kind_replaceable(self.buffers[fw.buffer_idx].kind)
                && !conv_ids.is_some_and(|ids| ids.contains(&focused_id))
            {
                return Some(focused_id);
            }
        }
        // Then check all other windows.
        self.window_mgr
            .iter_windows()
            .find(|w| {
                w.buffer_idx < self.buffers.len()
                    && self.is_kind_replaceable(self.buffers[w.buffer_idx].kind)
                    && !conv_ids.is_some_and(|ids| ids.contains(&w.id))
            })
            .map(|w| w.id)
    }

    /// Returns true if `win_id` belongs to a dedicated purpose (file tree,
    /// conversation pair) and should never be repurposed for general buffer routing.
    pub fn is_dedicated_window(&self, win_id: crate::window::WindowId) -> bool {
        if self.file_tree_window_id == Some(win_id) {
            return true;
        }
        if let Some(ref pair) = self.ai.conversation_pair {
            if win_id == pair.output_window_id || win_id == pair.input_window_id {
                return true;
            }
        }
        // Fallback: check buffer kind for other sidebar types (debug, messages, etc.)
        // but exclude replaceable kinds — those windows CAN be repurposed.
        if let Some(w) = self.window_mgr.window(win_id) {
            if w.buffer_idx < self.buffers.len()
                && self.buffers[w.buffer_idx].kind.is_sidebar()
                && !self.is_kind_replaceable(self.buffers[w.buffer_idx].kind)
            {
                return true;
            }
        }
        false
    }

    /// Switch to buffer `idx` but avoid stealing focus from a conversation window.
    ///
    /// If the focused window shows a conversation buffer, the new buffer is
    /// routed to another window (or a new split is created). This keeps `*AI*`
    /// Adjust `ai_target_buffer_idx` after a buffer at `removed_idx` was removed.
    /// Must be called after every `buffers.remove()` to prevent stale indices.
    pub fn adjust_ai_target_after_remove(&mut self, removed_idx: usize) {
        if let Some(ref mut target) = self.ai.target_buffer_idx {
            if *target == removed_idx {
                // The target buffer was removed — clear it
                self.ai.target_buffer_idx = None;
            } else if *target > removed_idx {
                *target -= 1;
            }
        }
    }

    /// Central bookkeeping after `buffers.remove(removed_idx)`.
    ///
    /// Rekeys all Editor-owned HashMaps keyed by buffer index, adjusts
    /// pending queues, alternate_buffer_idx, AI target, syntax map, and
    /// per-window saved_view_states. Also pushes `removed_idx` to
    /// `pending_buffer_removals` so the binary can rekey its own maps
    /// (shell_terminals, shell_last_dims, shell_generations).
    ///
    /// Callers are still responsible for adjusting `window.buffer_idx`
    /// (different sites have different retarget logic).
    pub fn notify_buffer_removed(&mut self, removed_idx: usize) {
        // 1. Syntax + AI target
        self.syntax.shift_after_remove(removed_idx);
        self.adjust_ai_target_after_remove(removed_idx);

        // 2. Editor-owned shell maps
        rekey_after_remove(&mut self.shell.viewports, removed_idx);
        rekey_after_remove(&mut self.shell.viewport_cwds, removed_idx);
        rekey_after_remove(&mut self.shell.cwds, removed_idx);

        // 3. Pending shell queues (Vec<usize> and Vec<(usize, _)>)
        self.shell.spawns.retain_mut(|idx| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.shell.agent_spawns.retain_mut(|(idx, _)| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.shell.resets.retain_mut(|idx| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.shell.closes.retain_mut(|idx| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.shell.inputs.retain_mut(|(idx, _)| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });

        // 4. Alternate buffer index
        if let Some(ref mut alt) = self.vi.alternate_buffer_idx {
            if *alt == removed_idx {
                self.vi.alternate_buffer_idx = None;
            } else if *alt > removed_idx {
                *alt -= 1;
            }
        }

        // 5. Per-window saved_view_states
        for win in self.window_mgr.iter_windows_mut() {
            rekey_after_remove(&mut win.saved_view_states, removed_idx);
        }

        // 6. Conversation pair buffer indices
        if let Some(ref mut pair) = self.ai.conversation_pair {
            if pair.output_buffer_idx == removed_idx || pair.input_buffer_idx == removed_idx {
                self.ai.conversation_pair = None; // invalidate
            } else {
                if pair.output_buffer_idx > removed_idx {
                    pair.output_buffer_idx -= 1;
                }
                if pair.input_buffer_idx > removed_idx {
                    pair.input_buffer_idx -= 1;
                }
            }
        }

        // 7. Signal the binary to rekey its own maps
        self.pending_buffer_removals.push(removed_idx);
    }

    /// Display buffer `idx` the way an AI/MCP agent's actions should: reusing
    /// the single window the agent has been driving (`self.ai.work_window`,
    /// a `DrivenWindow`) across a sequence of calls, regardless of the
    /// buffer's `BufferKind` — a "browser tab navigating between page types"
    /// model rather than one split per action. Also avoids stealing focus
    /// from a conversation window.
    ///
    /// Renamed from `switch_to_buffer_non_conversation` (which had this
    /// exact logic, but was only ever called for `Text`/`Diff` buffers)
    /// once confirmed
    /// `BufferKind`-agnostic: every branch below operates on `idx`/
    /// `buf_idx`, never branches on the displayed buffer's kind (only on
    /// buffer *properties* like `agent_shell` and on *other* windows'
    /// dedicated/replaceable status) — so the exact same step-based fallback
    /// (reuse if visible -> commandeer non-focused/non-dedicated ->
    /// commandeer replaceable -> split) is correct for KB/Shell/Messages
    /// buffers too, not just Text/Diff. This is the direct fix for the
    /// reported cascading-splits bug: agent-triggered display calls that
    /// used to bypass this window uniformly now route through it.
    ///
    /// `self.ai.work_window`'s validity check (`get_valid`) is used directly
    /// here rather than `DrivenWindow::resolve_persistent`, because the
    /// commandeer/split fallback below needs full `&mut Editor` access
    /// (buffer-kind lookups via `is_dedicated_window`/`is_kind_replaceable`,
    /// conversation-pair awareness, window splitting) that doesn't fit
    /// `resolve_persistent`'s intentionally narrow `&WindowManager`-only
    /// `create_or_pick` closure. See `driven_window.rs` module docs.
    pub fn display_buffer_for_agent(&mut self, idx: usize) -> bool {
        // Same mode-sync gap `display_buffer` had (see its doc comment) —
        // this is a THIRD, separate buffer-display primitive (used by the
        // AI/MCP `open_file` tool, among others) that directly mutates a
        // window's `buffer_idx` in several branches below, none of which
        // ever synced `Editor.mode` to the newly-shown buffer. Wrapped
        // (rather than editing each branch individually) since this
        // function has several early-return branches and, by design,
        // deliberately does NOT steal focus in the common case — so mode
        // must only resync when the FOCUSED window's buffer actually
        // changed as a side effect, not unconditionally. Comparing the
        // focused window's buffer_idx before/after handles every branch
        // uniformly, including the ones that mutate a non-focused window
        // (correctly a no-op) and the one that already calls
        // `switch_to_buffer` internally (already handles its own sync,
        // this wrapper's post-check is then just a harmless no-op re-sync
        // of the same, already-correct value).
        let focused_id = self.window_mgr.focused_id();
        let old_focused_buf = self.window_mgr.window(focused_id).map(|w| w.buffer_idx);
        self.save_mode_to_buffer();
        let result = self.display_buffer_for_agent_impl(idx);
        let new_focused_buf = self.window_mgr.window(focused_id).map(|w| w.buffer_idx);
        if old_focused_buf != new_focused_buf {
            self.sync_mode_to_buffer();
        }
        result
    }

    fn display_buffer_for_agent_impl(&mut self, idx: usize) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }

        self.ai.target_buffer_idx = Some(idx);

        // 0. Reuse the dedicated AI work window if it exists and is still valid.
        if let Some(work_id) = self.ai.work_window.get_valid(&self.window_mgr) {
            if let Some(win) = self.window_mgr.window_mut(work_id) {
                win.buffer_idx = idx;
                win.cursor_row = 0;
                win.cursor_col = 0;
            }
            self.ai.target_window_id = Some(work_id);
            self.mark_full_redraw();
            return true;
        } else {
            self.ai.work_window.set(None); // stale reference (no-op if already unset)
        }

        self.find_or_create_companion_window(idx, None).is_some()
    }

    /// Proactively guarantees `ai.target_window_id` points at a real,
    /// isolated window before MCP/AI-driven dispatch runs — a standing
    /// invariant (CLAUDE.md principle #3: the AI is a peer, and must never
    /// silently hide the human/conversation view), established up front
    /// rather than inferred after the fact from a before/after diff. No-op
    /// once a valid target already exists; also a no-op if the
    /// currently-focused window isn't showing anything worth protecting
    /// (e.g. the agent is already working in a plain file buffer) — this
    /// never forces a split when there's nothing to isolate. See
    /// `with_ai_dispatch_scope`, the enforced entry point that calls this.
    fn ensure_ai_dispatch_target(&mut self) -> Option<crate::window::WindowId> {
        if let Some(id) = self.ai.target_window_id {
            if self.window_mgr.window(id).is_some() {
                return Some(id);
            }
        }
        let focused_id = self.window_mgr.focused_id();
        let focused_buf = self.window_mgr.window(focused_id)?.buffer_idx;
        if !self.is_conversation_buffer(focused_buf) {
            return None;
        }
        // Split (or reuse) a companion window; if a split happens it clones
        // the currently-focused buffer into the new pane rather than
        // swapping in a foreign one — the same thing a human would do by
        // hand (`command_split_vertical` first), just automatic.
        let companion = self.find_or_create_companion_window(focused_buf, Some(focused_id))?;
        self.ai.work_window.set(Some(companion));
        self.ai.target_window_id = Some(companion);
        Some(companion)
    }

    /// The single enforced scope for MCP/AI-driven editor mutations
    /// (issue #372). Ensures an isolated target window exists (see
    /// `ensure_ai_dispatch_target`), focuses it for the duration of `f`,
    /// then restores whatever was focused beforehand — a Python
    /// `with`/Rust-RAII-guard-shaped primitive: setup, run the body,
    /// guaranteed teardown. Because focus is redirected *before* `f` runs,
    /// everything inside it — `dispatch_builtin`, `dispatch_command_by_name`,
    /// any future dispatch mechanism — automatically operates against the
    /// companion window through the existing `display_buffer`/
    /// `ReplaceFocused` machinery, with no per-command or per-call-site
    /// awareness required. Every MCP dispatch entry point MUST route
    /// through this rather than reimplementing target redirection locally.
    pub fn with_ai_dispatch_scope<R>(&mut self, f: impl FnOnce(&mut Editor) -> R) -> R {
        self.ensure_ai_dispatch_target();
        let target = self.ai.target_window_id;
        let saved_focus = self.window_mgr.focused_id();
        if let Some(win_id) = target {
            self.window_mgr.set_focused(win_id);
        }
        let result = f(self);
        if target.is_some() {
            self.window_mgr.set_focused(saved_focus);
        }
        result
    }

    /// Find (or create by splitting) a window — distinct from `exclude` when
    /// given — that ends up showing buffer `idx`. Shared by
    /// `display_buffer_for_agent_impl` (which passes `exclude: None`: it just
    /// wants a home for `idx`, wherever that ends up — including the
    /// currently-focused window, if `idx` already happens to be shown there)
    /// and `ensure_ai_dispatch_target` (which passes `exclude: Some(focused_id)`:
    /// it specifically wants a window DIFFERENT from the one currently
    /// focused, since the whole point is giving the conversation/agent-shell
    /// buffer its own untouched window rather than reporting "it's already
    /// showing what you asked for" when `idx` IS that same protected buffer).
    /// On every terminal branch this sets `self.ai.work_window`/`target_window_id`
    /// to the returned id, matching the pre-refactor behavior exactly (pure
    /// code motion, CLAUDE.md principle #8 — no logic change for the
    /// `exclude: None` caller).
    fn find_or_create_companion_window(
        &mut self,
        idx: usize,
        exclude: Option<crate::window::WindowId>,
    ) -> Option<crate::window::WindowId> {
        // 1. Is this buffer already visible (other than in `exclude`)?
        if let Some(w) = self
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == idx && Some(w.id) != exclude)
        {
            let id = w.id;
            self.ai.target_window_id = Some(id);
            return Some(id);
        }

        // 2. Can we put it in a non-focused, non-dedicated window?
        let focused_id = self.window_mgr.focused_id();
        let win_ids: Vec<_> = self.window_mgr.iter_windows().map(|w| w.id).collect();
        let other = win_ids.into_iter().find(|&wid| {
            wid != focused_id && Some(wid) != exclude && !self.is_dedicated_window(wid)
        });

        if let Some(other_id) = other {
            if let Some(win) = self.window_mgr.window_mut(other_id) {
                win.buffer_idx = idx;
                win.cursor_row = 0;
                win.cursor_col = 0;
            }
            self.ai.work_window.set(Some(other_id));
            self.ai.target_window_id = Some(other_id);
            self.mark_full_redraw();
            return Some(other_id);
        }

        // 2.5: If there's a replaceable window (e.g. dashboard), take it over.
        if let Some(repl_id) = self.find_replaceable_window() {
            if Some(repl_id) != exclude {
                if let Some(win) = self.window_mgr.window_mut(repl_id) {
                    win.buffer_idx = idx;
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                }
                self.ai.work_window.set(Some(repl_id));
                self.ai.target_window_id = Some(repl_id);
                self.mark_full_redraw();
                return Some(repl_id);
            }
        }

        // 3. Fallback: split a window. Prefer a non-conversation window to avoid
        // splitting the tiny *ai-input* pane or the *AI* output pane. (No
        // `exclude` handling needed below: a split always produces a brand
        // new window id, and the conversation-avoidance logic already skips
        // any window showing a protected buffer — which is exactly what
        // `exclude` would be in the `ensure_ai_dispatch_target` caller.)
        let focused_is_conv = self.is_conversation_buffer(self.active_buffer_idx());
        if focused_is_conv {
            // Find any non-conversation window to focus before splitting.
            let non_conv_win = self
                .window_mgr
                .iter_windows()
                .find(|w| !self.is_conversation_buffer(w.buffer_idx))
                .map(|w| w.id);
            if let Some(id) = non_conv_win {
                self.window_mgr.set_focused(id);
            } else if let Some(ref pair) = self.ai.conversation_pair {
                // All windows are conversation. Agent shells are persistent
                // interactive sessions — stealing the output window would
                // permanently replace the conversation display. Skip the steal
                // and fall through to the split attempt below.
                let is_agent_shell = idx < self.buffers.len() && self.buffers[idx].agent_shell;
                if !is_agent_shell {
                    // Non-agent buffer: steal output temporarily (restored on session end).
                    let out_id = pair.output_window_id;
                    if let Some(win) = self.window_mgr.window_mut(out_id) {
                        win.buffer_idx = idx;
                        win.cursor_row = 0;
                        win.cursor_col = 0;
                    }
                    self.ai.work_window.set(Some(out_id));
                    self.ai.target_window_id = Some(out_id);
                    self.mark_full_redraw();
                    return Some(out_id);
                }
                // Agent shell: fall through to split attempt.
            }
        }
        let area = self.default_area();
        let is_agent = idx < self.buffers.len() && self.buffers[idx].agent_shell;
        let ratio = self.agent_display_split_ratio;

        // For agent shells, use split_root to guarantee the shell gets a
        // top-level window beside the entire conversation group, regardless
        // of which conversation pane is focused.
        let split_result = if is_agent {
            self.window_mgr
                .split_root(crate::window::SplitDirection::Vertical, idx, area, ratio)
        } else {
            self.window_mgr.split_with_ratio(
                crate::window::SplitDirection::Vertical,
                idx,
                area,
                ratio,
            )
        };

        match split_result {
            Ok(new_id) => {
                self.ai.work_window.set(Some(new_id));
                self.ai.target_window_id = Some(new_id);
                self.mark_full_redraw();
                Some(new_id)
            }
            Err(_) => {
                // Too small to split — if we are in conversation, we HAVE to steal focus
                // but we try to avoid it.
                if self.is_conversation_buffer(self.active_buffer_idx()) {
                    if self.switch_to_buffer(idx) {
                        Some(self.window_mgr.focused_id())
                    } else {
                        None
                    }
                } else {
                    // Not in conversation, so just keep focus where it is.
                    Some(self.window_mgr.focused_id())
                }
            }
        }
    }

    /// Policy-aware buffer display: routes the buffer to the right window
    /// based on its `BufferKind` and the active `DisplayPolicy`.
    ///
    /// This is the primary entry point for making a buffer visible. It replaces
    /// direct `focused_window_mut().buffer_idx = idx` assignments throughout
    /// the codebase, adding conversation protection and side-window reuse.
    pub fn display_buffer(&mut self, buf_idx: usize) {
        if buf_idx >= self.buffers.len() {
            return;
        }
        // Save the outgoing buffer's mode and restore/reset the incoming
        // one's, mirroring `switch_to_buffer`'s save/sync pair — this is
        // the ROOT buffer-display primitive ~35 call sites across the
        // codebase use directly (open_file, KB/help/git/notification/
        // option-editing navigation, etc.), and until now only
        // `switch_to_buffer` (cycling between already-open buffers) and
        // `display_buffer_and_focus` (a handful of explicit callers) kept
        // per-buffer mode consistent. Every other caller left a stale
        // global `Editor.mode` untouched — e.g. opening a brand-new file
        // while `self.mode` was still `ShellInsert` from an earlier
        // terminal interaction silently routed ALL of that buffer's
        // keypresses through the shell keymap instead of its real one,
        // with no visible symptom beyond "keybindings do nothing." A
        // no-op for the overwhelmingly common case (already in Normal/
        // Insert/Visual mode — `sync_mode_to_buffer`'s fallback only acts
        // when currently `ShellInsert`/`ConversationInput`), so this only
        // changes behavior for the exact stuck-mode scenario it fixes.
        self.save_mode_to_buffer();
        let kind = self.buffers[buf_idx].kind;
        let action = self.display_policy.action_for(kind);
        match action {
            crate::display_policy::DisplayAction::ReplaceFocused => {
                if self.is_conversation_buffer(self.active_buffer_idx()) {
                    self.display_buffer_for_agent(buf_idx);
                } else {
                    let win = self.window_mgr.focused_window_mut();
                    win.buffer_idx = buf_idx;
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                }
            }
            crate::display_policy::DisplayAction::AvoidConversation => {
                if self.is_conversation_buffer(self.active_buffer_idx()) {
                    self.display_buffer_for_agent(buf_idx);
                } else {
                    let win = self.window_mgr.focused_window_mut();
                    win.buffer_idx = buf_idx;
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                }
            }
            crate::display_policy::DisplayAction::ReuseOrSplit { direction, ratio } => {
                // Side-window pattern: reuse existing window of same kind.
                let reuse_win_id = self.find_window_with_kind(kind);
                if let Some(win_id) = reuse_win_id {
                    if let Some(win) = self.window_mgr.window_mut(win_id) {
                        win.buffer_idx = buf_idx;
                        win.cursor_row = 0;
                        win.cursor_col = 0;
                    }
                } else if let Some(repl_id) = self.find_replaceable_window() {
                    // Replace a replaceable window instead of splitting alongside it.
                    if kind
                        != self.buffers[self.window_mgr.window(repl_id).unwrap().buffer_idx].kind
                    {
                        if let Some(win) = self.window_mgr.window_mut(repl_id) {
                            win.buffer_idx = buf_idx;
                            win.cursor_row = 0;
                            win.cursor_col = 0;
                        }
                    } else {
                        self.display_buffer_split(buf_idx, direction, ratio);
                    }
                } else {
                    self.display_buffer_split(buf_idx, direction, ratio);
                }
            }
            crate::display_policy::DisplayAction::Hidden => {}
        }
        self.sync_mode_to_buffer();
        self.mark_full_redraw();
    }

    /// Like `display_buffer` but also sets focus to the window showing the buffer.
    /// Use this when opening a buffer that the user wants to interact with immediately
    /// (e.g. terminal, agenda). Also sets `alternate_buffer_idx`.
    pub fn display_buffer_and_focus(&mut self, buf_idx: usize) {
        if buf_idx >= self.buffers.len() {
            return;
        }
        let prev_idx = self.active_buffer_idx();
        self.save_mode_to_buffer();
        // Save the current window's view state before switching.
        self.window_mgr.focused_window_mut().save_view_state();
        self.display_buffer(buf_idx);
        // Find the window now showing buf_idx and focus it.
        let win_id = self
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == buf_idx)
            .map(|w| w.id);
        if let Some(id) = win_id {
            self.window_mgr.set_focused(id);
        }
        // Restore view state for the new buffer (scroll position, cursor).
        self.window_mgr
            .focused_window_mut()
            .restore_view_state(buf_idx);
        // No forced fallback: if display_buffer() routed the buffer via
        // display_buffer_for_agent (e.g. split_root for agent
        // shells), the buffer is already placed in a new window that may
        // not match the iter_windows search above. Forcing it into the
        // focused window would steal conversation windows.
        if prev_idx != buf_idx {
            self.vi.alternate_buffer_idx = Some(prev_idx);
        }
        self.sync_mode_to_buffer();
    }

    /// Find a window showing a buffer of the given kind (non-conversation).
    /// Excludes windows that are part of the conversation pair (output/input).
    /// Prefers the focused window if it already qualifies (mirrors
    /// `find_replaceable_window`'s "what you see gets replaced" UX) before
    /// falling back to scanning all windows in unspecified order.
    pub(super) fn find_window_with_kind(
        &self,
        kind: crate::BufferKind,
    ) -> Option<crate::window::WindowId> {
        let conv_ids = self
            .ai
            .conversation_pair
            .as_ref()
            .map(|p| [p.output_window_id, p.input_window_id]);
        let qualifies = |w: &crate::window::Window| {
            w.buffer_idx < self.buffers.len()
                && self.buffers[w.buffer_idx].kind == kind
                && !self.is_conversation_buffer(w.buffer_idx)
                && !conv_ids.is_some_and(|ids| ids.contains(&w.id))
        };
        let focused_id = self.window_mgr.focused_id();
        if let Some(fw) = self.window_mgr.window(focused_id) {
            if qualifies(fw) {
                return Some(focused_id);
            }
        }
        self.window_mgr
            .iter_windows()
            .find(|w| qualifies(w))
            .map(|w| w.id)
    }

    /// Split helper for display_buffer: creates a new split.
    /// Group-aware: if focused inside a conversation group, the split wraps the
    /// entire group rather than splitting within it.
    fn display_buffer_split(
        &mut self,
        buf_idx: usize,
        direction: crate::window::SplitDirection,
        ratio: f32,
    ) {
        // Redirect focus away from conversation windows before splitting,
        // so the split happens outside the conversation group.
        if self.is_conversation_buffer(self.active_buffer_idx()) {
            let non_conv_win = self
                .window_mgr
                .iter_windows()
                .find(|w| !self.is_conversation_buffer(w.buffer_idx))
                .map(|w| w.id);
            if let Some(nc_id) = non_conv_win {
                self.window_mgr.set_focused(nc_id);
            } else {
                // All windows are conversation — split_root to place beside the group.
                let area = self.default_area();
                match self.window_mgr.split_root(direction, buf_idx, area, ratio) {
                    Ok(new_win_id) => self.window_mgr.set_focused(new_win_id),
                    Err(_) => {
                        self.display_buffer_for_agent(buf_idx);
                    }
                }
                return;
            }
        }
        let area = self.default_area();
        match self
            .window_mgr
            .split_with_ratio(direction, buf_idx, area, ratio)
        {
            Ok(new_win_id) => {
                self.window_mgr.set_focused(new_win_id);
            }
            Err(_) => {
                self.display_buffer_for_agent(buf_idx);
            }
        }
    }

    /// Open a file without stealing focus from a conversation window.
    ///
    /// The file is opened "hidden" (not assigned to focused window), then
    /// routed via `display_buffer_for_agent`.
    pub fn open_file_non_conversation(&mut self, path: impl AsRef<std::path::Path>) {
        if let Some(new_idx) = self.open_file_hidden(path) {
            self.display_buffer_for_agent(new_idx);
        }
    }

    /// Save current mode to the active buffer before switching away.
    pub fn save_mode_to_buffer(&mut self) {
        let idx = self.active_buffer_idx();
        self.buffers[idx].saved_mode = Some(self.mode);
    }

    /// Sync `self.mode` to the active buffer's kind after a focus/buffer change.
    /// Restores per-buffer `saved_mode` when available; otherwise falls back to
    /// a sensible default based on buffer kind.
    pub fn sync_mode_to_buffer(&mut self) {
        let idx = self.active_buffer_idx();
        self.ensure_buffer_git_branch(idx);
        self.buffer_focus_seq += 1;
        self.buffers[idx].last_focused = self.buffer_focus_seq;
        let kind = self.buffers[idx].kind;

        if let Some(saved) = self.buffers[idx].saved_mode {
            // Validate saved mode is appropriate for the buffer kind.
            let valid = match kind {
                crate::BufferKind::Shell => {
                    matches!(saved, Mode::ShellInsert | Mode::Normal)
                }
                crate::BufferKind::Conversation => {
                    matches!(
                        saved,
                        Mode::ConversationInput | Mode::Normal | Mode::Visual(_)
                    )
                }
                _ => !matches!(saved, Mode::ShellInsert),
            };
            if valid {
                self.set_mode(saved);
                return;
            }
        }

        // No saved mode or invalid — use default.
        match kind {
            crate::BufferKind::Shell => {
                self.set_mode(Mode::ShellInsert);
            }
            _ => {
                if matches!(self.mode, Mode::ShellInsert | Mode::ConversationInput) {
                    self.set_mode(Mode::Normal);
                }
            }
        }
    }

    /// Default area for window operations when we don't have the real terminal size.
    /// The renderer will provide real dimensions at render time.
    pub fn default_area(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        }
    }
}
