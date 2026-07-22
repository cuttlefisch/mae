//! File operations: open, save, close, rename, file picker.

use crate::buffer::Buffer;
use crate::file_picker::FilePicker;
use crate::Mode;

use super::super::Editor;

impl Editor {
    /// Dispatch file operation commands. Returns `Some(true)` if handled.
    pub(super) fn dispatch_file(&mut self, name: &str) -> Option<bool> {
        match name {
            "save" => self.save_current_buffer(),
            "quit" => {
                self.execute_command("q");
            }
            "force-quit" => {
                self.execute_command("q!");
            }
            "save-and-quit" => {
                self.execute_command("wq");
            }
            "save-all-and-quit" => {
                self.dispatch_builtin("save-all-buffers");
                self.execute_command("q");
            }
            "next-buffer" => {
                if self.buffers.len() <= 1 {
                    return Some(true);
                }
                self.save_mode_to_buffer();
                let prev_idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                win.save_view_state();
                let new_idx = (win.buffer_idx + 1) % self.buffers.len();
                win.restore_view_state(new_idx);
                self.vi.alternate_buffer_idx = Some(prev_idx);
                let name = self.buffers[new_idx].name.clone();
                self.set_status(format!("Buffer: {}", name));
                self.sync_mode_to_buffer();
            }
            "prev-buffer" => {
                if self.buffers.len() <= 1 {
                    return Some(true);
                }
                self.save_mode_to_buffer();
                let prev_idx = self.active_buffer_idx();
                let count = self.buffers.len();
                let win = self.window_mgr.focused_window_mut();
                win.save_view_state();
                let new_idx = (win.buffer_idx + count - 1) % count;
                win.restore_view_state(new_idx);
                self.vi.alternate_buffer_idx = Some(prev_idx);
                let name = self.buffers[new_idx].name.clone();
                self.set_status(format!("Buffer: {}", name));
                self.sync_mode_to_buffer();
            }
            "new-buffer" => {
                let mut buf = Buffer::new();
                let n = self
                    .buffers
                    .iter()
                    .filter(|b| b.name.starts_with("[scratch"))
                    .count();
                if n > 0 {
                    buf.name = format!("[scratch-{}]", n);
                }
                let new_idx = self.buffers.len();
                self.buffers.push(buf);
                self.display_buffer_and_focus(new_idx);
                self.set_status("New buffer");
            }
            "kill-buffer" => {
                let idx = self.active_buffer_idx();
                if self.buffers[idx].modified {
                    self.set_status("Buffer has unsaved changes (save first or use :q!)");
                } else {
                    self.kill_buffer_at(idx);
                }
            }
            "force-kill-buffer" => {
                let idx = self.active_buffer_idx();
                self.kill_buffer_at(idx);
            }
            "switch-buffer" => {
                let mut entries: Vec<(u64, String)> = self
                    .buffers
                    .iter()
                    .filter(|b| {
                        !matches!(
                            b.kind,
                            crate::BufferKind::ShellSelect | crate::BufferKind::Demo
                        )
                    })
                    .map(|b| (b.last_focused, b.name.clone()))
                    .collect();
                // Most-recently-focused first (usability gap fix, no tracked
                // issue -- users mostly cycle between a handful of recent
                // buffers, not an arbitrary creation-order list). Stable
                // sort: buffers never explicitly focused (last_focused == 0,
                // e.g. several brand-new buffers) keep their prior relative
                // order among themselves.
                entries.sort_by_key(|(seq, _)| std::cmp::Reverse(*seq));
                let mut names: Vec<String> = entries.into_iter().map(|(_, name)| name).collect();
                if !names.iter().any(|n| n == "*Messages*") {
                    names.push("*Messages*".to_string());
                }
                let name_refs: Vec<&str> = names.iter().map(|s: &String| s.as_str()).collect();
                self.command_palette = Some(crate::command_palette::CommandPalette::for_buffers(
                    &name_refs,
                ));
                self.set_mode(crate::Mode::CommandPalette);
            }
            "find-file" => {
                let root = self
                    .active_project_root()
                    .map(|p| p.to_path_buf())
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_default();
                let mut picker = FilePicker::scan(
                    &root,
                    self.file_picker_max_depth,
                    self.file_picker_max_candidates,
                );
                picker.reorder_by_recency(self.recent_files.list());
                self.file_picker = Some(picker);
                self.set_mode(Mode::FilePicker);
            }
            "file-browser" => {
                let start = self
                    .active_buffer()
                    .file_path()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_default();
                self.file_browser = Some(crate::FileBrowser::open(&start));
                self.set_mode(Mode::FileBrowser);
            }
            "recent-files" => self.recent_files_palette(),
            "goto-file-under-cursor" => self.goto_file_under_cursor(),
            "yank-file-path" => {
                if let Some(path) = self.active_buffer().file_path() {
                    let path_str = path.display().to_string();
                    self.write_named_register('+', &path_str);
                    self.set_status(format!("Yanked: {}", path_str));
                } else {
                    self.set_status("Buffer has no file path");
                }
            }
            "rename-file" => {
                use crate::command_palette::{MiniDialogContext, MiniDialogState};
                if let Some(path) = self.active_buffer().file_path().map(|p| p.to_path_buf()) {
                    let ps = path.display().to_string();
                    self.mini_dialog = Some(MiniDialogState::single_input(
                        "New path",
                        &ps,
                        "path",
                        MiniDialogContext::FileRename { old_path: path },
                    ));
                    self.set_mode(crate::Mode::CommandPalette);
                } else {
                    self.set_status("Buffer has no file path");
                }
            }
            "copy-this-file" => {
                use crate::command_palette::{MiniDialogContext, MiniDialogState};
                if let Some(path) = self.active_buffer().file_path().map(|p| p.to_path_buf()) {
                    let ps = path.display().to_string();
                    self.mini_dialog = Some(MiniDialogState::single_input(
                        "Copy to",
                        &ps,
                        "destination path",
                        MiniDialogContext::FileCopy { src_path: path },
                    ));
                    self.set_mode(crate::Mode::CommandPalette);
                } else {
                    self.set_status("Buffer has no file path");
                }
            }
            "save-as" => {
                use crate::command_palette::{MiniDialogContext, MiniDialogState};
                self.mini_dialog = Some(MiniDialogState::single_input(
                    "Save as",
                    "",
                    "path",
                    MiniDialogContext::FileSaveAs,
                ));
                self.set_mode(crate::Mode::CommandPalette);
            }
            "kill-other-buffers" => {
                let active = self.active_buffer_idx();
                let to_remove: Vec<usize> = (0..self.buffers.len())
                    .filter(|&i| {
                        i != active
                            && !self.buffers[i].modified
                            && !self.buffers[i].kind.is_sidebar()
                            && self.buffers[i].kind != crate::BufferKind::Dashboard
                    })
                    .collect();
                let killed = to_remove.len();
                for &i in to_remove.iter().rev() {
                    self.buffers.remove(i);
                    self.notify_buffer_removed(i);
                }
                self.ensure_scratch_exists();
                let buf_count = self.buffers.len();
                for win in self.window_mgr.iter_windows_mut() {
                    if win.buffer_idx >= buf_count {
                        win.buffer_idx = buf_count.saturating_sub(1);
                    }
                }
                self.set_status(format!("Killed {} buffer(s)", killed));
            }
            "save-all-buffers" => {
                let (saved, errors) = self.save_all_modified_buffers();
                if errors.is_empty() {
                    self.set_status(format!("Saved {} buffer(s)", saved));
                } else {
                    self.set_status(format!("Saved {}, errors: {}", saved, errors.join(", ")));
                }
            }
            "revert-buffer" => {
                let idx = self.active_buffer_idx();
                if let Some(path) = self.buffers[idx].file_path().map(|p| p.to_path_buf()) {
                    self.fire_hook("before-revert");
                    match Buffer::from_file(&path) {
                        Ok(buf) => {
                            let name = buf.name.clone();
                            self.buffers[idx] = buf;
                            self.window_mgr.focused_window_mut().cursor_row = 0;
                            self.window_mgr.focused_window_mut().cursor_col = 0;
                            self.fire_hook("after-revert");
                            self.set_status(format!("Reverted: {}", name));
                        }
                        Err(e) => self.set_status(format!("Revert failed: {}", e)),
                    }
                } else {
                    self.set_status("Buffer has no file path to revert from");
                }
            }
            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }
}
