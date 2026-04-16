use crate::theme::bundled_theme_names;

use super::Editor;

impl Editor {
    /// Parse and execute a command-line string (the text after ':').
    pub fn execute_command(&mut self, cmd: &str) -> bool {
        let cmd = cmd.trim();
        let (command, args) = match cmd.split_once(' ') {
            Some((c, a)) => (c, Some(a.trim())),
            None => (cmd, None),
        };

        match command {
            "w" => {
                if let Some(path) = args {
                    let idx = self.active_buffer_idx();
                    self.buffers[idx].set_file_path(std::path::PathBuf::from(path));
                }
                self.save_current_buffer();
                true
            }
            "q" => {
                if self.active_buffer().modified {
                    self.set_status("No write since last change (add ! to override)");
                } else {
                    self.running = false;
                }
                true
            }
            "q!" => {
                self.running = false;
                true
            }
            "wq" | "x" => {
                self.save_current_buffer();
                if self.running && !self.active_buffer().modified {
                    self.running = false;
                }
                true
            }
            "e" => {
                if let Some(path) = args {
                    self.open_file(path);
                } else {
                    self.set_status("Usage: :e <filename>");
                }
                true
            }
            "vsplit" => {
                self.dispatch_builtin("split-vertical");
                true
            }
            "split" => {
                self.dispatch_builtin("split-horizontal");
                true
            }
            "close" => {
                self.dispatch_builtin("close-window");
                true
            }
            "messages" => {
                self.dispatch_builtin("view-messages");
                true
            }
            "theme" => {
                if let Some(name) = args {
                    self.set_theme_by_name(name);
                } else {
                    let names = bundled_theme_names().join(", ");
                    self.set_status(format!("Usage: :theme <name>  Available: {}", names));
                }
                true
            }
            "noh" | "nohlsearch" => {
                self.search_state.highlight_active = false;
                true
            }
            _ => {
                // Shell escape: :!cmd
                if let Some(shell_cmd) = cmd.strip_prefix('!') {
                    let shell_cmd = shell_cmd.trim();
                    if shell_cmd.is_empty() {
                        self.set_status("Usage: :!<command>");
                        return true;
                    }
                    match std::process::Command::new("sh")
                        .arg("-c")
                        .arg(shell_cmd)
                        .output()
                    {
                        Ok(output) => {
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            let result = if !stdout.is_empty() {
                                stdout.trim().to_string()
                            } else if !stderr.is_empty() {
                                stderr.trim().to_string()
                            } else {
                                format!("(exit {})", output.status.code().unwrap_or(-1))
                            };
                            self.set_status(result);
                        }
                        Err(e) => {
                            self.set_status(format!("Shell error: {}", e));
                        }
                    }
                    return true;
                }
                // Check for substitute commands: s/.../.../  or %s/.../.../
                if cmd.starts_with("s/") || cmd.starts_with("%s/") {
                    self.execute_substitute_command(cmd);
                    return true;
                }
                self.set_status(format!("Unknown command: {}", command));
                false
            }
        }
    }
}
