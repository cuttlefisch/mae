//! KB, capture, daily, and agenda dispatch commands.

use crate::Mode;

use super::super::Editor;

impl Editor {
    /// Finish a capture: persist, tidy the scratch buffers, return to where the
    /// user was.
    ///
    /// Extracted from `dispatch_kb` (already 332 lines) rather than blessed.
    fn kb_capture_finalize(&mut self) {
        if let Some(cap) = self.kb.capture_state.take() {
            // Phase 3: `save` writes the capture BUFFER to its file, and
            // a store-backed capture has no file -- the node itself is
            // the buffer, already persisted by `kb_create_node` and
            // edited through the node write path. Calling `save` here
            // would at best no-op and at worst prompt for a filename.
            if cap.file_path.is_some() {
                self.dispatch_builtin("save");
            }
            // Remove hidden KB buffer seeded for this node.
            //
            // Only when there IS one: the file backing seeds a hidden KB
            // buffer alongside the file buffer, but a store-backed
            // capture has no file buffer -- the KB buffer is the one the
            // user is looking at, so removing it would close the capture
            // out from under them.
            if let Some(hi) = cap.file_path.as_ref().and_then(|_| {
                self.buffers
                    .iter()
                    .position(|b| b.kb_view().is_some_and(|hv| hv.current == cap.node_id))
            }) {
                self.buffers.remove(hi);
                // Audit #605.2 — pairs with every `buffers.remove()`;
                // without it the Editor's index-keyed maps (syntax, AI
                // target, shell viewports) keep stale indices.
                self.notify_buffer_removed(hi);
                for win in self.window_mgr.iter_windows_mut() {
                    if win.buffer_idx > hi {
                        win.buffer_idx = win.buffer_idx.saturating_sub(1);
                    }
                }
            }
            let ret = cap
                .return_buffer_idx
                .min(self.buffers.len().saturating_sub(1));
            self.display_buffer(ret);
            self.set_status("Capture finalized");
        } else {
            self.set_status("No active capture");
        }
    }

    /// Abandon a capture, discarding the buffer without a save prompt.
    fn kb_capture_abort(&mut self) {
        if let Some(cap) = self.kb.capture_state.take() {
            // Force-kill the capture buffer (no save prompt). For a
            // store-backed capture this IS the KB buffer, which is why
            // the removal below is file-only.
            self.dispatch_builtin("force-kill-buffer");
            // Remove hidden KB buffer seeded for this node.
            //
            // Only when there IS one: the file backing seeds a hidden KB
            // buffer alongside the file buffer, but a store-backed
            // capture has no file buffer -- the KB buffer is the one the
            // user is looking at, so removing it would close the capture
            // out from under them.
            if let Some(hi) = cap.file_path.as_ref().and_then(|_| {
                self.buffers
                    .iter()
                    .position(|b| b.kb_view().is_some_and(|hv| hv.current == cap.node_id))
            }) {
                self.buffers.remove(hi);
                // Audit #605.2 — pairs with every `buffers.remove()`;
                // without it the Editor's index-keyed maps (syntax, AI
                // target, shell viewports) keep stale indices.
                self.notify_buffer_removed(hi);
                for win in self.window_mgr.iter_windows_mut() {
                    if win.buffer_idx > hi {
                        win.buffer_idx = win.buffer_idx.saturating_sub(1);
                    }
                }
            }
            // Delete the file from disk
            if let Some(ref path) = cap.file_path {
                let _ = std::fs::remove_file(path);
            }
            // Remove node from the KB — including the DURABLE store.
            //
            // `kb_create_node` already persisted it, so removing only the
            // in-memory mirrors left an aborted capture to reappear on the next
            // restart. Invisible while capture was file-backed, because deleting
            // the file was the whole story then.
            let _ = self.kb_delete_node(&cap.node_id);
            self.kb.primary.remove(&cap.node_id);
            for kb in self.kb.instances.values_mut() {
                kb.remove(&cap.node_id);
            }
            let ret = cap
                .return_buffer_idx
                .min(self.buffers.len().saturating_sub(1));
            self.display_buffer(ret);
            self.set_status("Capture aborted");
        } else {
            self.set_status("No active capture");
        }
    }

