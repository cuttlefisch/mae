//! KB, capture, daily, and agenda dispatch commands.

use crate::Mode;

use super::super::Editor;

impl Editor {
    /// Dispatch KB, capture, daily, and agenda commands.
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_kb(&mut self, name: &str) -> Option<bool> {
        match name {
            "kb-find" | "kb-create" => {
                let nodes = self.kb_all_node_triples();
                self.command_palette =
                    Some(crate::command_palette::CommandPalette::for_kb_find_or_create(&nodes));
                self.set_mode(Mode::CommandPalette);
            }
            "kb-edit-source" => {
                self.help_edit_source();
            }
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
                let count = self.kb.primary.list_ids(None).len();
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
            "capture-finalize" => {
                if let Some(cap) = self.kb.capture_state.take() {
                    self.dispatch_builtin("save");
                    // Remove hidden KB buffer seeded for this node
                    if let Some(hi) = self
                        .buffers
                        .iter()
                        .position(|b| b.kb_view().is_some_and(|hv| hv.current == cap.node_id))
                    {
                        self.buffers.remove(hi);
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
            "capture-abort" => {
                if let Some(cap) = self.kb.capture_state.take() {
                    // Force-kill the capture buffer (no save prompt)
                    self.dispatch_builtin("force-kill-buffer");
                    // Remove hidden KB buffer seeded for this node
                    if let Some(hi) = self
                        .buffers
                        .iter()
                        .position(|b| b.kb_view().is_some_and(|hv| hv.current == cap.node_id))
                    {
                        self.buffers.remove(hi);
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
                    // Remove node from KB
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
