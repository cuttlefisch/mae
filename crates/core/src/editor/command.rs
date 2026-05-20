use std::path::Path;

use crate::theme::bundled_theme_names;

use super::ex_parse::{self, ExWriteQuit, SetAction};
use super::Editor;

impl Editor {
    /// Parse and execute a command-line string (the text after ':').
    pub fn execute_command(&mut self, cmd: &str) -> bool {
        let cmd = cmd.trim();
        let (command, args) = match cmd.split_once(' ') {
            Some((c, a)) => (c, Some(a.trim())),
            None => (cmd, None),
        };

        // Write/quit compound commands: w, q, wq, wq!, qa, qa!, wqa, wqa!, x, xa, xa!
        // The `:w <path>` variant needs special handling (args = path).
        if command == "w" && args.is_some() {
            // `:w <filename>` — save-as
            if let Some(path) = args {
                let idx = self.active_buffer_idx();
                self.buffers[idx].set_file_path(std::path::PathBuf::from(path));
            }
            self.save_current_buffer();
            return true;
        }
        if let Some(actions) = ex_parse::parse_write_quit(command) {
            return self.execute_write_quit(&actions);
        }

        match command {
            "e" => {
                if let Some(path) = args {
                    self.open_file(path);
                } else {
                    // No args = revert current buffer (like vim :e)
                    self.dispatch_builtin("revert-buffer");
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
            "agenda" => {
                self.dispatch_builtin("open-agenda");
                true
            }
            "agenda-add" => {
                if let Some(path) = args {
                    self.agenda_add_path(path);
                } else {
                    self.set_status("Usage: :agenda-add <path>");
                }
                true
            }
            "agenda-remove" => {
                if let Some(path) = args {
                    self.agenda_remove_path(path);
                } else {
                    self.set_status("Usage: :agenda-remove <path>");
                }
                true
            }
            "agenda-list" => {
                self.agenda_list_paths();
                true
            }
            "agenda-ingest" => {
                self.ingest_agenda_files();
                self.set_status("Agenda files re-ingested");
                true
            }
            "help-edit" => {
                // `:help-edit <topic>` → open/create ~/.config/mae/help/<topic>.org
                let topic = args
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("scratch");
                let help_dir = std::env::var("XDG_CONFIG_HOME")
                    .ok()
                    .map(std::path::PathBuf::from)
                    .or_else(|| {
                        std::env::var("HOME")
                            .ok()
                            .map(|h| std::path::PathBuf::from(h).join(".config"))
                    })
                    .unwrap_or_else(|| std::path::PathBuf::from(".config"))
                    .join("mae")
                    .join("help");
                let _ = std::fs::create_dir_all(&help_dir);
                let file_path = help_dir.join(format!("{}.org", topic));
                if !file_path.exists() {
                    let template = format!(
                        ":ID: {topic}\n:END:\n#+title: {topic}\n\nWrite your help content here.\n"
                    );
                    let _ = std::fs::write(&file_path, template);
                }
                self.open_file(file_path.display().to_string());
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
                            format!("scheme:{}", topic),
                            format!("option:{}", topic),
                            format!("lesson:{}", topic),
                            format!("tutorial:{}", topic),
                            format!("category:{}", topic),
                        ];
                        let found = candidates.iter().find(|id| self.kb.primary.contains(id));
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
                if self.kb.primary.contains(&id) {
                    self.open_help_at(&id);
                } else {
                    self.set_status(format!("Unknown command: {}", name));
                }
                true
            }
            "describe-module" => {
                let module_name = args.map(str::trim).filter(|s| !s.is_empty());
                self.show_module_report(module_name);
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
                        let report = self.kb.primary.ingest_org_dir(dir);
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
            "kb-create" => {
                match args.map(str::trim).filter(|s| !s.is_empty()) {
                    None => self.set_status("Usage: :kb-create <title> or :kb-create <id> <title>"),
                    Some(input) => {
                        let parts: Vec<&str> = input.splitn(2, ' ').collect();
                        if parts.len() == 2 && (parts[0].contains(':') || !parts[0].contains(' ')) {
                            // Legacy: explicit id + title
                            let id = parts[0];
                            let title = parts[1].trim();
                            match self.kb_create_node(id, title, "", mae_kb::NodeKind::Note) {
                                Ok(()) => {}
                                Err(e) => self.set_status(e),
                            }
                        } else {
                            // New: title-only, auto-generate ID
                            match self.kb_create_note_from_title(input) {
                                Ok(_) => {}
                                Err(e) => self.set_status(e),
                            }
                        }
                    }
                }
                true
            }
            "kb-delete" => {
                match args.map(str::trim).filter(|s| !s.is_empty()) {
                    None => self.set_status("Usage: :kb-delete <id>"),
                    Some(id) => match self.kb_delete_node(id) {
                        Ok(()) => {}
                        Err(e) => self.set_status(e),
                    },
                }
                true
            }
            "kb-register" => {
                match args.map(str::trim).filter(|s| !s.is_empty()) {
                    None => self.set_status("Usage: :kb-register <name> <directory>"),
                    Some(input) => {
                        let parts: Vec<&str> = input.splitn(2, ' ').collect();
                        if parts.len() < 2 {
                            self.set_status("Usage: :kb-register <name> <directory>");
                        } else {
                            let name = parts[0];
                            let dir = crate::file_picker::expand_tilde(parts[1].trim());
                            self.kb_register(name, std::path::Path::new(&dir));
                        }
                    }
                }
                true
            }
            "kb-unregister" => {
                match args.map(str::trim).filter(|s| !s.is_empty()) {
                    None => self.set_status("Usage: :kb-unregister <name>"),
                    Some(name) => self.kb_unregister(name),
                }
                true
            }
            "kb-reimport" => {
                match args.map(str::trim).filter(|s| !s.is_empty()) {
                    None => self.set_status("Usage: :kb-reimport <name>"),
                    Some(name) => {
                        self.kb_reimport(name);
                    }
                }
                true
            }
            "kb-save" => {
                self.dispatch_path_op(
                    args,
                    "kb-save",
                    |editor, p| {
                        editor
                            .kb
                            .primary
                            .save_to_sqlite(p)
                            .map(|()| editor.kb.primary.len())
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
                    |editor, p| {
                        editor
                            .kb
                            .primary
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
            "copy" => {
                if let Some(new_path) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    let idx = self.active_buffer_idx();
                    if let Some(old_path) = self.buffers[idx].file_path().map(|p| p.to_path_buf()) {
                        let new = std::path::PathBuf::from(new_path);
                        match std::fs::copy(&old_path, &new) {
                            Ok(_) => {
                                self.open_file(new.display().to_string());
                                self.set_status(format!(
                                    "Copied: {} → {}",
                                    old_path.display(),
                                    new.display()
                                ));
                            }
                            Err(e) => self.set_status(format!("Copy failed: {}", e)),
                        }
                    } else {
                        self.set_status("Buffer has no file path");
                    }
                } else {
                    self.set_status("Usage: :copy <new-path>");
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
            "recover" => {
                let idx = self.active_buffer_idx();
                let custom_dir = if self.swap_directory.is_empty() {
                    None
                } else {
                    Some(std::path::Path::new(&self.swap_directory))
                };
                if let Some(fp) = self.buffers[idx].file_path().map(|p| p.to_path_buf()) {
                    let swap_path = crate::swap::swap_path_for(&fp, custom_dir);
                    match crate::swap::read_swap(&swap_path) {
                        Ok((_header, rope)) => {
                            self.buffers[idx].replace_rope(rope);
                            self.buffers[idx].modified = true;
                            let _ = crate::swap::delete_swap(&fp, custom_dir);
                            self.buffers[idx].swap = crate::swap::SwapState::default();
                            self.set_status(
                                "Recovered from swap file. Review and :w to save.".to_string(),
                            );
                        }
                        Err(e) => {
                            self.set_status(format!("No swap file to recover: {}", e));
                        }
                    }
                } else {
                    self.set_status("Buffer has no file path");
                }
                true
            }
            "recover-session" => {
                let custom_dir = if self.swap_directory.is_empty() {
                    None
                } else {
                    Some(std::path::Path::new(&self.swap_directory))
                };
                let orphans = crate::swap::find_orphaned_swaps(custom_dir);
                if orphans.is_empty() {
                    self.set_status("No orphaned swap files found");
                } else {
                    let list: Vec<String> = orphans
                        .iter()
                        .map(|(_, h)| format!("{}", h.original_path.display()))
                        .collect();
                    self.set_status(format!(
                        "Recoverable files ({}): {}",
                        orphans.len(),
                        list.join(", ")
                    ));
                }
                true
            }
            "delete-swap" => {
                let idx = self.active_buffer_idx();
                let custom_dir = if self.swap_directory.is_empty() {
                    None
                } else {
                    Some(std::path::Path::new(&self.swap_directory))
                };
                if let Some(fp) = self.buffers[idx].file_path().map(|p| p.to_path_buf()) {
                    match crate::swap::delete_swap(&fp, custom_dir) {
                        Ok(()) => {
                            self.buffers[idx].swap = crate::swap::SwapState::default();
                            self.set_status("Swap file deleted");
                        }
                        Err(e) => {
                            self.set_status(format!("Failed to delete swap: {}", e));
                        }
                    }
                } else {
                    self.set_status("Buffer has no file path");
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
                        self.ai.pending_agent_setup = Some(name.to_string());
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
                self.ai.pending_agent_setup = Some("__list__".to_string());
                true
            }
            "read" | "r" => {
                match args.map(str::trim).filter(|s| !s.is_empty()) {
                    None => {
                        self.set_status("Usage: :read <file> or :read !<command>");
                    }
                    Some(arg) => {
                        if let Some(cmd_str) = arg.strip_prefix('!') {
                            let cmd_str = cmd_str.trim();
                            if cmd_str.is_empty() {
                                self.set_status("Usage: :read !<command>");
                            } else {
                                match std::process::Command::new("sh")
                                    .arg("-c")
                                    .arg(cmd_str)
                                    .output()
                                {
                                    Ok(output) => {
                                        if output.status.success() {
                                            let text = String::from_utf8_lossy(&output.stdout);
                                            self.insert_lines_after_cursor(&text);
                                        } else {
                                            let stderr = String::from_utf8_lossy(&output.stderr);
                                            self.set_status(format!(
                                                "Command failed: {}",
                                                stderr.trim()
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        self.set_status(format!("Shell error: {}", e));
                                    }
                                }
                            }
                        } else {
                            // Read file
                            match std::fs::read_to_string(arg) {
                                Ok(content) => {
                                    self.insert_lines_after_cursor(&content);
                                }
                                Err(e) => {
                                    self.set_status(format!("Cannot read {}: {}", arg, e));
                                }
                            }
                        }
                    }
                }
                true
            }
            "noh" | "nohlsearch" => {
                self.search_state.highlight_active = false;
                true
            }
            "set" => {
                if let Some(kv) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    match ex_parse::parse_set_args(kv) {
                        SetAction::Query(name) => match self.get_option(&name) {
                            Some((val, def)) => {
                                self.set_status(format!("{} = {}", def.name, val));
                            }
                            None => self.set_status(format!("Unknown option: {}", name)),
                        },
                        SetAction::Assign(name, value) => match self.set_option(&name, &value) {
                            Ok(msg) => self.set_status(msg),
                            Err(e) => self.set_status(e),
                        },
                        SetAction::Toggle(name) => match self.get_option(&name) {
                            Some((val, def)) if def.kind == crate::options::OptionKind::Bool => {
                                let toggled = if val == "true" { "false" } else { "true" };
                                match self.set_option(&name, toggled) {
                                    Ok(msg) => self.set_status(msg),
                                    Err(e) => self.set_status(e),
                                }
                            }
                            Some((_, def)) => {
                                self.set_status(format!("{} is not a boolean option", def.name));
                            }
                            None => self.set_status(format!("Unknown option: {}", name)),
                        },
                        SetAction::Enable(name) => match self.get_option(&name) {
                            Some((val, def)) if def.kind == crate::options::OptionKind::Bool => {
                                if val == "true" {
                                    self.set_status(format!("{} = true", def.name));
                                } else {
                                    match self.set_option(&name, "true") {
                                        Ok(msg) => self.set_status(msg),
                                        Err(e) => self.set_status(e),
                                    }
                                }
                            }
                            Some((val, def)) => {
                                self.set_status(format!("{} = {}", def.name, val));
                            }
                            None => self.set_status(format!("Unknown option: {}", name)),
                        },
                        SetAction::Disable(name) => match self.get_option(&name) {
                            Some((_, def)) if def.kind == crate::options::OptionKind::Bool => {
                                match self.set_option(&name, "false") {
                                    Ok(msg) => self.set_status(msg),
                                    Err(e) => self.set_status(e),
                                }
                            }
                            Some((_, def)) => {
                                self.set_status(format!("{} is not a boolean option", def.name));
                            }
                            None => self.set_status(format!("Unknown option: {}", name)),
                        },
                    }
                } else {
                    // No args: list all options
                    self.show_all_options();
                }
                true
            }
            "setlocal" => {
                if let Some(kv) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    match ex_parse::parse_set_args(kv) {
                        SetAction::Assign(name, value) => {
                            match self.set_local_option(&name, &value) {
                                Ok(msg) => self.set_status(msg),
                                Err(e) => self.set_status(e),
                            }
                        }
                        SetAction::Toggle(name) => {
                            // Toggle: read effective, flip, set local
                            let def_name = self.option_registry.find(&name).map(|d| d.name.clone());
                            if let Some(dn) = def_name {
                                let current = match dn.as_ref() {
                                    "word_wrap" => self.effective_word_wrap(),
                                    "line_numbers" => {
                                        self.line_numbers_for(self.active_buffer_idx())
                                    }
                                    "relative_line_numbers" => {
                                        self.relative_line_numbers_for(self.active_buffer_idx())
                                    }
                                    "break_indent" => {
                                        self.break_indent_for(self.active_buffer_idx())
                                    }
                                    "heading_scale" => {
                                        self.heading_scale_for(self.active_buffer_idx())
                                    }
                                    _ => {
                                        self.set_status(format!(
                                            "Option '{}' does not support buffer-local toggle",
                                            dn
                                        ));
                                        return true;
                                    }
                                };
                                let new_val = if current { "false" } else { "true" };
                                match self.set_local_option(&dn, new_val) {
                                    Ok(msg) => self.set_status(msg),
                                    Err(e) => self.set_status(e),
                                }
                            } else {
                                self.set_status(format!("Unknown option: {}", name));
                            }
                        }
                        SetAction::Enable(name) => match self.set_local_option(&name, "true") {
                            Ok(msg) => self.set_status(msg),
                            Err(e) => self.set_status(e),
                        },
                        SetAction::Disable(name) => match self.set_local_option(&name, "false") {
                            Ok(msg) => self.set_status(msg),
                            Err(e) => self.set_status(e),
                        },
                        SetAction::Query(name) => {
                            let def_name = self.option_registry.find(&name).map(|d| d.name.clone());
                            if let Some(dn) = def_name {
                                let idx = self.active_buffer_idx();
                                let local_val = match dn.as_ref() {
                                    "word_wrap" => self.buffers[idx]
                                        .local_options
                                        .word_wrap
                                        .map(|v| v.to_string()),
                                    "line_numbers" => self.buffers[idx]
                                        .local_options
                                        .line_numbers
                                        .map(|v| v.to_string()),
                                    "relative_line_numbers" => self.buffers[idx]
                                        .local_options
                                        .relative_line_numbers
                                        .map(|v| v.to_string()),
                                    "break_indent" => self.buffers[idx]
                                        .local_options
                                        .break_indent
                                        .map(|v| v.to_string()),
                                    "show_break" => {
                                        self.buffers[idx].local_options.show_break.clone()
                                    }
                                    "heading_scale" => self.buffers[idx]
                                        .local_options
                                        .heading_scale
                                        .map(|v| v.to_string()),
                                    _ => None,
                                };
                                match local_val {
                                    Some(v) => {
                                        self.set_status(format!("{} = {} (buffer-local)", dn, v))
                                    }
                                    None => {
                                        self.set_status(format!("{}: no buffer-local override", dn))
                                    }
                                }
                            } else {
                                self.set_status(format!("Unknown option: {}", name));
                            }
                        }
                    }
                } else {
                    self.set_status("Usage: :setlocal <option> [value]");
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
                            if self.kb.primary.contains(&id) {
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
            "set-project-root" => {
                if let Some(path) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    let path = std::path::PathBuf::from(path);
                    if path.is_dir() {
                        let idx = self.active_buffer_idx();
                        self.buffers[idx].project_root = Some(path.clone());
                        self.set_status(format!("Project root set: {}", path.display()));
                    } else {
                        self.set_status(format!("Not a directory: {}", path.display()));
                    }
                } else {
                    self.set_status("Usage: :set-project-root <path>");
                }
                true
            }
            "add-project" => {
                if let Some(path) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    self.add_project(path);
                } else {
                    self.set_status("Usage: :add-project <path>");
                }
                true
            }
            "remove-project" => {
                if let Some(path) = args.map(str::trim).filter(|s| !s.is_empty()) {
                    self.remove_project(path);
                } else {
                    self.set_status("Usage: :remove-project <path>");
                }
                true
            }
            "ai-save" => {
                self.dispatch_path_op(
                    args,
                    "ai-save",
                    |editor, p| editor.ai_save(p),
                    "Saved",
                    "to",
                );
                true
            }
            "ai-load" => {
                self.dispatch_path_op(
                    args,
                    "ai-load",
                    |editor, p| editor.ai_load(p),
                    "Loaded",
                    "from",
                );
                true
            }
            "ai-set-mode" => {
                if let Some(mode) = args {
                    match self.set_option("ai-mode", mode) {
                        Ok(msg) => self.set_status(msg),
                        Err(e) => self.set_status(e),
                    }
                } else {
                    self.dispatch_builtin("ai-set-mode");
                }
                true
            }
            "ai-set-profile" => {
                if let Some(profile) = args {
                    match self.set_option("ai-profile", profile) {
                        Ok(msg) => self.set_status(msg),
                        Err(e) => self.set_status(e),
                    }
                } else {
                    self.dispatch_builtin("ai-set-profile");
                }
                true
            }
            "ai-reset" => {
                self.reset_ai_session();
                self.set_status("AI session reset");
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
            "debug-attach" => {
                let arg_str = args.unwrap_or("");
                let mut parts = arg_str.split_whitespace();
                let adapter = parts.next();
                let pid_str = parts.next();
                match (adapter, pid_str) {
                    (Some(adapter), Some(pid_str)) => match pid_str.parse::<u32>() {
                        Ok(pid) => {
                            if let Err(msg) = self.dap_attach_with_adapter(adapter, pid) {
                                self.set_status(msg);
                            }
                        }
                        Err(_) => {
                            self.set_status("Usage: :debug-attach <adapter> <pid>");
                        }
                    },
                    _ => {
                        self.set_status(
                            "Usage: :debug-attach <adapter> <pid>  — adapters: lldb, debugpy, codelldb",
                        );
                    }
                }
                true
            }
            "debug-eval" => {
                let expression = args.unwrap_or("").trim();
                if expression.is_empty() {
                    self.set_status("Usage: :debug-eval <expression>");
                } else if self.debug_state.is_none() {
                    self.set_status("No active debug session");
                } else {
                    self.dap_evaluate(expression, None, Some("repl"));
                    self.set_status(format!("Evaluating: {}", expression));
                }
                true
            }
            "debug-exceptions" => {
                let arg = args.unwrap_or("").trim();
                let filters = match arg {
                    "caught" => vec!["caught".to_string()],
                    "uncaught" => vec!["uncaught".to_string()],
                    "all" | "" => vec!["caught".to_string(), "uncaught".to_string()],
                    "none" | "clear" => vec![],
                    other => other.split(',').map(|s| s.trim().to_string()).collect(),
                };
                self.dap_set_exception_breakpoints(filters);
                true
            }
            "debug-add-watch" => {
                let expression = args.unwrap_or("").trim();
                if expression.is_empty() {
                    self.set_status("Usage: :debug-add-watch <expression>");
                } else {
                    self.debug_add_watch(expression.to_string());
                    self.debug_panel_refresh_if_open();
                }
                true
            }
            "debug-remove-watch" => {
                let idx_str = args.unwrap_or("").trim();
                match idx_str.parse::<usize>() {
                    Ok(idx) => {
                        self.debug_remove_watch(idx);
                        self.debug_panel_refresh_if_open();
                    }
                    Err(_) => self.set_status("Usage: :debug-remove-watch <index>"),
                }
                true
            }
            "session-save" => {
                let root = self
                    .active_project_root()
                    .map(|p| p.to_path_buf())
                    .or_else(|| self.project.as_ref().map(|p| p.root.clone()));
                match root {
                    Some(root) => {
                        let session = crate::session::Session::from_editor(self);
                        match session.save(&root) {
                            Ok(()) => self.set_status(format!(
                                "Session saved ({} buffers)",
                                session.buffers.len()
                            )),
                            Err(e) => self.set_status(e),
                        }
                    }
                    None => self.set_status("No project root — cannot save session"),
                }
                true
            }
            "session-load" => {
                let root = self
                    .active_project_root()
                    .map(|p| p.to_path_buf())
                    .or_else(|| self.project.as_ref().map(|p| p.root.clone()));
                match root {
                    Some(root) => match crate::session::Session::load(&root) {
                        Ok(session) => {
                            let count = session.buffers.len();
                            for sb in &session.buffers {
                                if !self
                                    .buffers
                                    .iter()
                                    .any(|b| b.file_path() == Some(&sb.file_path))
                                {
                                    self.open_file(&sb.file_path);
                                }
                            }
                            self.set_status(format!("Session loaded ({} buffers)", count));
                        }
                        Err(e) => self.set_status(e),
                    },
                    None => self.set_status("No project root — cannot load session"),
                }
                true
            }
            "tutor" => {
                self.dispatch_builtin("tutor");
                true
            }
            "record-start" => {
                self.dispatch_builtin("record-start");
                true
            }
            "record-stop" => {
                self.dispatch_builtin("record-stop");
                true
            }
            "record-save" => {
                self.dispatch_path_op(
                    args,
                    "record-save",
                    |editor, p| editor.event_recorder.save(p),
                    "Saved",
                    "to",
                );
                true
            }
            // H1-H5 Doom parity commands
            "format-buffer" | "format" => {
                self.dispatch_builtin("format-buffer");
                true
            }
            "spell-check" | "spell-toggle" => {
                self.dispatch_builtin("spell-check-buffer");
                true
            }
            "spell-next" => {
                self.dispatch_builtin("spell-next");
                true
            }
            "spell-prev" => {
                self.dispatch_builtin("spell-prev");
                true
            }
            "spell-suggest" => {
                self.dispatch_builtin("spell-suggest");
                true
            }
            "run-build" | "make" => {
                self.dispatch_builtin("run-build");
                true
            }
            "run-test" => {
                self.dispatch_builtin("run-test");
                true
            }
            "next-error" => {
                self.dispatch_builtin("next-error");
                true
            }
            "prev-error" => {
                self.dispatch_builtin("prev-error");
                true
            }
            "lookup-online" => {
                self.dispatch_builtin("lookup-online");
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
                // Global commands: :g/pattern/cmd and :v/pattern/cmd
                if cmd.starts_with("g/") || cmd.starts_with("v/") {
                    self.execute_global_command(cmd);
                    return true;
                }
                // Check for substitute commands: s/.../.../  or %s/.../.../ or range s/
                if cmd.starts_with("s/") || cmd.starts_with("%s/") {
                    self.execute_substitute_command(cmd);
                    return true;
                }
                // Range-prefixed substitute: .,+5s/...  1,10s/...  $s/...
                if cmd.contains("s/") {
                    if let Some((start, end, sub_cmd)) = self.parse_ex_range(cmd) {
                        self.execute_substitute_with_range(sub_cmd, Some((start, end)));
                        return true;
                    }
                }
                // collab-join with a doc name argument: join directly.
                if command == "collab-join" {
                    if let Some(doc_name) = args.map(str::trim).filter(|s| !s.is_empty()) {
                        self.collab.pending_intent = Some(super::CollabIntent::JoinDoc {
                            doc_id: doc_name.to_string(),
                        });
                        self.set_status(format!("Joining: {}...", doc_name));
                        return true;
                    }
                    // No arg: open palette picker (falls through to dispatch_builtin)
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

    /// Execute a parsed write/quit compound command.
    fn execute_write_quit(&mut self, actions: &[ExWriteQuit]) -> bool {
        for action in actions {
            match action {
                ExWriteQuit::Write { all: false } => {
                    self.save_current_buffer();
                    if !self.running {
                        return true;
                    }
                }
                ExWriteQuit::Write { all: true } => {
                    let (saved, errors) = self.save_all_modified_buffers();
                    if !errors.is_empty() {
                        self.set_status(format!("Saved {}, errors: {}", saved, errors.join(", ")));
                        return true;
                    }
                    // If this is a standalone :wa (no quit follows), show status.
                    if !actions
                        .iter()
                        .any(|a| matches!(a, ExWriteQuit::Quit { .. }))
                    {
                        self.set_status(format!("Saved {} buffer(s)", saved));
                    }
                }
                ExWriteQuit::WriteIfModified { all: false } => {
                    if self.active_buffer().modified {
                        self.save_current_buffer();
                    }
                }
                ExWriteQuit::WriteIfModified { all: true } => {
                    let (_saved, errors) = self.save_all_modified_buffers();
                    if !errors.is_empty() {
                        self.set_status(format!("Save errors: {}", errors.join(", ")));
                        return true;
                    }
                }
                ExWriteQuit::Quit { all, force } => {
                    if *all {
                        if !force && self.any_buffer_modified() {
                            self.set_status("No write since last change (add ! to override)");
                            return true;
                        }
                        self.on_quit();
                        self.running = false;
                    } else {
                        // :q without ! — close current window if multiple exist
                        if !force && self.active_buffer().modified {
                            self.set_status("No write since last change (add ! to override)");
                            return true;
                        }
                        if self.window_mgr.window_count() > 1 {
                            // Close focused window, don't exit the editor
                            self.dispatch_builtin("close-window");
                        } else {
                            self.on_quit();
                            self.running = false;
                        }
                    }
                }
            }
        }
        true
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
        let mut editor = Editor::new();
        editor.execute_command("debug-start");
        assert!(editor.status_msg.to_lowercase().contains("usage"));
        assert!(editor.pending_dap_intents.is_empty());
    }

    #[test]
    fn debug_start_command_queues_intent() {
        let mut editor = Editor::new();
        editor.execute_command("debug-start lldb /bin/ls");
        assert_eq!(editor.pending_dap_intents.len(), 1);
    }

    #[test]
    fn debug_start_command_unknown_adapter_sets_status() {
        let mut editor = Editor::new();
        editor.execute_command("debug-start bogus /bin/ls");
        assert!(editor.status_msg.contains("Unknown adapter"));
        assert!(editor.pending_dap_intents.is_empty());
    }

    #[test]
    fn ai_save_without_args_shows_usage() {
        let mut editor = Editor::new();
        editor.execute_command("ai-save");
        assert!(editor.status_msg.to_lowercase().contains("usage"));
    }

    #[test]
    fn ai_save_without_conversation_sets_error() {
        let mut editor = Editor::new();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        editor.execute_command(&format!("ai-save {}", tmp.path().display()));
        assert!(editor.status_msg.contains("No conversation"));
    }

    #[test]
    fn ai_load_without_args_shows_usage() {
        let mut editor = Editor::new();
        editor.execute_command("ai-load");
        assert!(editor.status_msg.to_lowercase().contains("usage"));
    }

    #[test]
    fn ai_save_and_load_round_trip_via_commands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conv.json");

        let mut editor = Editor::new();
        editor.open_conversation_buffer();
        editor.conversation_mut().unwrap().push_user("round-trip");

        editor.execute_command(&format!("ai-save {}", path.display()));
        assert!(editor.status_msg.contains("Saved 1 entries"));
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("round-trip"));

        // Mutate, then reload: load must replace, not merge.
        editor
            .conversation_mut()
            .unwrap()
            .push_user("to-be-replaced");
        assert_eq!(editor.conversation().unwrap().entries.len(), 2);

        editor.execute_command(&format!("ai-load {}", path.display()));
        assert!(editor.status_msg.contains("Loaded 1 entries"));
        assert_eq!(editor.conversation().unwrap().entries.len(), 1);
    }

    #[test]
    fn read_command_shell_inserts_output() {
        let mut editor = Editor::new();
        // Put some content in the buffer so cursor is on a real line
        editor.active_buffer_mut().insert_text_at(0, "first line\n");
        editor.execute_command("read !echo hello");
        let content = editor.active_buffer().rope().to_string();
        assert!(content.contains("hello"), "content was: {}", content);
        assert!(editor.status_msg.contains("1 lines inserted"));
    }

    #[test]
    fn read_command_file_inserts_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "file content\n").unwrap();
        let mut editor = Editor::new();
        editor.execute_command(&format!("read {}", path.display()));
        let content = editor.active_buffer().rope().to_string();
        assert!(content.contains("file content"), "content was: {}", content);
    }

    #[test]
    fn read_command_no_args_shows_usage() {
        let mut editor = Editor::new();
        editor.execute_command("read");
        assert!(
            editor.status_msg.to_lowercase().contains("usage"),
            "status was: {}",
            editor.status_msg
        );
    }

    #[test]
    fn r_alias_works() {
        let mut editor = Editor::new();
        editor.execute_command("r !echo test");
        let content = editor.active_buffer().rope().to_string();
        assert!(content.contains("test"), "content was: {}", content);
    }
}
