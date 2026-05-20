//! Terminal / shell dispatch commands.

use crate::buffer::Buffer;
use crate::Mode;

use super::super::Editor;

impl Editor {
    /// Dispatch terminal and shell commands.
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_terminal(&mut self, name: &str) -> Option<bool> {
        match name {
            "terminal" => {
                let shell_name = format!("*Terminal {}*", self.buffers.len());
                let buf = Buffer::new_shell(shell_name);
                self.buffers.push(buf);
                let idx = self.buffers.len() - 1;
                self.shell.spawns.push(idx);
                self.display_buffer_and_focus(idx);
                self.set_mode(Mode::ShellInsert);
            }
            "terminal-reset" => {
                let idx = self.active_buffer_idx();
                if self.buffers[idx].kind == crate::BufferKind::Shell {
                    self.shell.resets.push(idx);
                    self.set_status("Terminal reset");
                } else {
                    self.set_status("Not a terminal buffer");
                }
            }
            "shell-normal-mode" => {
                self.set_mode(Mode::Normal);
                self.set_status("Terminal: normal mode");
            }
            "terminal-close" => {
                let idx = self.active_buffer_idx();
                if self.buffers[idx].kind == crate::BufferKind::Shell {
                    self.shell.closes.push(idx);
                    self.set_mode(Mode::Normal);
                } else {
                    self.set_status("Not a terminal buffer");
                }
            }
            "shell-scroll-page-up" => {
                self.shell.scroll = Some(self.focused_viewport_height() as i32);
            }
            "shell-scroll-page-down" => {
                self.shell.scroll = Some(-(self.focused_viewport_height() as i32));
            }
            "shell-scroll-to-bottom" => {
                self.shell.scroll = Some(0);
            }
            "shell-select-mode" => {
                let buf_idx = self.active_buffer_idx();
                if self.buffers[buf_idx].kind != crate::BufferKind::Shell {
                    self.set_status("Not a shell buffer");
                } else {
                    // Read scrollback from cached shell viewport data.
                    let content = if let Some(viewport) = self.shell.viewports.get(&buf_idx) {
                        viewport.join("\n")
                    } else {
                        String::new()
                    };

                    if content.is_empty() {
                        self.set_status("No shell output to select");
                    } else {
                        // Reuse an existing *shell-select* buffer or create one.
                        let existing = self.buffers.iter().position(|b| b.name == "*shell-select*");
                        let new_idx = if let Some(i) = existing {
                            self.buffers[i].replace_contents(&content);
                            self.buffers[i].read_only = true;
                            self.buffers[i].kind = crate::BufferKind::ShellSelect;
                            i
                        } else {
                            let mut buf = crate::buffer::Buffer::new();
                            buf.replace_contents(&content);
                            buf.name = "*shell-select*".into();
                            buf.kind = crate::BufferKind::ShellSelect;
                            buf.modified = false;
                            buf.read_only = true;
                            self.buffers.push(buf);
                            self.buffers.len() - 1
                        };

                        // Record the shell buffer as alternate so close returns to it.
                        self.vi.alternate_buffer_idx = Some(buf_idx);
                        self.display_buffer(new_idx);
                        // Move cursor to end of buffer so user sees most recent output.
                        let line_count = self.buffers[new_idx].display_line_count();
                        if line_count > 0 {
                            let win = self.window_mgr.focused_window_mut();
                            win.cursor_row = line_count.saturating_sub(1);
                        }
                        self.mark_full_redraw();
                        self.set_status(
                            "Shell select mode — use v to select, y to yank, q/Esc to exit",
                        );
                    }
                }
            }
            "close-shell-select" => {
                let select_idx = self
                    .buffers
                    .iter()
                    .position(|b| b.kind == crate::BufferKind::ShellSelect);
                if let Some(idx) = select_idx {
                    // Switch to alternate buffer (the shell), or first non-select buffer.
                    let dest = self
                        .vi
                        .alternate_buffer_idx
                        .filter(|&i| i != idx && i < self.buffers.len())
                        .or_else(|| {
                            self.buffers
                                .iter()
                                .position(|b| b.kind != crate::BufferKind::ShellSelect)
                        })
                        .unwrap_or(0);
                    for win in self.window_mgr.iter_windows_mut() {
                        if win.buffer_idx == idx {
                            win.buffer_idx = dest;
                            win.cursor_row = 0;
                            win.cursor_col = 0;
                        }
                    }
                    self.buffers.remove(idx);
                    self.notify_buffer_removed(idx);
                    for win in self.window_mgr.iter_windows_mut() {
                        if win.buffer_idx > idx {
                            win.buffer_idx -= 1;
                        }
                    }
                    self.sync_mode_to_buffer();
                    self.mark_full_redraw();
                }
            }
            "send-to-shell" => {
                self.send_line_to_shell();
            }
            "send-region-to-shell" => {
                self.send_region_to_shell();
            }
            "terminal-here" => {
                // Open terminal in current buffer's file directory.
                let idx = self.active_buffer_idx();
                let cwd = self.buffers[idx]
                    .file_path()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .or_else(|| self.active_project_root().map(|p| p.to_path_buf()));
                if let Some(dir) = cwd {
                    let shell_name = format!("*Terminal {}*", self.buffers.len());
                    let buf = Buffer::new_shell(shell_name);
                    self.buffers.push(buf);
                    let shell_idx = self.buffers.len() - 1;
                    self.shell.spawns.push(shell_idx);
                    self.shell.cwds.insert(shell_idx, dir.clone());
                    self.display_buffer_and_focus(shell_idx);
                    self.set_mode(Mode::ShellInsert);
                    self.set_status(format!("Terminal: {}", dir.display()));
                } else {
                    // Fall back to regular terminal.
                    self.dispatch_builtin("terminal");
                }
            }
            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }
}
