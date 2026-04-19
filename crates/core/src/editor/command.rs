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
            "help" => {
                // `:help`  → index; `:help <topic>` → open KB node `topic`
                // with the same namespace-fallback the AI uses: first try
                // the literal id, then `cmd:<topic>`, then `concept:<topic>`.
                match args.map(str::trim).filter(|s| !s.is_empty()) {
                    None => self.open_help_at("index"),
                    Some(topic) => {
                        let candidates = [
                            topic.to_string(),
                            format!("cmd:{}", topic),
                            format!("concept:{}", topic),
                        ];
                        let found = candidates.iter().find(|id| self.kb.contains(id));
                        match found {
                            Some(id) => self.open_help_at(id),
                            None => self.set_status(format!("No help for: {}", topic)),
                        }
                    }
                }
                true
            }
            "describe-command" => {
                let Some(name) = args.map(str::trim).filter(|s| !s.is_empty()) else {
                    self.set_status("Usage: :describe-command <name>");
                    return true;
                };
                let id = format!("cmd:{}", name);
                if self.kb.contains(&id) {
                    self.open_help_at(&id);
                } else {
                    self.set_status(format!("Unknown command: {}", name));
                }
                true
            }
            "diagnostics" | "diag" => {
                self.dispatch_builtin("lsp-show-diagnostics");
                true
            }
            "changes" => {
                self.dispatch_builtin("show-changes-buffer");
                true
            }
            "reg" | "registers" | "display-registers" => {
                self.dispatch_builtin("show-registers");
                true
            }
            "kb-ingest" | "kb-ingest-dir" => {
                match args.map(str::trim).filter(|s| !s.is_empty()) {
                    None => self.set_status("Usage: :kb-ingest <directory>"),
                    Some(dir) => {
                        let report = self.kb.ingest_org_dir(dir);
                        self.set_status(format!(
                            "kb: indexed {}, skipped {} (no :ID:), errors {}",
                            report.indexed,
                            report.skipped_no_id,
                            report.read_errors.len()
                        ));
                    }
                }
                true
            }
            "kb-save" => {
                self.dispatch_path_op(
                    args,
                    "kb-save",
                    |ed, p| {
                        ed.kb
                            .save_to_sqlite(p)
                            .map(|()| ed.kb.len())
                            .map_err(|e| format!("kb save failed: {}", e))
                    },
                    "Saved",
                    "to",
                );
                true
            }
            "kb-load" => {
                self.dispatch_path_op(
                    args,
                    "kb-load",
                    |ed, p| {
                        ed.kb
                            .load_from_sqlite(p)
                            .map_err(|e| format!("kb load failed: {}", e))
                    },
                    "Loaded",
                    "from",
                );
                true
            }
            "theme" | "set-theme" => {
                if let Some(name) = args {
                    self.set_theme_by_name(name);
                } else if command == "set-theme" {
                    // No arg: open the interactive picker via dispatch.
                    self.dispatch_builtin("set-theme");
                } else {
                    let names = bundled_theme_names().join(", ");
                    self.set_status(format!("Usage: :theme <name>  Available: {}", names));
                }
                true
            }
            "set-splash-art" => {
                if let Some(name) = args {
                    self.splash_art = Some(name.to_string());
                    self.set_status(format!("Splash art set to: {}", name));
                } else {
                    // No arg: open the interactive picker via dispatch.
                    self.dispatch_builtin("set-splash-art");
                }
                true
            }
            "rename" => {
                if let Some(new_path) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    let idx = self.active_buffer_idx();
                    if let Some(old_path) = self.buffers[idx].file_path().map(|p| p.to_path_buf()) {
                        let new = std::path::PathBuf::from(new_path);
                        match std::fs::rename(&old_path, &new) {
                            Ok(()) => {
                                self.buffers[idx].set_file_path(new.clone());
                                self.buffers[idx].name =
                                    new.file_name().map_or(new_path.to_string(), |n| {
                                        n.to_string_lossy().to_string()
                                    });
                                self.set_status(format!(
                                    "Renamed: {} → {}",
                                    old_path.display(),
                                    new.display()
                                ));
                            }
                            Err(e) => self.set_status(format!("Rename failed: {}", e)),
                        }
                    } else {
                        self.set_status("Buffer has no file path");
                    }
                } else {
                    self.set_status("Usage: :rename <new-path>");
                }
                true
            }
            "saveas" => {
                if let Some(path) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    let idx = self.active_buffer_idx();
                    self.buffers[idx].set_file_path(std::path::PathBuf::from(path));
                    self.save_current_buffer();
                } else {
                    self.set_status("Usage: :saveas <path>");
                }
                true
            }
            "lsp-rename" => {
                if let Some(new_name) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    let idx = self.active_buffer_idx();
                    let win = self.window_mgr.focused_window();
                    let path_buf = self.buffers[idx].file_path().map(|p| p.to_path_buf());
                    if let Some(ref p) = path_buf {
                        let uri = crate::lsp_intent::path_to_uri(p);
                        let language_id = crate::lsp_intent::language_id_from_path(p)
                            .unwrap_or_else(|| "plaintext".to_string());
                        self.pending_lsp_requests
                            .push(crate::lsp_intent::LspIntent::Rename {
                                uri,
                                language_id,
                                line: win.cursor_row as u32,
                                character: win.cursor_col as u32,
                                new_name: new_name.to_string(),
                            });
                        self.set_status(format!("LSP rename → '{}'", new_name));
                    } else {
                        self.set_status("LSP rename: buffer has no file path");
                    }
                } else {
                    self.set_status("Usage: :lsp-rename <new-name>");
                }
                true
            }
            "agent-setup" => {
                match args.map(str::trim).filter(|s| !s.is_empty()) {
                    Some(name) => {
                        self.pending_agent_setup = Some(name.to_string());
                    }
                    None => {
                        self.set_status(
                            "Usage: :agent-setup <name>  — use :agent-list to see available agents",
                        );
                    }
                }
                true
            }
            "agent-list" => {
                self.pending_agent_setup = Some("__list__".to_string());
                true
            }
            "noh" | "nohlsearch" => {
                self.search_state.highlight_active = false;
                true
            }
            "set" => {
                if let Some(kv) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    let parts: Vec<&str> = kv.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        match self.set_option(parts[0], parts[1]) {
                            Ok(msg) => self.set_status(msg),
                            Err(e) => self.set_status(e),
                        }
                    } else {
                        // Single arg: toggle boolean or show current value
                        match self.get_option(parts[0]) {
                            Some((val, def)) if def.kind == crate::options::OptionKind::Bool => {
                                let toggled = if val == "true" { "false" } else { "true" };
                                match self.set_option(parts[0], toggled) {
                                    Ok(msg) => self.set_status(msg),
                                    Err(e) => self.set_status(e),
                                }
                            }
                            Some((val, def)) => {
                                self.set_status(format!("{} = {}", def.name, val));
                            }
                            None => self.set_status(format!("Unknown option: {}", parts[0])),
                        }
                    }
                } else {
                    // No args: list all options
                    self.show_all_options();
                }
                true
            }
            "set-save" => {
                if let Some(kv) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    let parts: Vec<&str> = kv.splitn(2, ' ').collect();
                    let key = parts[0];
                    // If value given, apply it first
                    if parts.len() == 2 {
                        if let Err(e) = self.set_option(key, parts[1]) {
                            self.set_status(e);
                            return true;
                        }
                    }
                    // Save current value to config.toml
                    match self.save_option_to_config(key) {
                        Ok(msg) => self.set_status(msg),
                        Err(e) => self.set_status(e),
                    }
                } else {
                    self.set_status("Usage: :set-save <option> [value]");
                }
                true
            }
            "describe-option" => {
                let name = args.map(str::trim).filter(|s| !s.is_empty());
                match name {
                    Some(n) => {
                        // Try to find the option and open its KB node
                        if let Some((_, def)) = self.get_option(n) {
                            let id = format!("option:{}", def.name);
                            if self.kb.contains(&id) {
                                self.open_help_at(&id);
                            } else {
                                // Fallback: show inline
                                let (val, _) = self.get_option(n).unwrap();
                                self.set_status(format!(
                                    "{}: {} (current: {}, default: {})",
                                    def.name, def.doc, val, def.default_value
                                ));
                            }
                        } else {
                            self.set_status(format!("Unknown option: {}", n));
                        }
                    }
                    None => {
                        // Open palette filtered to options
                        self.show_all_options();
                    }
                }
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
                        if let Err(msg) = self.dap_start_with_adapter(adapter, program, &extra_args)
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
