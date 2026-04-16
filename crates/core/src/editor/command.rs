use crate::dap_intent::DapSpawnConfig;
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
            "debug-start" => {
                let arg_str = args.unwrap_or("");
                let mut parts = arg_str.split_whitespace();
                let adapter = parts.next();
                let program = parts.next();
                let extra_args: Vec<String> = parts.map(|s| s.to_string()).collect();
                match (adapter, program) {
                    (Some(adapter), Some(program)) => {
                        match default_spawn_for_adapter(adapter) {
                            Some(spawn) => {
                                let launch_args = default_launch_args(adapter, program, &extra_args);
                                self.dap_start_session(
                                    spawn,
                                    program.to_string(),
                                    launch_args,
                                    false,
                                );
                            }
                            None => {
                                self.set_status(format!(
                                    "Unknown adapter: {} (known: lldb, debugpy, codelldb)",
                                    adapter
                                ));
                            }
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
}

/// Read an env var, falling back to a default string. Exists to keep the
/// adapter table below compact and self-documenting.
fn env_or(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.into())
}

/// Default adapter spawn config for a short name.
/// Returns None for unknown adapters. Callers can override the command
/// via environment variable if the preset binary name doesn't match.
fn default_spawn_for_adapter(adapter: &str) -> Option<DapSpawnConfig> {
    match adapter {
        "lldb" | "lldb-dap" => Some(DapSpawnConfig {
            command: env_or("MAE_DAP_LLDB", "lldb-dap"),
            args: vec![],
            adapter_id: "lldb".into(),
        }),
        "codelldb" => Some(DapSpawnConfig {
            command: env_or("MAE_DAP_CODELLDB", "codelldb"),
            args: vec!["--port".into(), "0".into()],
            adapter_id: "codelldb".into(),
        }),
        "debugpy" | "python" => Some(DapSpawnConfig {
            command: env_or("MAE_DAP_DEBUGPY", "python"),
            args: vec!["-m".into(), "debugpy.adapter".into()],
            adapter_id: "debugpy".into(),
        }),
        _ => None,
    }
}

/// Build the adapter-specific launch args JSON for a `program` path.
/// Keeps the preset minimal so most real programs just work.
fn default_launch_args(adapter: &str, program: &str, extra: &[String]) -> serde_json::Value {
    let base_args: Vec<String> = extra.to_vec();
    match adapter {
        "debugpy" | "python" => serde_json::json!({
            "request": "launch",
            "type": "python",
            "program": program,
            "args": base_args,
            "console": "internalConsole",
            "stopOnEntry": false,
        }),
        _ => serde_json::json!({
            "request": "launch",
            "program": program,
            "args": base_args,
            "stopOnEntry": false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_spawn_for_lldb() {
        let spawn = default_spawn_for_adapter("lldb").unwrap();
        assert_eq!(spawn.adapter_id, "lldb");
    }

    #[test]
    fn default_spawn_unknown_adapter() {
        assert!(default_spawn_for_adapter("nonexistent").is_none());
    }

    #[test]
    fn default_launch_args_python_shape() {
        let v = default_launch_args("debugpy", "/tmp/x.py", &[]);
        assert_eq!(v["type"], "python");
        assert_eq!(v["program"], "/tmp/x.py");
    }

    #[test]
    fn default_launch_args_lldb_shape() {
        let v = default_launch_args("lldb", "/bin/ls", &["--help".to_string()]);
        assert_eq!(v["program"], "/bin/ls");
        assert_eq!(v["args"][0], "--help");
    }

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
}
