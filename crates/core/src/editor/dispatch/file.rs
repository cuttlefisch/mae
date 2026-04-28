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
            "next-buffer" => {
                if self.buffers.len() <= 1 {
                    return Some(true);
                }
                self.save_mode_to_buffer();
                let prev_idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                win.buffer_idx = (win.buffer_idx + 1) % self.buffers.len();
                win.cursor_row = 0;
                win.cursor_col = 0;
                self.alternate_buffer_idx = Some(prev_idx);
                let name = self.buffers[win.buffer_idx].name.clone();
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
                win.buffer_idx = (win.buffer_idx + count - 1) % count;
                win.cursor_row = 0;
                win.cursor_col = 0;
                self.alternate_buffer_idx = Some(prev_idx);
                let name = self.buffers[win.buffer_idx].name.clone();
                self.set_status(format!("Buffer: {}", name));
                self.sync_mode_to_buffer();
            }
            "new-buffer" => {
                let prev_idx = self.active_buffer_idx();
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
                let win = self.window_mgr.focused_window_mut();
                win.buffer_idx = new_idx;
                win.cursor_row = 0;
                win.cursor_col = 0;
                self.alternate_buffer_idx = Some(prev_idx);
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
                let mut names: Vec<String> = self.buffers.iter().map(|b| b.name.clone()).collect();
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
                self.file_picker = Some(FilePicker::scan(&root));
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
                let path_str = self
                    .active_buffer()
                    .file_path()
                    .map(|p| p.display().to_string());
                if let Some(ps) = path_str {
                    self.set_mode(crate::Mode::Command);
                    self.command_line = format!("rename {}", ps);
                    self.command_cursor = self.command_line.len();
                    self.set_status("Rename file: edit path and press Enter");
                } else {
                    self.set_status("Buffer has no file path");
                }
            }
            "save-as" => {
                self.set_mode(crate::Mode::Command);
                self.command_line = "saveas ".to_string();
                self.command_cursor = self.command_line.len();
                self.set_status("Save as: enter path and press Enter");
            }
            "kill-other-buffers" => {
                let active = self.active_buffer_idx();
                let to_remove: Vec<usize> = (0..self.buffers.len())
                    .filter(|&i| i != active && !self.buffers[i].modified)
                    .collect();
                let killed = to_remove.len();
                for &i in to_remove.iter().rev() {
                    self.buffers.remove(i);
                    self.adjust_ai_target_after_remove(i);
                }
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
                    match Buffer::from_file(&path) {
                        Ok(buf) => {
                            let name = buf.name.clone();
                            self.buffers[idx] = buf;
                            self.window_mgr.focused_window_mut().cursor_row = 0;
                            self.window_mgr.focused_window_mut().cursor_col = 0;
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
        Some(true)
    }
}
