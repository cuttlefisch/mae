use std::path::Path;

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
            "diagnostics" | "diag" => {
                self.dispatch_builtin("lsp-show-diagnostics");
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
            "ai-save" => {
                self.dispatch_path_op(args, "ai-save", |ed, p| ed.ai_save(p), "Saved", "to");
                true
            }
            "ai-load" => {
                self.dispatch_path_op(args, "ai-load", |ed, p| ed.ai_load(p), "Loaded", "from");
                true
            }
            "debug-start" => {
                let arg_str = args.unwrap_or("");
                let mut parts = arg_str.split_whitespace();
                let adapter = parts.next();
                let program = parts.next();
                let extra_args: Vec<String> = parts.map(|s| s.to_string()).collect();
                match (adapter, program) {
                    (Some(adapter), Some(program)) => {
                        if let Err(msg) =
                            self.dap_start_with_adapter(adapter, program, &extra_args)
                        {
                            self.set_status(msg);
                        }
                    }
                    _ => {
                        self.set_status(
                            "Usage: :debug-start <adapter> <program> [args...]  — adapters: lldb, debugpy, codelldb",
                        );
                    }
                }
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
                // Final fallback: dispatch any registered builtin command by
                // name. This lets `:debug-stop`, `:debug-continue`, etc. work
                // without explicit `:`-arms, and is the foundation for making
                // every command also invokable from `:`.
                if self.dispatch_builtin(command) {
                    return true;
                }
                self.set_status(format!("Unknown command: {}", command));
                false
            }
        }
    }

    /// Shared handler for `:<cmd> <path>` style commands backed by a
    /// `Result<usize, String>`-returning operation. Extracts the path
    /// argument, surfaces usage on empty input, and formats a uniform
    /// success/error status message (e.g. "Saved 42 entries to foo.json").
    fn dispatch_path_op(
        &mut self,
        args: Option<&str>,
        cmd: &str,
        op: impl FnOnce(&mut Editor, &Path) -> Result<usize, String>,
        verb: &str,
        preposition: &str,
    ) {
        let path_str = args.unwrap_or("").trim();
        if path_str.is_empty() {
            self.set_status(format!("Usage: :{} <path>", cmd));
            return;
        }
        let path = Path::new(path_str);
        match op(self, path) {
            Ok(n) => self.set_status(format!(
                "{} {} entries {} {}",
                verb,
                n,
                preposition,
                path.display()
            )),
            Err(e) => self.set_status(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_start_command_without_args_shows_usage() {
        let mut ed = Editor::new();
        ed.execute_command("debug-start");
        assert!(ed.status_msg.to_lowercase().contains("usage"));
        assert!(ed.pending_dap_intents.is_empty());
    }

    #[test]
    fn debug_start_command_queues_intent() {
        let mut ed = Editor::new();
        ed.execute_command("debug-start lldb /bin/ls");
        assert_eq!(ed.pending_dap_intents.len(), 1);
    }

    #[test]
    fn debug_start_command_unknown_adapter_sets_status() {
        let mut ed = Editor::new();
        ed.execute_command("debug-start bogus /bin/ls");
        assert!(ed.status_msg.contains("Unknown adapter"));
        assert!(ed.pending_dap_intents.is_empty());
    }

    #[test]
    fn ai_save_without_args_shows_usage() {
        let mut ed = Editor::new();
        ed.execute_command("ai-save");
        assert!(ed.status_msg.to_lowercase().contains("usage"));
    }

    #[test]
    fn ai_save_without_conversation_sets_error() {
        let mut ed = Editor::new();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        ed.execute_command(&format!("ai-save {}", tmp.path().display()));
        assert!(ed.status_msg.contains("No conversation"));
    }

    #[test]
    fn ai_load_without_args_shows_usage() {
        let mut ed = Editor::new();
        ed.execute_command("ai-load");
        assert!(ed.status_msg.to_lowercase().contains("usage"));
    }

    #[test]
    fn ai_save_and_load_round_trip_via_commands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conv.json");

        let mut ed = Editor::new();
        ed.open_conversation_buffer();
        ed.conversation_mut().unwrap().push_user("round-trip");

        ed.execute_command(&format!("ai-save {}", path.display()));
        assert!(ed.status_msg.contains("Saved 1 entries"));
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("round-trip"));

        // Mutate, then reload: load must replace, not merge.
        ed.conversation_mut().unwrap().push_user("to-be-replaced");
        assert_eq!(ed.conversation().unwrap().entries.len(), 2);

        ed.execute_command(&format!("ai-load {}", path.display()));
        assert!(ed.status_msg.contains("Loaded 1 entries"));
        assert_eq!(ed.conversation().unwrap().entries.len(), 1);
    }
}