    /// Dispatch KB, capture, daily, and agenda commands.
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_kb(&mut self, name: &str) -> Option<bool> {
        match name {
            "kb-find" | "kb-create" => {
                // Bounded candidate set: all nodes for small KBs (client-filter),
                // or a ranked window for large KBs (lazy, re-queried as you type).
                let nodes = self.kb_find_candidates("");
                self.command_palette =
                    Some(crate::command_palette::CommandPalette::for_kb_find_or_create(&nodes));
                self.set_mode(Mode::CommandPalette);
            }
            "kb-edit-source" => {
                self.help_edit_source();
            }
            "kb-promote" => {
                // Acts on the current KB-view node, mirroring kb-edit-source
                // (#303 interim bridge — see kb_promote_node's doc comment).
                match self.kb_view().map(|v| v.current.clone()) {
                    Some(id) => {
                        if let Err(e) = self.kb_promote_node(&id) {
                            self.set_status(e);
                        } else {
                            // Refresh the rendered view so it reflects the
                            // node's new (primary) provenance immediately.
                            if let Some(buf_idx) = self
                                .buffers
                                .iter()
                                .position(|b| b.kind == crate::BufferKind::Kb)
                            {
                                self.kb_populate_buffer(buf_idx);
                            }
                        }
                    }
                    None => self.set_status("Not in a help buffer"),
                }
            }
            "kb-init-project" => {
                // ADR-058 Phase B: the explicit path, and also what the
                // provisioning-suggestion notification's "Register" action invokes.
                match self.kb_init_project(None) {
                    Ok(result) => self.set_status(result.status_summary()),
                    Err(e) => self.set_status(format!("KB init-project error: {e}")),
                }
            }
            "kb-decline-project-provisioning" => match self.kb_decline_project_provisioning(None) {
                Ok(()) => self.set_status("Won't ask again for this project's KB"),
                Err(e) => self.set_status(format!("KB decline-provisioning error: {e}")),
            },
            "kb-insert-link" => {
                let nodes = self.kb_all_node_pairs();
                self.command_palette = Some(
                    crate::command_palette::CommandPalette::for_kb_insert_link(&nodes),
                );
                self.set_mode(Mode::CommandPalette);
            }
            "kb-delete" => {
                self.set_mode(Mode::Command);
                self.vi.command_line = "kb-delete ".to_string();
                self.vi.command_cursor = self.vi.command_line.len();
            }
            "kb-register" => {
                self.set_mode(Mode::Command);
                self.vi.command_line = "kb-register ".to_string();
                self.vi.command_cursor = self.vi.command_line.len();
            }
            "kb-reimport" => {
                self.set_mode(Mode::Command);
                self.vi.command_line = "kb-reimport ".to_string();
                self.vi.command_cursor = self.vi.command_line.len();
            }
            // Story D / R8 — the import audit. Both take an argument, so the
            // no-arg dispatch prefills command mode the same way `kb-reimport`
            // does rather than silently doing nothing.
            // Both take a KB name, so the no-arg dispatch prefills command mode
            // the way `kb-reimport` does rather than silently doing nothing.
            "kb-detach" | "kb-attach" | "kb-retire-archive" | "kb-new" => {
                self.set_mode(Mode::Command);
                self.vi.command_line = format!("{name} ");
                self.vi.command_cursor = self.vi.command_line.len();
            }
            "kb-relink" => match self.kb_relink_project(None) {
                Ok(msg) | Err(msg) => self.set_status(msg),
            },
            "kb-import-plan" | "kb-import-verify" => {
                self.set_mode(Mode::Command);
                self.vi.command_line = format!("{name} ");
                self.vi.command_cursor = self.vi.command_line.len();
            }
            "kb-instances" => {
                self.show_kb_instances();
            }
            "kb-save" => {
                self.set_status("Usage: :kb-save <path>");
            }
            "kb-load" => {
                self.set_status("Usage: :kb-load <path>");
            }
            "kb-ingest" => {
                self.set_status("Usage: :kb-ingest <directory>");
            }
            "kb-rebuild" => {
                self.kb.primary =
                    crate::kb_seed::seed_kb(&self.commands, &self.keymaps, &self.hooks);
                let count = if let Some(q) = self.kb.query_layer() {
                    q.list_ids(None)
                        .map(|ids| ids.len())
                        .unwrap_or_else(|_| self.kb.primary.list_ids(None).len())
                } else {
                    self.kb.primary.list_ids(None).len()
                };
                self.set_status(format!("KB rebuilt: {} nodes", count));
            }
            "kb-audit" => {
                self.show_kb_audit_report();
            }
            "kb-health" => {
                self.show_kb_health_report();
            }
            "kb-cleanup-orphans" => {
                let count = self.kb_cleanup_orphans();
                if count == 0 {
                    self.set_status("No orphan user notes to remove");
                } else {
                    self.set_status(format!("Removed {} orphan note(s)", count));
                }
            }
            "kb-migrate-stranded" => {
                let (removed, diverged) = self.kb_migrate_stranded_federation_nodes();
                self.set_status(match (removed, diverged) {
                    (0, 0) => "No stranded primary-KB nodes found".to_string(),
                    (r, 0) => format!("Removed {r} stranded node(s) superseded by their joined instance"),
                    (0, d) => format!("{d} stranded node(s) diverge from their instance — see notifications"),
                    (r, d) => format!(
                        "Removed {r} stranded node(s); {d} diverge from their instance — see notifications"
                    ),
                });
            }
            "kb-agenda" => {
                self.set_mode(Mode::Command);
                self.vi.command_line = "kb-agenda ".to_string();
                self.vi.command_cursor = self.vi.command_line.len();
            }
            "kb-history" => {
                self.set_mode(Mode::Command);
                self.vi.command_line = "kb-history ".to_string();
                self.vi.command_cursor = self.vi.command_line.len();
            }
            "kb-restore" => {
                self.set_mode(Mode::Command);
                self.vi.command_line = "kb-restore ".to_string();
                self.vi.command_cursor = self.vi.command_line.len();
            }
            "kb-raw-query" => {
                self.set_mode(Mode::Command);
                self.vi.command_line = "kb-raw-query ".to_string();
                self.vi.command_cursor = self.vi.command_line.len();
            }
            "kb-narrow" => {
                self.kb_narrow_meta();
            }
            "kb-set-ai-residency" => {
                // :kb-set-ai-residency <kb-id|primary> <open|local_models_only>
                let line = self.vi.command_line.trim().to_string();
                let mut parts = line.split_whitespace();
                let kb_id = parts.next().unwrap_or("").to_string();
                let policy = parts.next().and_then(|p| match p {
                    "open" => Some(mae_kb::federation::AiResidency::Open),
                    "local_models_only" | "local-models-only" => {
                        Some(mae_kb::federation::AiResidency::LocalModelsOnly)
                    }
                    _ => None,
                });
                match (kb_id.is_empty(), policy) {
                    (false, Some(policy)) => match self.kb_set_ai_residency(&kb_id, policy) {
                        Ok(msg) => self.set_status(msg),
                        Err(e) => self.set_status(e),
                    },
                    _ => self.set_status(
                        "usage: :kb-set-ai-residency <kb-id|primary> <open|local_models_only>"
                            .to_string(),
                    ),
                }
            }
            "kb-set-role" => {
                // :kb-set-role <node-id> <source|atom|molecule|hub>
                let line = self.vi.command_line.trim().to_string();
                let mut parts = line.split_whitespace();
                let id = parts.next().unwrap_or("").to_string();
                let role = parts.next().unwrap_or("").to_string();
                if id.is_empty() || role.is_empty() {
                    self.set_status(
                        "usage: :kb-set-role <node-id> <source|atom|molecule|hub>".to_string(),
                    );
                } else {
                    match self.kb_set_role(&id, &role) {
                        Ok(msg) => self.set_status(msg),
                        Err(e) => self.set_status(e),
                    }
                }
            }
            "kb-widen" => {
                self.kb_widen_meta();
            }
            "kb-preview" => {
                // Mirrors "lsp-hover": pressing the manual trigger again
                // while the popup is already showing scrolls it instead of
                // re-fetching (see dispatch_lsp's "lsp-hover" arm).
                if self.kb_preview_popup().is_some() {
                    self.kb_preview_scroll_down();
                } else if !self.kb_preview_show_at_cursor(true) {
                    self.set_status("No KB link under cursor");
                }
            }
            "dismiss-kb-preview-popup" => self.kb_preview_dismiss(),
            "kb-preview-scroll-down" => self.kb_preview_scroll_down(),
            "kb-preview-scroll-up" => self.kb_preview_scroll_up(),
            "capture-finalize" => self.kb_capture_finalize(),
            "capture-abort" => self.kb_capture_abort(),
            "daily-goto-today" => {
                if let Err(e) = self.kb_goto_daily_today() {
                    self.set_status(format!("Daily: {}", e));
                }
            }
            "daily-goto-yesterday" => {
                if let Err(e) = self.kb_goto_daily_yesterday() {
                    self.set_status(format!("Daily: {}", e));
                }
            }
            "daily-goto-date" => {
                self.mini_dialog = Some(crate::command_palette::MiniDialogState::single_input(
                    "Date (YYYY-MM-DD):",
                    "",
                    "",
                    crate::command_palette::MiniDialogContext::DailyGotoDate,
                ));
                self.set_mode(crate::Mode::Command);
            }
            "daily-prev" => {
                if let Err(e) = self.kb_daily_prev() {
                    self.set_status(format!("Daily: {}", e));
                }
            }
            "daily-next" => {
                if let Err(e) = self.kb_daily_next() {
                    self.set_status(format!("Daily: {}", e));
                }
            }
            "ai-save" => {
                self.set_status("Usage: :ai-save <path>");
            }
            "ai-load" => {
                self.set_status("Usage: :ai-load <path>");
            }
            "open-agenda" => {
                self.open_agenda(crate::agenda_view::AgendaFilter::default());
            }
            "agenda-goto" => {
                self.agenda_goto();
            }
            "agenda-refresh" => {
                self.agenda_refresh();
            }
            "agenda-filter-todo" => {
                self.agenda_filter_todo();
            }
            "agenda-filter-priority" => {
                self.agenda_filter_priority();
            }
            "agenda-add" => {
                self.set_status("Use :agenda-add <path> to add agenda files");
            }
            "agenda-remove" => {
                self.set_status("Use :agenda-remove <path> to remove agenda files");
            }
            "agenda-list" => {
                self.agenda_list_paths();
            }
            "agenda-ingest" => {
                self.ingest_agenda_files();
                self.set_status("Agenda files re-ingested");
            }
            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }
}
